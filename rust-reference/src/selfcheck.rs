//! NM 注册自愈 / 一键自检：校验注册表、manifest、exe 与扩展 ID 一致性。
//!
//! 对应计划文档 §13.4.5 / §15.3 / §15.6（selfCheck 与 Doctor）。
//! 通过 `libloading` 调用 `advapi32.dll` 读取 `NativeMessagingHosts` 注册表项，
//! 判断当前宿主是否被浏览器正确识别，并返回可结构化渲染的诊断 JSON
//! （扩展端 Doctor 与 `doctor.ps1` 均可消费）。

use libloading::Library;
use serde_json::{json, Value};
use std::ffi::c_void;
use std::ptr;
use std::path::Path;

/// Native Messaging Host 名称（与 `com.winocr_host.json` 的 `name` 一致）
pub const HOST_NAME: &str = "com.winocr_host";
/// 本扩展固定 ID（来自 manifest 的 `allowed_origins`）
pub const EXTENSION_ID: &str = "lakabkkdcnfpoeodnidihbcnlbjbnkdm";

/// 待检测的浏览器注册表项（HKCU 下）
const REG_ROOTS: &[(&str, &str)] = &[
    (
        "Chrome",
        "Software\\Google\\Chrome\\NativeMessagingHosts\\com.winocr_host",
    ),
    (
        "Edge",
        "Software\\Microsoft\\Edge\\NativeMessagingHosts\\com.winocr_host",
    ),
    (
        "Firefox",
        "Software\\Mozilla\\NativeMessagingHosts\\com.winocr_host",
    ),
];

type HKEY = *mut c_void;
const HKEY_CURRENT_USER: HKEY = 0x8000_0001 as HKEY;
const KEY_READ: u32 = 0x20019;
const KEY_WOW64_64KEY: u32 = 0x0100;

type RegOpenKeyExWFn = unsafe extern "system" fn(
    hkey: HKEY,
    subkey: *const u16,
    options: u32,
    sam: u32,
    phk: *mut HKEY,
) -> i32;
type RegQueryValueExWFn = unsafe extern "system" fn(
    hkey: HKEY,
    value: *const u16,
    reserved: *const u32,
    ty: *mut u32,
    data: *mut u8,
    cb: *mut u32,
) -> i32;
type RegCloseKeyFn = unsafe extern "system" fn(hkey: HKEY) -> i32;

/// 读取注册表键的默认值（REG_SZ），不存在或读取失败返回 None
fn reg_query_default_string(subkey: &str) -> Option<String> {
    let wide: Vec<u16> = subkey
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let lib = unsafe { Library::new("advapi32.dll") }.ok()?;
    let open = unsafe { lib.get::<RegOpenKeyExWFn>(b"RegOpenKeyExW").ok()? };
    let mut hkey: HKEY = ptr::null_mut();
    let st = unsafe { open(HKEY_CURRENT_USER, wide.as_ptr(), 0, KEY_READ | KEY_WOW64_64KEY, &mut hkey) };
    if st != 0 || hkey.is_null() {
        return None;
    }

    let query = match unsafe { lib.get::<RegQueryValueExWFn>(b"RegQueryValueExW") } {
        Ok(s) => s,
        Err(_) => {
            if let Ok(c) = unsafe { lib.get::<RegCloseKeyFn>(b"RegCloseKey") } {
                unsafe { c(hkey) };
            }
            return None;
        }
    };

    let mut size: u32 = 0;
    let mut ty: u32 = 0;
    let st2 = unsafe {
        query(
            hkey,
            ptr::null(),
            ptr::null(),
            &mut ty as *mut u32,
            ptr::null_mut(),
            &mut size,
        )
    };
    // ERROR_SUCCESS(0) 或 ERROR_MORE_DATA(234) 都可能带回 size
    if (st2 != 0 && st2 != 234) || size == 0 {
        if let Ok(c) = unsafe { lib.get::<RegCloseKeyFn>(b"RegCloseKey") } {
            unsafe { c(hkey) };
        }
        return None;
    }

    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let st3 = unsafe {
        query(
            hkey,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            buf.as_mut_ptr(),
            &mut size,
        )
    };
    if let Ok(c) = unsafe { lib.get::<RegCloseKeyFn>(b"RegCloseKey") } {
        unsafe { c(hkey) };
    }
    if st3 != 0 {
        return None;
    }

    // REG_SZ：UTF-16LE，去掉结尾空字符
    let wchars: Vec<u16> = buf
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let s = String::from_utf16_lossy(&wchars);
    Some(s.trim_end_matches('\u{0}').to_string())
}

/// 运行完整自检，返回结构化诊断 JSON
pub fn run_self_check() -> Value {
    let current_exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let config_path = crate::config::default_config_path().unwrap_or_default();
    let config_exists = Path::new(&config_path).exists();
    let config_version = crate::config::Config::load(None)
        .map(|c| c.config_version)
        .unwrap_or(0);

    let mut browsers = Vec::new();
    let mut any_ok = false;

    for (name, key) in REG_ROOTS {
        let mut entry = serde_json::Map::new();
        entry.insert("name".to_string(), json!(name));

        match reg_query_default_string(key) {
            None => {
                entry.insert("registered".to_string(), json!(false));
                entry.insert("manifest_path".to_string(), json!(null));
                entry.insert("manifest_exists".to_string(), json!(false));
                entry.insert("exe_path".to_string(), json!(null));
                entry.insert("exe_exists".to_string(), json!(false));
                entry.insert("exe_matches".to_string(), json!(false));
                entry.insert("allowed_origin_ok".to_string(), json!(false));
            }
            Some(mp) => {
                let manifest_exists = Path::new(&mp).exists();
                let (exe_path, exe_exists, exe_matches, allowed_ok) = if manifest_exists {
                    match std::fs::read_to_string(&mp) {
                        Ok(txt) => match serde_json::from_str::<Value>(&txt) {
                            Ok(m) => {
                                let exe = m
                                    .get("path")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let exe_exists =
                                    !exe.is_empty() && Path::new(&exe).exists();
                                let exe_matches = exe_exists
                                    && Path::new(&exe).canonicalize().ok()
                                        == Path::new(&current_exe).canonicalize().ok();
                                let allowed_ok = m
                                    .get("allowed_origins")
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter().any(|x| {
                                            x.as_str()
                                                .map(|s| s.contains(EXTENSION_ID))
                                                .unwrap_or(false)
                                        })
                                    })
                                    .unwrap_or(false);
                                (exe, exe_exists, exe_matches, allowed_ok)
                            }
                            Err(_) => (String::new(), false, false, false),
                        },
                        Err(_) => (String::new(), false, false, false),
                    }
                } else {
                    (String::new(), false, false, false)
                };

                entry.insert("registered".to_string(), json!(true));
                entry.insert("manifest_path".to_string(), json!(mp));
                entry.insert("manifest_exists".to_string(), json!(manifest_exists));
                entry.insert("exe_path".to_string(), json!(exe_path));
                entry.insert("exe_exists".to_string(), json!(exe_exists));
                entry.insert("exe_matches".to_string(), json!(exe_matches));
                entry.insert("allowed_origin_ok".to_string(), json!(allowed_ok));

                if manifest_exists && exe_exists && allowed_ok {
                    any_ok = true;
                }
            }
        }
        browsers.push(Value::Object(entry));
    }

    json!({
        "host_name": HOST_NAME,
        "app_version": crate::APP_VERSION,
        "current_exe": current_exe,
        "config_path": config_path,
        "config_exists": config_exists,
        "config_version": config_version,
        "registered_any": any_ok,
        "browsers": browsers,
        "timestamp": now_unix(),
    })
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
