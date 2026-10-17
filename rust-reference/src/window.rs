//! Overlay Window Module (WinAPI Layered Window + GDI Drawing)
//! Displays original text + translation near the screenshot region

use crate::hotkey::Region;
use crate::tr;
use anyhow::Result;
use std::collections::HashMap;
use std::mem;
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use winapi::shared::windef::{HWND, HFONT, RECT, POINT};
use winapi::um::wingdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, Rectangle, SelectObject, SetBkMode,
    SetTextColor, TextOutW, FW_NORMAL, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
    CLEARTYPE_QUALITY, TRANSPARENT, RGB, WHITE_BRUSH,
};
use winapi::um::winbase::{GlobalAlloc, GlobalLock, GlobalUnlock};
use winapi::um::libloaderapi::GetModuleHandleW;
use winapi::um::winbase::GMEM_MOVEABLE;
use serde_json::Value;
use winapi::um::winuser::{
    BeginPaint, CallWindowProcW, CloseClipboard, CreateWindowExW, DefWindowProcW, DestroyWindow,
    EmptyClipboard, EndPaint,     GetClientRect, GetCursorPos, GetParent, GetWindowLongPtrW,
    GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetForegroundWindow, MonitorFromWindow,
    GetMonitorInfoW, LoadCursorW, OpenClipboard, RegisterClassW,
    GetClipboardData, SetClipboardData, SetLayeredWindowAttributes, SetWindowPos, SetCapture,
    SetWindowTextW,
    SetFocus, ReleaseCapture, ShowWindow, UpdateWindow, MessageBoxW,
    CF_UNICODETEXT, CS_HREDRAW, CS_VREDRAW, CS_DBLCLKS, GWLP_USERDATA, GWLP_WNDPROC,
    IDC_ARROW, InvalidateRect, LWA_ALPHA, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetTimer, KillTimer, SetWindowLongPtrW, VK_RETURN, WM_COMMAND,
    WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_MOUSEMOVE, WM_PAINT,
    WM_TIMER, WM_KILLFOCUS, WS_BORDER, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE, ES_AUTOHSCROLL, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};

// ---------------------------------------------------------------------------
// 译文浮窗 / 消息浮窗（§17.5 可交互，P1-4）
// 译文浮窗带工具栏：复制 / 重译 / 固定 / 关闭；不抢焦点（WS_EX_NOACTIVATE）。
// 每条浮窗的状态存于 GWLP_USERDATA，由 window_proc 自行处理点击与绘制。
// ---------------------------------------------------------------------------

/// 浮窗状态（存于窗口 GWLP_USERDATA）
struct OverlayState {
    original: String,
    translation: String,
    region: Region,
    /// true=译文浮窗（带工具栏）；false=纯消息浮窗（点击关闭）
    interactive: bool,
    /// 固定后不被下一次捕获自动隐藏，且 Esc/外部点击不关闭
    pinned: bool,
    /// 「重译」按钮回传主循环的发送端
    retransmit: Option<Sender<RetranslateRequest>>,
    /// 主题（light/dark/auto），§17.3 外观分区
    theme: String,
    /// 字号档位（small/standard/large），§17.3 外观分区
    font_scale: String,
    /// 不透明度 0-255，§17.3 外观分区
    opacity: u8,
    /// 是否记忆上次位置，§17.5 位置记忆
    allow_pos_mem: bool,
    /// 已收藏（生词本），§17.5 收藏生词
    favorited: bool,
    /// 当前「换引擎」循环到的引擎（§17.5 换引擎重译）
    engine_cursor: usize,
    /// 正在拖动（标题栏）
    drag: bool,
    drag_off_x: i32,
    drag_off_y: i32,
    /// 正在缩放（右下角手柄）
    resizing: bool,
    /// 双击编辑时挂载的子 EDIT 句柄（§17.5 双击编辑）
    edit_hwnd: HWND,
}

/// 重译请求：浮窗「重译」-> 主循环用缓存原文重新翻译并刷新
pub struct RetranslateRequest {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub original: String,
    /// 指定引擎（换引擎重译；None 用配置默认），§17.5
    pub engine: Option<String>,
    /// 双击编辑后的自定义原文（优先于 original），§17.5
    pub edited_text: Option<String>,
}

/// 可切换的翻译引擎清单（换引擎重译时循环使用），§17.5
const SWITCH_ENGINES: &[&str] = &["sf", "mymemory", "ollama", "local"];

const TOOLBAR_H: i32 = 34;
const BTN_W: i32 = 56;
const BTN_H: i32 = 24;
const BTN_GAP: i32 = 5;
const BTN_MARGIN: i32 = 8;
/// 右下角缩放手柄尺寸（§17.5 可拖动缩放）
const RESIZE_HANDLE: i32 = 16;

#[derive(Clone, Copy)]
enum ToolBtn {
    Copy,
    Retranslate,
    Pin,
    Close,
    Favorite,
    SwitchEngine,
    Read,
    Replace,
}

/// 浮窗管理器（译文浮窗 + 纯消息浮窗）
pub struct OverlayWindow {
    /// 最近一次创建的译文浮窗（用于隐藏未固定的前一条）
    hwnd: Option<HWND>,
    /// 全屏独占时隐藏（§17.5 全屏隐藏）；恢复时据此重建
    fullscreen_hidden: bool,
    /// 最近一次展示参数（用于全屏恢复重建）
    last: Option<LastOverlay>,
}

/// 缓存最近一次译文浮窗参数，供退出全屏后恢复（§17.5）
#[derive(Clone)]
struct LastOverlay {
    region: Region,
    original: String,
    translation: String,
    theme: String,
    font_scale: String,
    opacity: u8,
    allow_pos_mem: bool,
    retransmit: Option<Sender<RetranslateRequest>>,
}

impl OverlayWindow {
    pub fn new(_config: &crate::config::Config) -> Result<Self> {
        Ok(Self {
            hwnd: None,
            fullscreen_hidden: false,
            last: None,
        })
    }

    fn destroy_prev_if_unpinned(&mut self) {
        if let Some(hwnd) = self.hwnd {
            let pinned = unsafe { overlay_pinned(hwnd) };
            if !pinned {
                unsafe { DestroyWindow(hwnd); }
            }
        }
    }

    /// 显示可交互译文浮窗（工具栏：复制 / 重译 / 换引擎 / 收藏 / 固定 / 关闭）
    pub fn show_translation(
        &mut self,
        region: &Region,
        original: &str,
        translation: &str,
        retransmit: &Sender<RetranslateRequest>,
        theme: &str,
        font_scale: &str,
        opacity: u8,
        allow_pos_mem: bool,
    ) {
        self.destroy_prev_if_unpinned();

        let screen_w = unsafe { winapi::um::winuser::GetSystemMetrics(winapi::um::winuser::SM_CXSCREEN) };
        let screen_h = unsafe { winapi::um::winuser::GetSystemMetrics(winapi::um::winuser::SM_CYSCREEN) };

        let w = 560i32;
        let content_h = calculate_text_height(original, translation).max(80).min(300);
        let h = TOOLBAR_H + content_h + 12;

        // 位置：优先使用记忆位置（§17.5 位置记忆）
        let mut x = region.x;
        let mut y = region.y + region.h + 10;
        if allow_pos_mem {
            if let Some((mx, my, mw, mh)) = load_float_pos() {
                if mw > 0 && mh > 0 && mx + mw <= screen_w && my + mh <= screen_h && mx >= 0 && my >= 0 {
                    x = mx;
                    y = my;
                }
            }
        }
        if y + h > screen_h {
            y = (region.y - h - 10).max(10);
        }
        if x + w > screen_w {
            x = screen_w - w - 10;
        }
        if x < 0 {
            x = 10;
        }

        let state = Box::new(OverlayState {
            original: original.to_string(),
            translation: translation.to_string(),
            region: Region {
                x: region.x,
                y: region.y,
                w: region.w,
                h: region.h,
            },
            interactive: true,
            pinned: false,
            retransmit: Some(retransmit.clone()),
            theme: theme.to_string(),
            font_scale: font_scale.to_string(),
            opacity,
            allow_pos_mem,
            favorited: false,
            engine_cursor: 0,
            drag: false,
            drag_off_x: 0,
            drag_off_y: 0,
            resizing: false,
            edit_hwnd: ptr::null_mut(),
        });
        let hwnd = create_overlay_window(x, y, w, h, state);
        self.hwnd = Some(hwnd);

        // 缓存最近一次参数，供全屏恢复重建（§17.5）
        self.last = Some(LastOverlay {
            region: Region { x: region.x, y: region.y, w: region.w, h: region.h },
            original: original.to_string(),
            translation: translation.to_string(),
            theme: theme.to_string(),
            font_scale: font_scale.to_string(),
            opacity,
            allow_pos_mem,
            retransmit: Some(retransmit.clone()),
        });
    }

    /// 纯消息浮窗（点击关闭，无工具栏）
    pub fn show_message(&mut self, x: i32, y: i32, message: &str) {
        self.destroy_prev_if_unpinned();
        let w = 400i32;
        let h = calculate_text_height(message, "").max(80).min(200) + 16;
        let state = Box::new(OverlayState {
            original: String::new(),
            translation: message.to_string(),
            region: Region { x, y, w, h },
            interactive: false,
            pinned: false,
            retransmit: None,
            theme: "light".to_string(),
            font_scale: "standard".to_string(),
            opacity: 235,
            allow_pos_mem: false,
            favorited: false,
            engine_cursor: 0,
            drag: false,
            drag_off_x: 0,
            drag_off_y: 0,
            resizing: false,
            edit_hwnd: ptr::null_mut(),
        });
        let hwnd = create_overlay_window(x, y, w, h, state);
        self.hwnd = Some(hwnd);
    }

    /// Close the most recent translation window
    pub fn hide(&mut self) {
        if let Some(hwnd) = self.hwnd {
            unsafe {
                DestroyWindow(hwnd);
            }
            self.hwnd = None;
        }
        self.last = None;
    }

    /// 全屏独占时临时隐藏浮窗（§17.5）。退出全屏后调用本函数 false 恢复。
    pub fn set_fullscreen_hidden(&mut self, hidden: bool) {
        if hidden {
            if !self.fullscreen_hidden {
                if let Some(hwnd) = self.hwnd {
                    unsafe { DestroyWindow(hwnd); }
                    self.hwnd = None;
                }
                self.fullscreen_hidden = true;
            }
        } else if self.fullscreen_hidden {
            self.fullscreen_hidden = false;
            // 恢复：用缓存参数重建
            if let Some(l) = self.last.clone() {
                if let Some(tx) = l.retransmit.clone() {
                    self.show_translation(
                        &l.region,
                        &l.original,
                        &l.translation,
                        &tx,
                        &l.theme,
                        &l.font_scale,
                        l.opacity,
                        l.allow_pos_mem,
                    );
                }
            }
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        self.hide();
    }
}

/// Calculate required text height
fn calculate_text_height(text1: &str, text2: &str) -> i32 {
    let lines = if text2.is_empty() {
        text1.lines().count().max(1)
    } else {
        text1.lines().count() + text2.lines().count()
    };
    (lines as i32 * 20) + 60
}

/// Create layered overlay window (non-blocking, no focus steal)
fn create_overlay_window(x: i32, y: i32, w: i32, h: i32, state: Box<OverlayState>) -> HWND {
    unsafe {
        let class_name = encode_wide("WinOCROverlayWindow");
        let wnd_class = winapi::um::winuser::WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            hInstance: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: GetStockObject(WHITE_BRUSH as i32) as *mut _,
            lpszClassName: class_name.as_ptr(),
            lpszMenuName: ptr::null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hIcon: ptr::null_mut(),
        };

        RegisterClassW(&wnd_class);

        // WS_EX_NOACTIVATE: 不抢焦点（满足 ADR-005）
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            class_name.as_ptr(),
            ptr::null(),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            w,
            h,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );

        if hwnd.is_null() {
            return ptr::null_mut();
        }

        let op = state.opacity;
        SetLayeredWindowAttributes(hwnd, RGB(0, 0, 0), op, LWA_ALPHA);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        // UIA（§17.5 无障碍）：以译文作为窗口标题，供屏幕阅读器读取
        let title = {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if p != 0 {
                (*(p as *const OverlayState)).translation.clone()
            } else {
                String::new()
            }
        };
        if !title.is_empty() {
            SetWindowTextW(hwnd, encode_wide(&title).as_ptr());
        }
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        hwnd
    }
}

/// 工具栏按钮的 x 起点（从右到左：关闭 / 固定 / 收藏 / 换引擎 / 重译 / 复制）
unsafe fn toolbar_rects(w: i32) -> [(i32, ToolBtn); 8] {
    let close_x = w - BTN_MARGIN - BTN_W;
    let pin_x = close_x - BTN_GAP - BTN_W;
    let fav_x = pin_x - BTN_GAP - BTN_W;
    let sw_x = fav_x - BTN_GAP - BTN_W;
    let retr_x = sw_x - BTN_GAP - BTN_W;
    let copy_x = retr_x - BTN_GAP - BTN_W;
    let repl_x = copy_x - BTN_GAP - BTN_W;
    let read_x = repl_x - BTN_GAP - BTN_W;
    [
        (read_x, ToolBtn::Read),
        (repl_x, ToolBtn::Replace),
        (copy_x, ToolBtn::Copy),
        (retr_x, ToolBtn::Retranslate),
        (sw_x, ToolBtn::SwitchEngine),
        (fav_x, ToolBtn::Favorite),
        (pin_x, ToolBtn::Pin),
        (close_x, ToolBtn::Close),
    ]
}

unsafe fn with_overlay_state<F, R>(hwnd: HWND, f: F) -> R
where
    F: FnOnce(&OverlayState) -> R,
{
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if p == 0 {
        // 无状态时的安全兜底（不应发生）
        f(&OverlayState {
            original: String::new(),
            translation: String::new(),
            region: Region { x: 0, y: 0, w: 0, h: 0 },
            interactive: false,
            pinned: false,
            retransmit: None,
            theme: "light".to_string(),
            font_scale: "standard".to_string(),
            opacity: 235,
            allow_pos_mem: false,
            favorited: false,
            engine_cursor: 0,
            drag: false,
            drag_off_x: 0,
            drag_off_y: 0,
            resizing: false,
            edit_hwnd: ptr::null_mut(),
        })
    } else {
        f(&*(p as *const OverlayState))
    }
}

unsafe fn overlay_pinned(hwnd: HWND) -> bool {
    with_overlay_state(hwnd, |s| s.pinned)
}

/// 浮窗位置记忆文件路径（exe 同目录，§17.5 位置记忆）
fn float_pos_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("last_float_pos.json"))
}

/// 读取上次记忆的浮窗位置与尺寸
fn load_float_pos() -> Option<(i32, i32, i32, i32)> {
    let p = float_pos_path()?;
    let s = std::fs::read_to_string(&p).ok()?;
    let v: Value = serde_json::from_str(&s).ok()?;
    Some((
        v.get("x")?.as_i64()? as i32,
        v.get("y")?.as_i64()? as i32,
        v.get("w")?.as_i64()? as i32,
        v.get("h")?.as_i64()? as i32,
    ))
}

/// 写入浮窗位置与尺寸（拖动/缩放结束后调用）
fn save_float_pos(x: i32, y: i32, w: i32, h: i32) {
    if let Some(p) = float_pos_path() {
        let _ = std::fs::write(
            &p,
            format!("{{\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}", x, y, w, h),
        );
    }
}

/// 命中工具栏按钮则返回对应动作（仅 interactive 浮窗）
unsafe fn hit_test_toolbar(hwnd: HWND, x: i32, y: i32) -> Option<ToolBtn> {
    if !with_overlay_state(hwnd, |s| s.interactive) {
        return None;
    }
    if y < BTN_MARGIN || y > BTN_MARGIN + BTN_H {
        return None;
    }
    let mut rect: winapi::shared::windef::RECT = mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let w = rect.right - rect.left;
    for (bx, btn) in toolbar_rects(w).iter() {
        if x >= *bx && x <= *bx + BTN_W {
            return Some(*btn);
        }
    }
    None
}

/// 复制译文（译文为空则复制原文）到剪贴板
unsafe fn copy_translation(hwnd: HWND) {
    with_overlay_state(hwnd, |s| {
        let text = if s.translation.is_empty() {
            s.original.clone()
        } else {
            s.translation.clone()
        };
        if !text.is_empty() {
            copy_to_clipboard(&text);
            show_toast("已复制到剪贴板", ToastLevel::Info);
        }
    });
}

/// 朗读译文（系统语音合成，零模型依赖；桌面浮层不收键盘焦点，故用按钮触发）
unsafe fn speak_translation(hwnd: HWND) {
    with_overlay_state(hwnd, |s| {
        let text = if s.translation.is_empty() {
            s.original.clone()
        } else {
            s.translation.clone()
        };
        if !text.is_empty() {
            speak(&text);
            show_toast("正在朗读…", ToastLevel::Info);
        }
    });
}

/// 调用系统语音合成朗读（PowerShell System.Speech，.NET 自带，无需额外模型）。
/// 注：中文需系统安装中文 voice；若无则用默认 voice。后续可换 Windows.Media.SpeechSynthesis。
fn speak(text: &str) {
    let first_line = text.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return;
    }
    // PowerShell 单引号串内用 '' 转义单引号
    let escaped = first_line.replace('\'', "''");
    let ps = format!(
        "Add-Type -AssemblyName System.speech; (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak('{}')",
        escaped
    );
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
        .spawn();
}

/// 公开包装：朗读任意文本（供 S12 Alt+R 全局热键 / 主窗口 🔊 按钮调用）
pub fn speak_text(text: &str) {
    speak(text);
}

/// 公开包装：复制文本到剪贴板（供 S12 Alt+C / 主窗口复制按钮调用）
pub fn copy_text(text: &str) {
    unsafe {
        copy_to_clipboard(text);
    }
}

/// 读取系统剪贴板文本（CF_UNICODETEXT），无文本返回 None（供 粘贴翻译 / Alt+T 调用）
pub fn read_clipboard() -> Option<String> {
    unsafe {
        if OpenClipboard(ptr::null_mut()) == 0 {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT);
        let text = if h.is_null() {
            None
        } else {
            let p = GlobalLock(h) as *const u16;
            if p.is_null() {
                None
            } else {
                let mut len = 0usize;
                while *p.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(p, len);
                let s = String::from_utf16_lossy(slice);
                GlobalUnlock(h);
                Some(s)
            }
        };
        CloseClipboard();
        text
    }
}

/// 「重译」：把缓存原文发回主循环重新翻译
unsafe fn send_retransmit(hwnd: HWND) {
    with_overlay_state(hwnd, |s| {
        if let Some(tx) = &s.retransmit {
            let r = s.region;
            let _ = tx.send(RetranslateRequest {
                x: r.x,
                y: r.y,
                w: r.w,
                h: r.h,
                original: s.original.clone(),
                engine: None,
                edited_text: None,
            });
            show_toast(tr!("retranslating"), ToastLevel::Info);
        }
    });
}

unsafe fn toggle_pin(hwnd: HWND) {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if p != 0 {
        let s = &mut *(p as *mut OverlayState);
        s.pinned = !s.pinned;
    }
    InvalidateRect(hwnd, ptr::null_mut(), 1);
}

/// 「换引擎」：循环到下一个可用引擎并重译（§17.5 换引擎重译）
unsafe fn switch_engine_retransmit(hwnd: HWND) {
    let mut cur = 0usize;
    let mut region = Region { x: 0, y: 0, w: 0, h: 0 };
    let mut original = String::new();
    let mut tx = None;
    with_overlay_state(hwnd, |s| {
        cur = s.engine_cursor;
        region = s.region;
        original = s.original.clone();
        tx = s.retransmit.clone();
    });
    if let Some(tx) = tx {
        let next = (cur + 1) % SWITCH_ENGINES.len();
        let engine = SWITCH_ENGINES[next];
        let _ = tx.send(RetranslateRequest {
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
            original: original.clone(),
            engine: Some(engine.to_string()),
            edited_text: None,
        });
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if p != 0 {
            (*(p as *mut OverlayState)).engine_cursor = next;
        }
        show_toast(&format!("已用 {} 引擎重译", engine), ToastLevel::Info);
    }
}

/// 「收藏」：把原文+译文加入生词本（§17.5 收藏生词）
unsafe fn favorite_translation(hwnd: HWND) {
    let mut original = String::new();
    let mut translation = String::new();
    with_overlay_state(hwnd, |s| {
        original = s.original.clone();
        translation = s.translation.clone();
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if p != 0 {
            (*(p as *mut OverlayState)).favorited = true;
        }
    });
    if !original.trim().is_empty() {
        crate::history::add_vocab(&original, &translation);
        show_toast(tr!("favorited"), ToastLevel::Info);
        InvalidateRect(hwnd, ptr::null_mut(), 1);
    }
}

/// 双击编辑：挂载子 EDIT 编辑原文，回车后按编辑内容重译（§17.5 双击编辑）
unsafe fn start_inline_edit(hwnd: HWND) {
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if p == 0 {
        return;
    }
    let s = &mut *(p as *mut OverlayState);
    if !s.edit_hwnd.is_null() {
        return;
    }
    let mut crect: RECT = mem::zeroed();
    GetClientRect(hwnd, &mut crect);
    let edit = CreateWindowExW(
        0,
        encode_wide("EDIT").as_ptr(),
        ptr::null(),
        WS_CHILD | WS_VISIBLE | WS_BORDER | ES_AUTOHSCROLL as u32,
        12,
        TOOLBAR_H + 8,
        crect.right - 24,
        24,
        hwnd,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if edit.is_null() {
        return;
    }
    SetWindowTextW(edit, encode_wide(&s.original).as_ptr());
    // 子类化：保存原 WNDPROC，替换为 edit_proc 捕获回车
    let old = GetWindowLongPtrW(edit, GWLP_WNDPROC);
    SetWindowLongPtrW(edit, GWLP_USERDATA, old);
    SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as isize);
    s.edit_hwnd = edit;
    SetFocus(edit);
    InvalidateRect(hwnd, ptr::null_mut(), 1);
}

/// 子 EDIT 的子类过程：回车提交编辑后重译，失焦取消（§17.5 双击编辑）
unsafe extern "system" fn edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    if msg == WM_KEYDOWN && wparam == VK_RETURN as usize {
        let parent = GetParent(hwnd);
        let len = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; (len + 1) as usize];
        GetWindowTextW(hwnd, buf.as_mut_ptr(), len + 1);
        let text = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        with_overlay_state(parent, |s| {
            if let Some(tx) = &s.retransmit {
                let r = s.region;
                let _ = tx.send(RetranslateRequest {
                    x: r.x,
                    y: r.y,
                    w: r.w,
                    h: r.h,
                    original: s.original.clone(),
                    engine: None,
                    edited_text: Some(text.clone()),
                });
            }
        });
        show_toast(tr!("retranslating"), ToastLevel::Info);
        DestroyWindow(hwnd);
        let p = GetWindowLongPtrW(parent, GWLP_USERDATA);
        if p != 0 {
            (*(p as *mut OverlayState)).edit_hwnd = ptr::null_mut();
        }
        ShowWindow(parent, SW_SHOW);
        return 0;
    } else if msg == WM_KILLFOCUS {
        let parent = GetParent(hwnd);
        DestroyWindow(hwnd);
        let p = GetWindowLongPtrW(parent, GWLP_USERDATA);
        if p != 0 {
            (*(p as *mut OverlayState)).edit_hwnd = ptr::null_mut();
        }
        let old = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if old != 0 {
            let prev: winapi::um::winuser::WNDPROC = std::mem::transmute(old);
        return CallWindowProcW(prev, hwnd, msg, wparam, lparam);
        }
        return 0;
    }
    let old = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if old != 0 {
        let prev: winapi::um::winuser::WNDPROC = std::mem::transmute(old);
        return CallWindowProcW(prev, hwnd, msg, wparam, lparam);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// 将文本以 CF_UNICODETEXT 写入系统剪贴板
unsafe fn copy_to_clipboard(text: &str) {
    if OpenClipboard(ptr::null_mut()) == 0 {
        return;
    }
    EmptyClipboard();
    let wide = encode_wide(text); // 已含结尾 NUL
    let size = wide.len() * 2;
    let h = GlobalAlloc(GMEM_MOVEABLE, size as usize);
    if !h.is_null() {
        let p = GlobalLock(h) as *mut u16;
        if !p.is_null() {
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            GlobalUnlock(h);
            SetClipboardData(CF_UNICODETEXT, h);
        }
    }
    CloseClipboard();
}

fn make_font(size: i32, bold: bool) -> HFONT {
    unsafe {
        CreateFontW(
            size,
            0,
            0,
            0,
            if bold { FW_NORMAL + 300 } else { FW_NORMAL },
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            0,
            encode_wide("Microsoft YaHei").as_ptr(),
        )
    }
}

unsafe fn draw_button(hdc: winapi::shared::windef::HDC, x: i32, y: i32, w: i32, h: i32, label: &str) {
    let brush = CreateSolidBrush(RGB(58, 64, 74));
    let old = SelectObject(hdc, brush as *mut _);
    Rectangle(hdc, x, y, x + w, y + h);
    SelectObject(hdc, old);
    DeleteObject(brush as *mut _);

    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, RGB(220, 225, 235));
    let font = make_font(13, false);
    let old = SelectObject(hdc, font as *mut _);
    let lw = encode_wide(label);
    TextOutW(hdc, x + 8, y + 4, lw.as_ptr(), (lw.len() as i32 - 1).max(0));
    SelectObject(hdc, old);
    DeleteObject(font as *mut _);
}

/// 绘制浮窗（工具栏 + 原文/译文 或 纯消息）
unsafe fn draw_overlay(hwnd: HWND, hdc: winapi::shared::windef::HDC) {
    let mut rect = mem::zeroed::<winapi::shared::windef::RECT>();
    GetClientRect(hwnd, &mut rect);
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;

    with_overlay_state(hwnd, |state| {
        // 配色随主题（§17.3 外观分区；auto 视作深色浮窗）
        let dark = state.theme != "light";
        let (bg, tb, title_c, orig_lbl, orig_txt, trans_lbl, trans_txt, msg_txt) = if dark {
            (
                RGB(28, 28, 32),
                RGB(40, 44, 52),
                RGB(120, 180, 255),
                RGB(160, 160, 170),
                RGB(230, 230, 235),
                RGB(120, 200, 255),
                RGB(255, 255, 255),
                RGB(255, 255, 255),
            )
        } else {
            (
                RGB(245, 245, 248),
                RGB(228, 231, 238),
                RGB(20, 110, 210),
                RGB(90, 90, 100),
                RGB(40, 40, 45),
                RGB(20, 110, 210),
                RGB(20, 20, 25),
                RGB(20, 20, 25),
            )
        };
        let fs_base: i32 = match state.font_scale.as_str() {
            "small" => 13,
            "large" => 18,
            _ => 15,
        };

        // 背景
        let b = CreateSolidBrush(bg);
        let old = SelectObject(hdc, b as *mut _);
        Rectangle(hdc, 0, 0, w, h);
        SelectObject(hdc, old);
        DeleteObject(b as *mut _);

        if state.interactive {
            // 工具栏底
            let tb_brush = CreateSolidBrush(tb);
            let old = SelectObject(hdc, tb_brush as *mut _);
            Rectangle(hdc, 0, 0, w, TOOLBAR_H);
            SelectObject(hdc, old);
            DeleteObject(tb_brush as *mut _);

            // 标题
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, title_c);
            let font = make_font(fs_base - 1, false);
            let old = SelectObject(hdc, font as *mut _);
            let t = encode_wide("WinOCR 译文");
            TextOutW(hdc, 10, 8, t.as_ptr(), (t.len() as i32 - 1).max(0));
            SelectObject(hdc, old);
            DeleteObject(font as *mut _);

            // 按钮
            for (bx, btn) in toolbar_rects(w).iter() {
                let label = match btn {
                    ToolBtn::Copy => tr!("copy"),
                    ToolBtn::Retranslate => tr!("retransmit"),
                    ToolBtn::SwitchEngine => tr!("switch_engine"),
                    ToolBtn::Favorite => {
                        if state.favorited {
                            tr!("favorited")
                        } else {
                            tr!("favorite")
                        }
                    }
                    ToolBtn::Pin => {
                        if state.pinned {
                            tr!("pinned")
                        } else {
                            tr!("pin")
                        }
                    }
                    ToolBtn::Read => "朗读",
                    ToolBtn::Replace => "替换",
                    ToolBtn::Close => tr!("close"),
                };
                draw_button(hdc, *bx, BTN_MARGIN, BTN_W, BTN_H, label);
            }
        }

        // 内容
        SetBkMode(hdc, TRANSPARENT as i32);
        let font = make_font(fs_base, false);
        let old = SelectObject(hdc, font as *mut _);
        let mut y = if state.interactive { TOOLBAR_H + 8 } else { 10 };

        if state.interactive {
            SetTextColor(hdc, orig_lbl);
            let ot = encode_wide("原文：");
            TextOutW(hdc, 12, y, ot.as_ptr(), (ot.len() as i32 - 1).max(0));
            y += 22;
            SetTextColor(hdc, orig_txt);
            for line in state.original.lines().take(8) {
                let lw = encode_wide(line);
                if !lw.is_empty() {
                    TextOutW(hdc, 12, y, lw.as_ptr(), (lw.len() as i32 - 1).max(0));
                }
                y += 20;
            }
            y += 6;
            SetTextColor(hdc, trans_lbl);
            let tt = encode_wide("译文：");
            TextOutW(hdc, 12, y, tt.as_ptr(), (tt.len() as i32 - 1).max(0));
            y += 22;
            SetTextColor(hdc, trans_txt);
            for line in state.translation.lines().take(12) {
                let lw = encode_wide(line);
                if !lw.is_empty() {
                    TextOutW(hdc, 12, y, lw.as_ptr(), (lw.len() as i32 - 1).max(0));
                }
                y += 20;
            }
        } else {
            SetTextColor(hdc, msg_txt);
            for line in state.translation.lines().take(10) {
                let lw = encode_wide(line);
                if !lw.is_empty() {
                    TextOutW(hdc, 12, y, lw.as_ptr(), (lw.len() as i32 - 1).max(0));
                }
                y += 20;
            }
        }

        SelectObject(hdc, old);
        DeleteObject(font as *mut _);
    });

    // 缩放手柄提示（右下角，§17.5 可拖动缩放）
    if with_overlay_state(hwnd, |s| s.interactive) {
        let rh = RESIZE_HANDLE;
        let b = CreateSolidBrush(RGB(120, 130, 150));
        let old = SelectObject(hdc, b as *mut _);
        Rectangle(hdc, w - rh, h - rh, w, h);
        SelectObject(hdc, old);
        DeleteObject(b as *mut _);
    }
}

/// Window procedure
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps: winapi::um::winuser::PAINTSTRUCT = mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            draw_overlay(hwnd, hdc);
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xffff) as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i16 as i32;
            match hit_test_toolbar(hwnd, x, y) {
                Some(ToolBtn::Read) => speak_translation(hwnd),
                Some(ToolBtn::Replace) => copy_translation(hwnd), // 桌面浮层降级为复制（真正替换在扩展侧 content script）
                Some(ToolBtn::Copy) => copy_translation(hwnd),
                Some(ToolBtn::Retranslate) => send_retransmit(hwnd),
                Some(ToolBtn::SwitchEngine) => switch_engine_retransmit(hwnd),
                Some(ToolBtn::Favorite) => favorite_translation(hwnd),
                Some(ToolBtn::Pin) => toggle_pin(hwnd),
                Some(ToolBtn::Close) => {
                    DestroyWindow(hwnd);
                }
                None => {
                    // 标题栏拖动 / 右下角缩放手柄（§17.5）
                    if with_overlay_state(hwnd, |s| s.interactive) {
                        let mut crect: RECT = mem::zeroed();
                        GetClientRect(hwnd, &mut crect);
                        let cw = crect.right - crect.left;
                        let ch = crect.bottom - crect.top;
                        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                        if p != 0 {
                            let s = &mut *(p as *mut OverlayState);
                            if x >= cw - RESIZE_HANDLE && y >= ch - RESIZE_HANDLE {
                                s.resizing = true;
                                SetCapture(hwnd);
                            } else if y <= TOOLBAR_H {
                                let mut wr: RECT = mem::zeroed();
                                GetWindowRect(hwnd, &mut wr);
                                let mut cp: POINT = mem::zeroed();
                                GetCursorPos(&mut cp);
                                s.drag = true;
                                s.drag_off_x = cp.x - wr.left;
                                s.drag_off_y = cp.y - wr.top;
                                SetCapture(hwnd);
                            }
                        }
                    }
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            // 双击内容区进入「编辑原文重译」（§17.5 双击编辑）
            let x = (lparam & 0xffff) as i16 as i32;
            let y = ((lparam >> 16) & 0xffff) as i16 as i32;
            if with_overlay_state(hwnd, |s| s.interactive) && y > TOOLBAR_H {
                let mut crect: RECT = mem::zeroed();
                GetClientRect(hwnd, &mut crect);
                if x > 8 && x < crect.right - 8 && y < crect.bottom - RESIZE_HANDLE {
                    start_inline_edit(hwnd);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if p != 0 {
                let s = &mut *(p as *mut OverlayState);
                if s.drag {
                    let mut cp: POINT = mem::zeroed();
                    if GetCursorPos(&mut cp) != 0 {
                        SetWindowPos(
                            hwnd,
                            ptr::null_mut(),
                            cp.x - s.drag_off_x,
                            cp.y - s.drag_off_y,
                            0,
                            0,
                            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE,
                        );
                    }
                } else if s.resizing {
                    let mut cp: POINT = mem::zeroed();
                    if GetCursorPos(&mut cp) != 0 {
                        let mut wr: RECT = mem::zeroed();
                        GetWindowRect(hwnd, &mut wr);
                        let nw = (cp.x - wr.left).max(320);
                        let nh = (cp.y - wr.top).max(120);
                        SetWindowPos(
                            hwnd,
                            ptr::null_mut(),
                            0,
                            0,
                            nw,
                            nh,
                            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
                        );
                        InvalidateRect(hwnd, ptr::null_mut(), 1);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if p != 0 {
                let s = &mut *(p as *mut OverlayState);
                if s.drag || s.resizing {
                    if s.allow_pos_mem {
                        let mut wr: RECT = mem::zeroed();
                        GetWindowRect(hwnd, &mut wr);
                        save_float_pos(wr.left, wr.top, wr.right - wr.left, wr.bottom - wr.top);
                    }
                    s.drag = false;
                    s.resizing = false;
                    ReleaseCapture();
                }
            }
            0
        }
        WM_KEYDOWN => {
            // Esc 关闭（已固定的不关闭）
            if wparam == 27 && !overlay_pinned(hwnd) {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            // 注意：这里是子浮窗，销毁时不要 PostQuitMessage，
            // 否则会向主线程投递 WM_QUIT 导致整个常驻循环退出。
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if p != 0 {
                let _ = Box::from_raw(p as *mut OverlayState);
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Encode string to wide characters
fn encode_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 弹出确认对话框（是/否），返回是否选择「是」。
/// 供历史/生词本窗口「清空」等操作使用。
pub unsafe fn show_confirm(parent: HWND, msg: &str) -> bool {
    let r = MessageBoxW(
        parent,
        encode_wide(msg).as_ptr(),
        encode_wide("确认").as_ptr(),
        0x24, // MB_YESNO
    );
    r == 6 // IDYES
}

/// 检测当前是否存在全屏独占前台窗口（游戏/播放器全屏，§17.5 全屏隐藏）。
/// 启发式：前台窗口矩形覆盖整个显示器全区域（非仅工作区）即视为全屏独占。
pub fn is_fullscreen_exclusive() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        let mut wr: RECT = mem::zeroed();
        if GetWindowRect(fg, &mut wr) == 0 {
            return false;
        }
        let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST);
        if mon.is_null() {
            return false;
        }
        let mut mi: MONITORINFO = mem::zeroed();
        mi.cbSize = mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) == 0 {
            return false;
        }
        let full = mi.rcMonitor;
        let fw = wr.right - wr.left;
        let fh = wr.bottom - wr.top;
        let mw = full.right - full.left;
        let mh = full.bottom - full.top;
        fw >= mw - 4
            && fh >= mh - 4
            && wr.left <= full.left + 4
            && wr.top <= full.top + 4
    }
}

// ---------------------------------------------------------------------------
// Toast 通知（§17.7）
// 右下角瞬时提示：不抢焦点（WS_EX_NOACTIVATE）、分级配色、自动消失。
// 增强（本轮）：最多 3 个纵向堆叠、分级时长（提示 4s / 警告 8s / 错误不自动消失）、
// 同一消息 5 分钟内不重复、错误带「复制诊断 / 打开设置」操作按钮。
// 窗口由创建线程的消息泵驱动；主循环已在跑 PeekMessage/DispatchMessage。
// ---------------------------------------------------------------------------

/// 提示级别，决定配色与默认语义
#[derive(Clone, Copy, PartialEq)]
pub enum ToastLevel {
    Info,
    Warn,
    Error,
}

/// 错误 Toast 的可点击操作（§17.7 操作按钮）
#[derive(Clone, Copy, PartialEq)]
pub enum ToastAction {
    /// 复制最近一次诊断报告（脱敏文本）
    CopyDiagnostic,
    /// 打开设置窗口
    OpenSettings,
}

struct ToastData {
    text: String,
    level: ToastLevel,
    actions: Vec<ToastAction>,
    h: i32,
}

const TOAST_CLASS: &str = "WinOCRToastWindow";
const TOAST_TIMER_ID: usize = 1;
const TOAST_W: i32 = 340;
const TOAST_H: i32 = 76;
const TOAST_H_ACTIONS: i32 = 112;
const TOAST_GAP: i32 = 8;
const TOAST_MARGIN: i32 = 20;
/// 同时最多堆叠数量（§17.7）
const TOAST_MAX: usize = 3;
/// 抑制窗口：同一消息 5 分钟内不重复（§17.7 抑制）
const SUPPRESS_WINDOW: Duration = Duration::from_secs(300);

/// 活着的 Toast 句柄栈（index 0 = 最早）。用于堆叠定位与上限淘汰。
static TOAST_STACK: Mutex<Vec<isize>> = Mutex::new(Vec::new());
/// 抑制表：message -> 上次弹出时刻
static TOAST_SUPPRESS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

/// 在屏幕右下角弹出一条短时提示。便捷封装，无操作按钮。
pub fn show_toast(message: &str, level: ToastLevel) {
    show_toast_actions(message, level, &[]);
}

/// 带操作按钮的 Toast（错误级常用）。
pub fn show_toast_actions(message: &str, level: ToastLevel, actions: &[ToastAction]) {
    // 抑制：同一消息 5 分钟内不重复
    {
        let mut map = TOAST_SUPPRESS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
        let now = Instant::now();
        if let Some(last) = map.get(message) {
            if now.duration_since(*last) < SUPPRESS_WINDOW {
                return;
            }
        }
        map.insert(message.to_string(), now);
    }

    let actions = if level == ToastLevel::Error {
        // 错误默认带「复制诊断 / 打开设置」，若调用方未指定
        if actions.is_empty() {
            vec![ToastAction::CopyDiagnostic, ToastAction::OpenSettings]
        } else {
            actions.to_vec()
        }
    } else {
        actions.to_vec()
    };

    let h = if actions.is_empty() { TOAST_H } else { TOAST_H_ACTIONS };

    unsafe {
        let cls = encode_wide(TOAST_CLASS);
        let hinst = GetModuleHandleW(ptr::null_mut());
        let wc = winapi::um::winuser::WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(toast_proc),
            hInstance: hinst,
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(),
            lpszClassName: cls.as_ptr(),
            lpszMenuName: ptr::null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hIcon: ptr::null_mut(),
        };
        RegisterClassW(&wc);

        // 上限淘汰：超过 3 个则销毁最早的一个
        {
            let mut stack = TOAST_STACK.lock().unwrap();
            while stack.len() >= TOAST_MAX {
                if let Some(old) = stack.first().copied() {
                    DestroyWindow(old as HWND);
                    stack.remove(0);
                } else {
                    break;
                }
            }
        }

        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            cls.as_ptr(),
            ptr::null(),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            TOAST_W,
            h,
            ptr::null_mut(),
            ptr::null_mut(),
            hinst,
            ptr::null_mut(),
        );
        if hwnd.is_null() {
            return;
        }
        SetLayeredWindowAttributes(hwnd, RGB(0, 0, 0), 240, LWA_ALPHA);
        let data = Box::new(ToastData {
            text: message.to_string(),
            level,
            actions: actions.clone(),
            h,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);

        // 加入栈并重新排布所有活着的 Toast 位置
        {
            let mut stack = TOAST_STACK.lock().unwrap();
            stack.push(hwnd as isize);
            toast_reflow(&stack);
        }

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        // 分级自动消失：提示 4s、警告 8s、错误不自动消失（需手动关闭）
        let dur = match level {
            ToastLevel::Info => 4000,
            ToastLevel::Warn => 8000,
            ToastLevel::Error => 0,
        };
        if dur > 0 {
            SetTimer(hwnd, TOAST_TIMER_ID, dur, None);
        }
    }
}

/// 依据栈中顺序重新计算并定位每个 Toast（最新在最下，靠右下角）
unsafe fn toast_reflow(stack: &[isize]) {
    let screen_h = winapi::um::winuser::GetSystemMetrics(winapi::um::winuser::SM_CYSCREEN);
    let n = stack.len();
    for (i, &hwnd) in stack.iter().enumerate() {
        let hwnd = hwnd as HWND;
        let th = with_toast_data(hwnd, |d| d.map(|x| x.h).unwrap_or(TOAST_H));
        // i=0 最早，置于最上方；最新(n-1)贴右下角
        let from_bottom = (n - 1 - i) as i32;
        let y = screen_h - TOAST_MARGIN - th - from_bottom * (TOAST_H_ACTIONS + TOAST_GAP);
        let x = winapi::um::winuser::GetSystemMetrics(winapi::um::winuser::SM_CXSCREEN) - TOAST_W - TOAST_MARGIN;
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOSIZE,
        );
    }
}

/// 从栈中移除并销毁（WM_DESTROY 时调用）
unsafe fn toast_remove(hwnd: HWND) {
    let mut stack = TOAST_STACK.lock().unwrap();
    if let Some(pos) = stack.iter().position(|&h| h == hwnd as isize) {
        stack.remove(pos);
        toast_reflow(&stack);
    }
}

/// 计算操作按钮矩形（最多 2 个，底部等宽排列）
unsafe fn toast_action_rects(hwnd: HWND) -> Vec<(i32, i32, i32, i32, ToastAction)> {
    with_toast_data(hwnd, |opt| match opt {
        Some(d) if !d.actions.is_empty() => {
            let mut rect = mem::zeroed::<winapi::shared::windef::RECT>();
            GetClientRect(hwnd, &mut rect);
            let w = rect.right - rect.left;
            let bw = 150i32;
            let bh = 26i32;
            let y = rect.bottom - bh - 10;
            let total = d.actions.len() as i32;
            let gap = (w - total * bw) / (total + 1);
            d.actions
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let x = gap + i as i32 * (bw + gap);
                    (x, y, bw, bh, *a)
                })
                .collect()
        }
        _ => vec![],
    })
}

/// 以不可有所有权转移的方式读取窗口关联的 ToastData
unsafe fn with_toast_data<F, R>(hwnd: HWND, f: F) -> R
where
    F: FnOnce(Option<&ToastData>) -> R,
{
    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
    if p == 0 {
        f(None)
    } else {
        let data = &*(p as *const ToastData);
        f(Some(data))
    }
}

/// 执行某个操作按钮（§17.7）
unsafe fn run_toast_action(action: ToastAction) {
    match action {
        ToastAction::CopyDiagnostic => {
            let diag = crate::globals::last_diagnostic();
            if !diag.is_empty() {
                copy_to_clipboard(&diag);
                show_toast_actions("诊断已复制", ToastLevel::Info, &[]);
            } else {
                show_toast_actions("暂无可复制的诊断", ToastLevel::Info, &[]);
            }
        }
        ToastAction::OpenSettings => {
            if let Some(g) = crate::globals::with_globals(|g| (g.config.clone(), g.config_path.clone())) {
                std::thread::spawn(move || {
                    crate::settings_window::show_settings(g.0, &g.1);
                });
            }
        }
    }
}

unsafe extern "system" fn toast_proc(
    hwnd: HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps: winapi::um::winuser::PAINTSTRUCT = mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            draw_toast(hwnd, hdc);
            EndPaint(hwnd, &ps);
            0
        }
        WM_TIMER => {
            DestroyWindow(hwnd);
            0
        }
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xFFFF) as i16 as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i16 as i32;
            // 先命中操作按钮（错误 Toast）
            let hit = toast_action_rects(hwnd)
                .into_iter()
                .find(|(bx, by, bw, bh, _)| x >= *bx && x <= *bx + *bw && y >= *by && y <= *by + *bh)
                .map(|(_, _, _, _, a)| a);
            if let Some(action) = hit {
                run_toast_action(action);
                DestroyWindow(hwnd);
                0
            } else if with_toast_data(hwnd, |d| d.map(|x| x.level).unwrap_or(ToastLevel::Info))
                != ToastLevel::Error
            {
                // 非错误 Toast 点击关闭
                DestroyWindow(hwnd);
                0
            } else {
                // 错误 Toast 点击空白不关闭（需点按钮或叉）
                0
            }
        }
        WM_DESTROY => {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if p != 0 {
                let _ = Box::from_raw(p as *mut ToastData);
            }
            KillTimer(hwnd, TOAST_TIMER_ID);
            toast_remove(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn draw_toast(hwnd: HWND, hdc: winapi::shared::windef::HDC) {
    let mut rect = mem::zeroed::<winapi::shared::windef::RECT>();
    GetClientRect(hwnd, &mut rect);
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;

    let (bg, accent) = with_toast_data(hwnd, |opt| match opt {
        Some(d) => match d.level {
            ToastLevel::Info => (RGB(30, 41, 59), RGB(0, 150, 255)),
            ToastLevel::Warn => (RGB(60, 45, 15), RGB(230, 170, 40)),
            ToastLevel::Error => (RGB(60, 20, 20), RGB(230, 70, 70)),
        },
        None => (RGB(30, 41, 59), RGB(0, 150, 255)),
    });

    let bg_brush = winapi::um::wingdi::CreateSolidBrush(bg);
    let old = SelectObject(hdc, bg_brush as *mut _);
    winapi::um::wingdi::Rectangle(hdc, 0, 0, w, h);
    SelectObject(hdc, old);
    DeleteObject(bg_brush as *mut _);

    let bar = winapi::um::wingdi::CreateSolidBrush(accent);
    let old = SelectObject(hdc, bar as *mut _);
    winapi::um::wingdi::Rectangle(hdc, 0, 0, 6, h);
    SelectObject(hdc, old);
    DeleteObject(bar as *mut _);

    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, RGB(255, 255, 255));
    let font = CreateFontW(
        15, 0, 0, 0, FW_NORMAL, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0,
        encode_wide("Microsoft YaHei").as_ptr(),
    );
    let old = SelectObject(hdc, font as *mut _);

    let text = with_toast_data(hwnd, |opt| opt.map(|d| d.text.clone()).unwrap_or_default());
    let mut y = 12i32;
    // 有操作按钮时文本区压缩到按钮上方
    let max_y = with_toast_data(hwnd, |opt| {
        opt.map(|d| if d.actions.is_empty() { h - 8 } else { h - 44 })
            .unwrap_or(h - 8)
    });
    for line in text.lines().take(3) {
        let lw = encode_wide(line);
        if !lw.is_empty() {
            TextOutW(hdc, 16, y, lw.as_ptr(), (lw.len() as i32 - 1).max(0));
        }
        y += 20;
        if y > max_y {
            break;
        }
    }

    SelectObject(hdc, old);
    DeleteObject(font as *mut _);

    // 操作按钮
    for (bx, by, bw, bh, action) in toast_action_rects(hwnd) {
        let label = match action {
            ToastAction::CopyDiagnostic => tr!("copy_diagnostic"),
            ToastAction::OpenSettings => tr!("open_settings"),
        };
        draw_button(hdc, bx, by, bw, bh, label);
    }
}
