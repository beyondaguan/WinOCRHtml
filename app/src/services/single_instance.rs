// ============================================================
// Services · 单实例守护（P2-4）
// 通过命名 mutex 保证同一时刻只有一个 WinOCR 进程运行。
// 若已有实例，尝试通过窗口 title 找到旧窗口并置前，
// 然后新实例退出。
// ============================================================
use std::ffi::OsString;
use std::os::windows::ffi::OsStrExt;
use std::ptr;

use anyhow::Result;
use log::info;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};

/// 单实例守护。持有命名 mutex，Drop 时释放。
pub struct SingleInstanceGuard {
    handle: HANDLE,
}

impl SingleInstanceGuard {
    /// 尝试创建单实例锁。
    /// 返回 `Some(guard)` 表示当前是唯一实例；
    /// 返回 `None` 表示已有实例运行（并尝试激活旧窗口）。
    pub fn new(app_title: &str) -> Result<Option<Self>> {
        let mutex_name = to_wide("WinOCR_SingleInstance_Mutex_v1");
        let handle = unsafe { CreateMutexW(ptr::null_mut(), 1, mutex_name.as_ptr()) };
        if handle.is_null() {
            anyhow::bail!("CreateMutexW 失败");
        }
        let err = unsafe { GetLastError() };
        if err == ERROR_ALREADY_EXISTS {
            info!("[single_instance] 检测到已有实例，尝试激活旧窗口");
            let title_w = to_wide(app_title);
            let hwnd = unsafe { FindWindowW(ptr::null(), title_w.as_ptr()) };
            if !hwnd.is_null() {
                unsafe { SetForegroundWindow(hwnd) };
            }
            unsafe { CloseHandle(handle) };
            Ok(None)
        } else {
            Ok(Some(Self { handle }))
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.handle);
            CloseHandle(self.handle);
        }
    }
}

/// 按窗口标题查找顶层窗口并置前（Windows 前台锁定规则下，仅 show() 不一定能抢到前台）。
/// 供热键路径复用：结果回填前把主窗口提到前台。返回是否找到窗口。
pub fn activate_window_by_title(title: &str) -> bool {
    let title_w = to_wide(title);
    let hwnd = unsafe { FindWindowW(ptr::null(), title_w.as_ptr()) };
    if !hwnd.is_null() {
        unsafe { SetForegroundWindow(hwnd) };
        true
    } else {
        false
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
