// ============================================================
// Core · DPAPI 封装
// 使用 Windows 数据保护 API 加密本地密钥，避免明文落盘。
//
// 对应计划文档 §13.5 / §15.9 红线：API Key 不得明文出现在磁盘上。
// 通过 libloading 直接调用 crypt32.dll，不引入新 feature，
// 规避 gnu 工具链下 windows-sys 重编译需要 dlltool 的限制。
//
// 加密字段：sf_key / ai_sf_key / youdao_app_key / youdao_app_secret
// ============================================================
use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use libloading::Library;
use std::ffi::c_void;
use std::ptr;

#[repr(C)]
struct CryptBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

type CryptProtectDataFn = unsafe extern "system" fn(
    p_data_in: *const CryptBlob,
    sz_data_descr: *const u16,
    p_optional_entropy: *const CryptBlob,
    pv_reserved: *const c_void,
    p_prompt_struct: *const c_void,
    dw_flags: u32,
    p_data_out: *mut CryptBlob,
) -> i32;

type CryptUnprotectDataFn = unsafe extern "system" fn(
    p_data_in: *const CryptBlob,
    ppsz_data_descr: *mut *mut u16,
    p_optional_entropy: *const CryptBlob,
    pv_reserved: *const c_void,
    p_prompt_struct: *const c_void,
    dw_flags: u32,
    p_data_out: *mut CryptBlob,
) -> i32;

type LocalFreeFn = unsafe extern "system" fn(hmem: *mut c_void) -> *mut c_void;

fn local_free(ptr: *mut u8) {
    if let Ok(lib) = unsafe { Library::new("kernel32.dll") } {
        if let Ok(f) = unsafe { lib.get::<LocalFreeFn>(b"LocalFree") } {
            unsafe {
                f(ptr as *mut c_void);
            }
        }
    }
}

/// 加密任意字符串，返回 base64 编码的 DPAPI blob。空字符串直接返回空。
pub fn encrypt_secret(plain: &str) -> Result<String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    let data = plain.as_bytes();
    let in_blob = CryptBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut out_blob = CryptBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    let lib = unsafe { Library::new("crypt32.dll") }.context("加载 crypt32.dll 失败")?;
    let f = unsafe { lib.get::<CryptProtectDataFn>(b"CryptProtectData")? };
    let res = unsafe {
        f(
            &in_blob,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &mut out_blob,
        )
    };
    if res == 0 {
        return Err(anyhow::anyhow!("CryptProtectData 失败 (res=0)"));
    }
    let out =
        unsafe { std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize) }.to_vec();
    local_free(out_blob.pb_data);
    Ok(B64.encode(out))
}

/// 解密 base64 编码的 DPAPI blob 为明文字符串。
pub fn decrypt_secret(b64: &str) -> Result<String> {
    if b64.is_empty() {
        return Ok(String::new());
    }
    let raw = B64.decode(b64).context("DPAPI 密文 base64 解码失败")?;
    let in_blob = CryptBlob {
        cb_data: raw.len() as u32,
        pb_data: raw.as_ptr() as *mut u8,
    };
    let mut out_blob = CryptBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };
    let lib = unsafe { Library::new("crypt32.dll") }.context("加载 crypt32.dll 失败")?;
    let f = unsafe { lib.get::<CryptUnprotectDataFn>(b"CryptUnprotectData")? };
    let res = unsafe {
        f(
            &in_blob,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &mut out_blob,
        )
    };
    if res == 0 {
        return Err(anyhow::anyhow!("CryptUnprotectData 失败 (res=0)"));
    }
    let out =
        unsafe { std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize) }.to_vec();
    local_free(out_blob.pb_data);
    String::from_utf8(out).context("解密结果不是合法 UTF-8")
}
