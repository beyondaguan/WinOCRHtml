// ============================================================
// Services · 系统托盘（真实实现）
// 移植自 rust-reference/tray.rs（winapi → windows-sys 0.59）
// 设计：独立线程创建隐藏消息窗口，托盘事件经 mpsc 回传。
// ============================================================
use std::ffi::OsString;
use std::mem;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::sync::mpsc::{channel, Sender};
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result};
use log::error;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    GetCursorPos, GetMessageW, GetWindowLongPtrW, HICON, PostMessageW, PostThreadMessageW,
    RegisterClassW, SetForegroundWindow, SetWindowLongPtrW, TrackPopupMenu, TranslateMessage, MSG,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_RIGHTBUTTON, WM_COMMAND, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
    WM_NULL, WM_QUIT, WM_RBUTTONUP, WM_USER, WNDCLASSW, GWLP_USERDATA, HWND_MESSAGE,
};

/// 托盘菜单触发的事件
#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    Capture,
    Settings,
    Pause,
    SelfCheck,
    History,
    Collect,
    CollectReview,
    OpenMain,
    Quit,
}

const WM_TRAY_CALLBACK: u32 = WM_USER + 1;
const IDM_CAPTURE: usize = 1;
const IDM_SETTINGS: usize = 2;
const IDM_PAUSE: usize = 3;
const IDM_SELFCHECK: usize = 4;
const IDM_HISTORY: usize = 6;
const IDM_VOCAB: usize = 7;
const IDM_REVIEW: usize = 8;
const IDM_OPENMAIN: usize = 10;
const IDM_QUIT: usize = 9;

const TRAY_CLASS: &str = "WinOCRSlintTrayWnd";

struct TrayCtx {
    events: Sender<TrayEvent>,
    nid: NOTIFYICONDATAW,
    #[allow(dead_code)] // P2-4：窗口句柄保留用于后续托盘图标重定位
    hwnd: HWND,
}

pub struct TrayIcon {
    thread_id: u32,
    _join: Option<JoinHandle<()>>,
}

impl TrayIcon {
    pub fn new(events: Sender<TrayEvent>, first_run: bool) -> Result<Self> {
        let (id_tx, id_rx) = channel::<u32>();
        let join = thread::Builder::new()
            .name("winocr-tray".into())
            .spawn(move || {
                let tid = unsafe {
                    windows_sys::Win32::System::Threading::GetCurrentThreadId()
                };
                let _ = id_tx.send(tid);
                if let Err(e) = unsafe { tray_thread(events, first_run) } {
                    error!("托盘线程异常: {}", e);
                }
            })?;
        let thread_id = id_rx.recv().context("托盘线程启动失败")?;
        Ok(Self {
            thread_id,
            _join: Some(join),
        })
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
    }
}

unsafe fn to_wide(s: &str) -> Vec<u16> {
    OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

unsafe fn tray_thread(events: Sender<TrayEvent>, first_run: bool) -> Result<()> {
    let cls_w = to_wide(TRAY_CLASS);
    let hinst = GetModuleHandleW(ptr::null_mut());
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: ptr::null_mut(),
        hCursor: ptr::null_mut(),
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null_mut(),
        lpszClassName: cls_w.as_ptr(),
    };
    RegisterClassW(&wc);

    let hwnd = CreateWindowExW(
        0,
        cls_w.as_ptr(),
        ptr::null(),
        0,
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        ptr::null_mut(),
        hinst,
        ptr::null_mut(),
    );
    if hwnd.is_null() {
        anyhow::bail!("创建托盘消息窗口失败");
    }

    let mut nid: NOTIFYICONDATAW = mem::zeroed();
    nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY_CALLBACK;
    nid.hIcon = load_tray_icon();
    let tip = to_wide("WinOCR");
    ptr::copy_nonoverlapping(tip.as_ptr(), nid.szTip.as_mut_ptr(), tip.len().min(128));
    Shell_NotifyIconW(NIM_ADD, &mut nid);

    if first_run {
        nid.uFlags |= NIF_INFO;
        let info = to_wide("欢迎使用 WinOCR！右键任务栏图标即可截图翻译或退出。");
        ptr::copy_nonoverlapping(
            info.as_ptr(),
            nid.szInfo.as_mut_ptr(),
            info.len().min(nid.szInfo.len()),
        );
        let title = to_wide("首次运行");
        ptr::copy_nonoverlapping(
            title.as_ptr(),
            nid.szInfoTitle.as_mut_ptr(),
            title.len().min(nid.szInfoTitle.len()),
        );
        nid.Anonymous.uTimeout = 15000;
        nid.dwInfoFlags = NIIF_INFO;
        Shell_NotifyIconW(NIM_MODIFY, &mut nid);
    }

    let ctx = Box::new(TrayCtx { events, nid, hwnd });
    let ctx_ptr = Box::into_raw(ctx);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);

    let mut msg: MSG = mem::zeroed();
    while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    let mut ctx = Box::from_raw(ctx_ptr);
    Shell_NotifyIconW(NIM_DELETE, &mut ctx.nid);
    Ok(())
}

unsafe fn load_tray_icon() -> HICON {
    let mut buf = [0u16; 1024];
    let n = GetModuleFileNameW(ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32);
    if n > 0 {
        let exe = String::from_utf16_lossy(&buf[..n as usize]);
        if let Some(parent) = std::path::Path::new(&exe).parent() {
            let ico = parent.join("tray.ico");
            if let Some(p) = ico.to_str() {
                let w = to_wide(p);
                let h = windows_sys::Win32::UI::WindowsAndMessaging::LoadImageW(
                    ptr::null_mut(),
                    w.as_ptr(),
                    0, // IMAGE_ICON
                    0,
                    0,
                    0x0010 | 0x0040, // LR_LOADFROMFILE | LR_DEFAULTSIZE
                ) as HICON;
                if !h.is_null() {
                    return h;
                }
            }
        }
    }
    windows_sys::Win32::UI::WindowsAndMessaging::LoadIconW(
        GetModuleHandleW(ptr::null_mut()),
        32512 as *const u16, // IDI_APPLICATION (MAKEINTRESOURCE)
    )
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_TRAY_CALLBACK => {
            let ctx = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayCtx);
            match lparam as u32 {
                WM_RBUTTONUP | WM_LBUTTONUP => show_menu(hwnd),
                WM_LBUTTONDBLCLK => {
                    let _ = ctx.events.send(TrayEvent::OpenMain);
                }
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            let id = wparam;
            let ctx = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayCtx);
            let ev = match id {
                IDM_CAPTURE => Some(TrayEvent::Capture),
                IDM_SETTINGS => Some(TrayEvent::Settings),
                IDM_PAUSE => Some(TrayEvent::Pause),
                IDM_SELFCHECK => Some(TrayEvent::SelfCheck),
                IDM_HISTORY => Some(TrayEvent::History),
                IDM_VOCAB => Some(TrayEvent::Collect),
                IDM_REVIEW => Some(TrayEvent::CollectReview),
                IDM_OPENMAIN => Some(TrayEvent::OpenMain),
                IDM_QUIT => Some(TrayEvent::Quit),
                _ => None,
            };
            if let Some(ev) = ev {
                let _ = ctx.events.send(ev);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    AppendMenuW(menu, 0x00000000, IDM_CAPTURE, to_wide("截图翻译(&S)").as_ptr()); // MF_STRING
    AppendMenuW(menu, 0x00000000, IDM_SETTINGS, to_wide("打开设置(&O)").as_ptr());
    AppendMenuW(menu, 0x00000000, IDM_PAUSE, to_wide("暂停/恢复(&P)").as_ptr());
    AppendMenuW(menu, 0x00000008, 0, ptr::null()); // MF_SEPARATOR
    AppendMenuW(menu, 0x00000000, IDM_SELFCHECK, to_wide("运行诊断(&D)").as_ptr());
    AppendMenuW(menu, 0x00000000, IDM_HISTORY, to_wide("历史记录").as_ptr());
    AppendMenuW(menu, 0x00000000, IDM_OPENMAIN, to_wide("打开主窗口(&M)").as_ptr());
    AppendMenuW(menu, 0x00000000, IDM_QUIT, to_wide("退出(&X)").as_ptr());

    let mut pt: windows_sys::Win32::Foundation::POINT = mem::zeroed();
    GetCursorPos(&mut pt);
    SetForegroundWindow(hwnd);
    TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_LEFTALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        ptr::null(),
    );
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
}
