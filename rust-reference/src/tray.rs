//! 系统托盘图标 + 右键菜单（§17.2）
//!
//! 设计：在独立线程里创建隐藏消息窗口并跑 Win32 消息泵，
//! 托盘回调（左/右键、气泡）解析为菜单命令，经 mpsc 通道回传给主循环。
//! 这样主线程的业务逻辑（热键 / 截图 / NM）集中处理，互不干扰。
//! 图标优先用 exe 同目录的 `tray.ico`，缺失时回退系统图标。

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr;
use std::mem;
use std::sync::mpsc::{channel, Sender};
use std::thread::{self, JoinHandle};

use anyhow::Context;
use winapi::shared::minwindef::{LRESULT, UINT, WPARAM, LPARAM};
use winapi::shared::windef::{HWND, HICON, HMENU, POINT};
use winapi::um::shellapi::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIIF_INFO,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use winapi::um::libloaderapi::{GetModuleFileNameW, GetModuleHandleW};
use winapi::um::processthreadsapi::GetCurrentThreadId;
use winapi::um::winuser::*;

/// 托盘菜单触发的事件，由主循环消费
#[derive(Debug, Clone, Copy)]
pub enum TrayEvent {
    Capture,   // 截图翻译
    Settings,  // 打开设置
    Pause,     // 暂停/恢复
    SelfCheck, // 运行诊断
    History,   // 历史记录（§17.6）
    Vocab,     // 生词本（§17.6）
    Review,    // 复习闪卡（§17.6）
    OpenMain,  // 打开主窗口（启动 GUI 仪表盘）
    Quit,      // 退出
}

const WM_TRAY_CALLBACK: UINT = WM_USER + 1;
const IDM_CAPTURE: usize = 1;
const IDM_SETTINGS: usize = 2;
const IDM_PAUSE: usize = 3;
const IDM_SELFCHECK: usize = 4;
const IDM_HISTORY: usize = 6;
const IDM_VOCAB: usize = 7;
const IDM_REVIEW: usize = 8;
const IDM_OPENMAIN: usize = 10;
const IDM_QUIT: usize = 9;

const TRAY_CLASS: &str = "WinOCRHostTrayWndCls";

struct TrayCtx {
    events: Sender<TrayEvent>,
    nid: NOTIFYICONDATAW,
    hwnd: HWND,
}

/// 托盘图标句柄；析构时通知后台线程退出消息泵（非阻塞）
pub struct TrayIcon {
    thread_id: u32,
    #[allow(dead_code)]
    join: Option<JoinHandle<()>>,
}

impl TrayIcon {
    pub fn new(events: Sender<TrayEvent>, first_run: bool) -> anyhow::Result<Self> {
        let (id_tx, id_rx) = channel::<u32>();
        let join = thread::spawn(move || {
            let tid = unsafe { GetCurrentThreadId() };
            let _ = id_tx.send(tid);
            if let Err(e) = unsafe { tray_thread(events, first_run) } {
                log::error!("托盘线程异常: {}", e);
            }
        });
        let thread_id = id_rx.recv().context("托盘线程启动失败")?;
        Ok(Self {
            thread_id,
            join: Some(join),
        })
    }
}

impl Drop for TrayIcon {
    fn drop(&mut self) {
        // 非阻塞：让后台线程的 GetMessage 收到 WM_QUIT 自然退出；进程退出时线程随之结束
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
    }
}

unsafe fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
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
                let h = LoadImageW(
                    ptr::null_mut(),
                    w.as_ptr(),
                    IMAGE_ICON as u32,
                    0,
                    0,
                    LR_LOADFROMFILE | LR_DEFAULTSIZE,
                ) as HICON;
                if !h.is_null() {
                    return h;
                }
            }
        }
    }
    LoadIconW(
        GetModuleHandleW(ptr::null_mut()),
        IDI_APPLICATION as *const u16,
    )
}

unsafe fn tray_thread(events: Sender<TrayEvent>, first_run: bool) -> anyhow::Result<()> {
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
        lpszMenuName: 0 as *const u16,
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
    let tip = to_wide("WinOCR-Html");
    ptr::copy_nonoverlapping(tip.as_ptr(), nid.szTip.as_mut_ptr(), tip.len().min(128));
    Shell_NotifyIconW(NIM_ADD, &mut nid);

    if first_run {
        nid.uFlags |= NIF_INFO;
        let info = to_wide(
            "欢迎使用 WinOCR-Html！右键任务栏图标即可截图翻译、打开设置、运行诊断或退出。",
        );
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
        *nid.u.uTimeout_mut() = 15000;
        nid.dwInfoFlags = NIIF_INFO;
        Shell_NotifyIconW(NIM_MODIFY, &mut nid);
    }

    // 将上下文存入窗口 USERDATA，供窗口过程取用
    let ctx = Box::new(TrayCtx { events, nid, hwnd });
    let ctx_ptr = Box::into_raw(ctx);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);

    let mut msg: MSG = mem::zeroed();
    while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }

    // 退出：删除图标并释放上下文
    let mut ctx = Box::from_raw(ctx_ptr);
    Shell_NotifyIconW(NIM_DELETE, &mut ctx.nid);
    Ok(())
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
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
            let id = (wparam as u16) as usize;
            let ctx = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayCtx);
            let ev = match id {
                IDM_CAPTURE => Some(TrayEvent::Capture),
                IDM_SETTINGS => Some(TrayEvent::Settings),
                IDM_PAUSE => Some(TrayEvent::Pause),
                IDM_SELFCHECK => Some(TrayEvent::SelfCheck),
                IDM_HISTORY => Some(TrayEvent::History),
                IDM_VOCAB => Some(TrayEvent::Vocab),
                IDM_REVIEW => Some(TrayEvent::Review),
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
    AppendMenuW(
        menu,
        MF_STRING,
        IDM_CAPTURE,
        to_wide("截图翻译(&S)").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        IDM_SETTINGS,
        to_wide("打开设置(&O)").as_ptr(),
    );
    AppendMenuW(
        menu,
        MF_STRING,
        IDM_PAUSE,
        to_wide("暂停/恢复(&P)").as_ptr(),
    );
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
    AppendMenuW(
        menu,
        MF_STRING,
        IDM_SELFCHECK,
        to_wide("运行诊断(&D)").as_ptr(),
    );
    // §17.6 历史 / 生词本 / 复习入口
    let hk = crate::i18n::tr("history");
    let vcb = crate::i18n::tr("vocab");
    let rev = crate::i18n::tr("review");
    AppendMenuW(menu, MF_STRING, IDM_HISTORY, to_wide(&hk).as_ptr());
    AppendMenuW(menu, MF_STRING, IDM_VOCAB, to_wide(&vcb).as_ptr());
    AppendMenuW(menu, MF_STRING, IDM_REVIEW, to_wide(&rev).as_ptr());
    AppendMenuW(menu, MF_STRING, IDM_OPENMAIN, to_wide("打开主窗口(&M)").as_ptr());
    AppendMenuW(menu, MF_STRING, IDM_QUIT, to_wide("退出(&X)").as_ptr());

    let mut pt: POINT = mem::zeroed();
    GetCursorPos(&mut pt);
    // 弹出前先抢前台（否则菜单不自动消失），弹完后 PostMessage WM_NULL 收尾
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
