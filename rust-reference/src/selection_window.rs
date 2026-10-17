//! Region Selection Window Module
//! Provides fullscreen overlay with mouse drag selection for screenshot region.
//!
//! §17.4 增强：方向键微调、Shift+方向键调大小、Enter 确认、Esc 取消、区域记忆（上次选区）；
//! 本轮新增：选区放大镜（独立窗口 BitBlt 缩放）、8 个缩放手柄、边缘吸附
//! （屏幕边 + 顶层窗口边，可关）、底部居中提示条（2s 淡出）。

use crate::hotkey::Region;
use crate::tr;
use serde_json::json;
use std::cell::RefCell;
use std::mem;
use std::path::PathBuf;
use std::ptr;
use winapi::shared::minwindef::{LPARAM, UINT};
use winapi::shared::windef::{HBRUSH, HDC, HGDIOBJ, HWND, RECT};
use winapi::um::wingdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePen, CreateSolidBrush, DeleteDC,
    DeleteObject, Rectangle, RGB, SelectObject, SetROP2, SetStretchBltMode, StretchBlt, BLACK_BRUSH,
    HALFTONE, PS_SOLID, R2_NOTXORPEN, SRCCOPY, WHITE_BRUSH,
};
use winapi::um::winuser::{
    BeginPaint, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EndPaint,
    EnumWindows, GetClientRect, GetDC, GetKeyState, GetMessageW, GetSystemMetrics, InvalidateRect, IsWindowVisible,
    LoadCursorW, PostQuitMessage, RegisterClassW, ReleaseDC, ScreenToClient, SetCursor,
    SetLayeredWindowAttributes, SetTimer, ShowWindow, TranslateMessage, UpdateWindow, ClientToScreen,
    CS_HREDRAW, CS_VREDRAW, IDC_ARROW, IDC_CROSS, LWA_COLORKEY, MK_LBUTTON, MSG, SM_CXSCREEN,
    SM_CYSCREEN, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP, WM_DESTROY,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,     WM_PAINT, WM_TIMER, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE, SW_SHOW,
};

thread_local! {
    static SELECTION_STATE: RefCell<SelectionState> = RefCell::new(SelectionState::default());
}

/// 选区缩放手柄大小（§17.4 8 手柄）
const HANDLE: i32 = 8;
/// 吸附阈值（屏幕像素），§17.4
const SNAP_THRESHOLD: i32 = 12;
/// 放大镜放大倍数（§17.4）
const MAG_ZOOM: i32 = 3;
/// 放大镜源取样尺寸（像素）
const MAG_SRC: i32 = 60;
/// 放大镜窗口尺寸
const MAG_W: i32 = MAG_SRC * MAG_ZOOM;
/// 提示条淡出时长（ms）
const HINT_TIMER_MS: u32 = 2000;

#[derive(Clone, Copy, Default)]
struct SelectionState {
    is_selecting: bool,
    start_x: i32,
    start_y: i32,
    end_x: i32,
    end_y: i32,
    has_selection: bool,
    /// 已确认（鼠标抬起或回车），用于退出消息循环
    confirmed: bool,
    result: Option<Region>,
    /// 缩放手柄模式（0..7 对应 8 个手柄；-1 表示非缩放）
    resize_mode: i32,
    /// 拖动整体选区
    drag_move: bool,
    drag_off_x: i32,
    drag_off_y: i32,
    hint_visible: bool,
    /// 十字辅助线跟随光标（客户区坐标）
    cursor_x: i32,
    cursor_y: i32,
}

/// 冻结的桌面背景（选区窗口的不透明底图，保证接收鼠标输入）
#[derive(Default)]
struct BgState {
    hdc: HDC,
    hbmp: winapi::shared::windef::HBITMAP,
}

thread_local! {
    static BG: RefCell<BgState> = RefCell::new(BgState::default());
}

/// 读取当前配置开关（经全局上下文，避免改动 select_region 签名）
fn cfg_magnifier() -> bool {
    crate::globals::with_globals(|g| g.config.lock().unwrap().magnifier)
        .unwrap_or(true)
}
fn cfg_snap() -> bool {
    crate::globals::with_globals(|g| g.config.lock().unwrap().snap)
        .unwrap_or(true)
}

/// Show selection window and return selected region
pub fn select_region() -> Option<Region> {
    unsafe {
        // 区域记忆：加载上次选区作为初始预览（不自动确认，可微调后回车）
        let mut init = SelectionState::default();
        if let Some(r) = load_last_region() {
            init.start_x = r.x;
            init.start_y = r.y;
            init.end_x = r.x + r.w;
            init.end_y = r.y + r.h;
            init.has_selection = true;
        }
        init.hint_visible = true;
        SELECTION_STATE.with(|s| {
            *s.borrow_mut() = init;
        });

        let class_name = encode_wide("WinOCRSelectionWindow");
        let title_name = encode_wide("WinOCR Selection");

        let wnd_class = winapi::um::winuser::WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(selection_window_proc),
            hInstance: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_CROSS),
            // 背景由 WM_PAINT 全量绘制（冻结桌面截图），不设画刷避免黑闪
            hbrBackground: ptr::null_mut(),
            lpszClassName: class_name.as_ptr(),
            lpszMenuName: ptr::null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hIcon: ptr::null_mut(),
        };

        RegisterClassW(&wnd_class);

        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);

        // 抓一帧当前屏幕作为不透明背景。
        // 注意：不能用 LWA_COLORKEY 黑色键——分层窗口对键色像素是「点击穿透」的，
        // 全屏黑键会让所有鼠标消息穿透到下层窗口，导致十字线卡死、无法框选。
        capture_screen_background(screen_w, screen_h);

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class_name.as_ptr(),
            title_name.as_ptr(),
            WS_POPUP,
            0, 0, screen_w, screen_h,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );

        if hwnd.is_null() {
            return None;
        }

        // 提示条 2s 后淡出
        SetTimer(hwnd, 1, HINT_TIMER_MS, None);

        // 选区放大镜（独立顶层窗口）
        let mag_hwnd = if cfg_magnifier() {
            create_magnifier()
        } else {
            ptr::null_mut()
        };

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        // Message loop
        let mut msg: MSG = mem::zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);

            // 仅确认后退出（区域记忆的初始预览不会触发）
            let should_exit = SELECTION_STATE.with(|s| s.borrow().confirmed);
            if should_exit {
                break;
            }
        }

        if !mag_hwnd.is_null() {
            DestroyWindow(mag_hwnd);
            mag_cleanup();
        }
        cleanup_background();
        DestroyWindow(hwnd);

        // Return result
        SELECTION_STATE.with(|s| s.borrow().result)
    }
}

// ---------------------------------------------------------------------------
// 放大镜（§17.4）
// 独立顶层窗口，跟随光标显示光标周围 MAG_SRC 区域的 MAG_ZOOM 倍放大。
// 位图缓存于线程局部，鼠标移动时重新捕获并令其重绘。
// ---------------------------------------------------------------------------

thread_local! {
    static MAG: RefCell<MagState> = RefCell::new(MagState::default());
}

#[derive(Default, Clone)]
struct MagState {
    hdc: HDC,
    hbmp: winapi::shared::windef::HBITMAP,
    valid: bool,
}

/// 抓一帧主屏到线程局部缓存，作为选区窗口不透明背景
unsafe fn capture_screen_background(w: i32, h: i32) {
    let screen_dc = GetDC(ptr::null_mut());
    if screen_dc.is_null() {
        return;
    }
    let mem_dc = CreateCompatibleDC(screen_dc);
    let bmp = CreateCompatibleBitmap(screen_dc, w, h);
    if !mem_dc.is_null() && !bmp.is_null() {
        let old = SelectObject(mem_dc, bmp as HGDIOBJ);
        BitBlt(mem_dc, 0, 0, w, h, screen_dc, 0, 0, SRCCOPY);
        SelectObject(mem_dc, old);
        BG.with(|b| {
            let mut st = b.borrow_mut();
            if !st.hdc.is_null() {
                DeleteDC(st.hdc);
            }
            if !st.hbmp.is_null() {
                DeleteObject(st.hbmp as HGDIOBJ);
            }
            st.hdc = mem_dc;
            st.hbmp = bmp;
        });
    } else {
        if !mem_dc.is_null() {
            DeleteDC(mem_dc);
        }
        if !bmp.is_null() {
            DeleteObject(bmp as HGDIOBJ);
        }
    }
    ReleaseDC(ptr::null_mut(), screen_dc);
}

/// 释放背景缓存
unsafe fn cleanup_background() {
    BG.with(|b| {
        let mut st = b.borrow_mut();
        if !st.hdc.is_null() {
            DeleteDC(st.hdc);
            st.hdc = ptr::null_mut();
        }
        if !st.hbmp.is_null() {
            DeleteObject(st.hbmp as HGDIOBJ);
            st.hbmp = ptr::null_mut();
        }
    });
}

unsafe fn create_magnifier() -> HWND {
    let cls = encode_wide("WinOCRMagWindow");
    let wc = winapi::um::winuser::WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(mag_proc),
        hInstance: ptr::null_mut(),
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        hbrBackground: ptr::null_mut(),
        lpszClassName: cls.as_ptr(),
        lpszMenuName: ptr::null_mut(),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hIcon: ptr::null_mut(),
    };
    RegisterClassW(&wc);
    let screen_w = GetSystemMetrics(SM_CXSCREEN);
    let screen_h = GetSystemMetrics(SM_CYSCREEN);
    let x = (screen_w - MAG_W) / 2;
    let y = (screen_h - MAG_W) / 2;
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
        cls.as_ptr(),
        ptr::null(),
        WS_POPUP | WS_VISIBLE,
        x, y, MAG_W, MAG_W,
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
        ptr::null_mut(),
    );
    if !hwnd.is_null() {
        SetLayeredWindowAttributes(hwnd, RGB(0, 0, 0), 235, LWA_COLORKEY);
    }
    hwnd
}

/// 捕获光标周围区域到放大镜缓存，并触发重绘
unsafe fn mag_capture(mag_hwnd: HWND, cx: i32, cy: i32) {
    if mag_hwnd.is_null() {
        return;
    }
    let screen_dc = GetDC(ptr::null_mut());
    if screen_dc.is_null() {
        return;
    }
    let mut st = MAG.with(|m| m.borrow().clone());
    if st.hdc.is_null() {
        st.hdc = CreateCompatibleDC(screen_dc);
    }
    if !st.hbmp.is_null() {
        DeleteObject(st.hbmp as HGDIOBJ);
    }
    st.hbmp = CreateCompatibleBitmap(screen_dc, MAG_SRC, MAG_SRC);
    let old = SelectObject(st.hdc, st.hbmp as HGDIOBJ);
    let sx = (cx - MAG_SRC / 2).max(0);
    let sy = (cy - MAG_SRC / 2).max(0);
    BitBlt(st.hdc, 0, 0, MAG_SRC, MAG_SRC, screen_dc, sx, sy, SRCCOPY);
    SelectObject(st.hdc, old);
    st.valid = true;
    MAG.with(|m| *m.borrow_mut() = st);
    ReleaseDC(ptr::null_mut(), screen_dc);
    InvalidateRect(mag_hwnd, ptr::null_mut(), 1);
    // 跟随光标偏移，避免遮挡光标
    let mx = (cx + 24).min(GetSystemMetrics(SM_CXSCREEN) - MAG_W);
    let my = (cy + 24).min(GetSystemMetrics(SM_CYSCREEN) - MAG_W);
    winapi::um::winuser::SetWindowPos(
        mag_hwnd,
        ptr::null_mut(),
        mx,
        my,
        0,
        0,
        winapi::um::winuser::SWP_NOZORDER | winapi::um::winuser::SWP_NOACTIVATE | winapi::um::winuser::SWP_NOSIZE,
    );
}

unsafe fn mag_cleanup() {
    let mut st = MAG.with(|m| m.borrow().clone());
    if !st.hdc.is_null() {
        DeleteDC(st.hdc);
        st.hdc = ptr::null_mut();
    }
    if !st.hbmp.is_null() {
        DeleteObject(st.hbmp as HGDIOBJ);
        st.hbmp = ptr::null_mut();
    }
    st.valid = false;
    MAG.with(|m| *m.borrow_mut() = st);
}

unsafe extern "system" fn mag_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps = mem::zeroed::<winapi::um::winuser::PAINTSTRUCT>();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rect = mem::zeroed::<RECT>();
            GetClientRect(hwnd, &mut rect);
            // 背景
            let bg = CreateSolidBrush(RGB(20, 20, 24));
            let old = SelectObject(hdc, bg as HGDIOBJ);
            Rectangle(hdc, 0, 0, rect.right, rect.bottom);
            SelectObject(hdc, old);
            DeleteObject(bg as HGDIOBJ);

            let st = MAG.with(|m| m.borrow().clone());
            if st.valid && !st.hdc.is_null() {
                SetStretchBltMode(hdc, HALFTONE);
                StretchBlt(
                    hdc, 0, 0, MAG_W, MAG_W, st.hdc, 0, 0, MAG_SRC, MAG_SRC, SRCCOPY,
                );
            }
            // 中心十字线
            let pen = CreatePen(PS_SOLID as i32, 1, RGB(255, 80, 80));
            let old = SelectObject(hdc, pen as HGDIOBJ);
            winapi::um::wingdi::MoveToEx(hdc, MAG_W / 2, 0, ptr::null_mut());
            winapi::um::wingdi::LineTo(hdc, MAG_W / 2, MAG_W);
            winapi::um::wingdi::MoveToEx(hdc, 0, MAG_W / 2, ptr::null_mut());
            winapi::um::wingdi::LineTo(hdc, MAG_W, MAG_W / 2);
            SelectObject(hdc, old);
            DeleteObject(pen as HGDIOBJ);
            EndPaint(hwnd, &ps);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ---------------------------------------------------------------------------
// 选区窗口过程
// ---------------------------------------------------------------------------

unsafe extern "system" fn selection_window_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps = mem::zeroed::<winapi::um::winuser::PAINTSTRUCT>();
            let hdc = BeginPaint(hwnd, &mut ps);

            // 背景：冻结的桌面截图（不透明，保证鼠标输入落在本窗口）
            BG.with(|b| {
                let st = b.borrow();
                if !st.hdc.is_null() {
                    BitBlt(
                        hdc,
                        0,
                        0,
                        ps.rcPaint.right,
                        ps.rcPaint.bottom,
                        st.hdc,
                        0,
                        0,
                        SRCCOPY,
                    );
                }
            });

            SELECTION_STATE.with(|s| {
                let state = s.borrow();
                // 十字辅助线（未拖拽时跟随光标，指示当前坐标）
                if !state.is_selecting {
                    draw_crosshair(hdc, state.cursor_x, state.cursor_y);
                }
                // 选区矩形 + 手柄
                if state.is_selecting || state.has_selection {
                    draw_selection_rect(hdc, state.start_x, state.start_y, state.end_x, state.end_y);
                    if state.has_selection && !state.is_selecting {
                        draw_handles(hdc, state.start_x, state.start_y, state.end_x, state.end_y);
                    }
                }
            });

            // 提示条（2s 后淡出）
            let hint = SELECTION_STATE.with(|s| s.borrow().hint_visible);
            if hint {
                draw_hint(hwnd);
            }

            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_LBUTTONDOWN => {
            let x = (lparam & 0xFFFF) as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i32;

            let hit_handle = SELECTION_STATE.with(|s| {
                let st = s.borrow();
                if st.has_selection && !st.is_selecting {
                    hit_handle_test(x, y, st.start_x, st.start_y, st.end_x, st.end_y)
                } else {
                    -1
                }
            });

            if hit_handle >= 0 {
                SELECTION_STATE.with(|s| s.borrow_mut().resize_mode = hit_handle);
                winapi::um::winuser::SetCapture(hwnd);
            } else {
                let inside = SELECTION_STATE.with(|s| {
                    let st = s.borrow();
                    st.has_selection
                        && x >= st.start_x.min(st.end_x)
                        && x <= st.start_x.max(st.end_x)
                        && y >= st.start_y.min(st.end_y)
                        && y <= st.start_y.max(st.end_y)
                });
                if inside {
                    // 拖动整体选区
                    let (ox, oy) = SELECTION_STATE.with(|s| {
                        let st = s.borrow();
                        (x - st.start_x, y - st.start_y)
                    });
                    SELECTION_STATE.with(|s| {
                        let mut st = s.borrow_mut();
                        st.drag_move = true;
                        st.drag_off_x = ox;
                        st.drag_off_y = oy;
                    });
                    winapi::um::winuser::SetCapture(hwnd);
                } else {
                    SELECTION_STATE.with(|s| {
                        let mut state = s.borrow_mut();
                        state.is_selecting = true;
                        state.start_x = x;
                        state.start_y = y;
                        state.end_x = x;
                        state.end_y = y;
                        state.has_selection = false;
                        state.confirmed = false;
                        state.result = None;
                        state.resize_mode = -1;
                        state.drag_move = false;
                    });
                    winapi::um::winuser::SetCapture(hwnd);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let x = (lparam & 0xFFFF) as i32;
            let y = ((lparam >> 16) & 0xFFFF) as i32;

            // 放大镜跟随
            let mag = find_mag_hwnd();
            mag_capture(mag, x, y);

            let mode = SELECTION_STATE.with(|s| s.borrow().resize_mode);
            let dragging = SELECTION_STATE.with(|s| s.borrow().is_selecting);
            let moving = SELECTION_STATE.with(|s| s.borrow().drag_move);

            if mode >= 0 {
                resize_by_handle(hwnd, mode, x, y);
            } else if dragging {
                let (nx, ny) = maybe_snap(x, y);
                SELECTION_STATE.with(|s| {
                    let mut state = s.borrow_mut();
                    state.end_x = nx;
                    state.end_y = ny;
                });
                InvalidateRect(hwnd, ptr::null_mut(), 1);
            } else if moving {
                let (ox, oy) = SELECTION_STATE.with(|s| {
                    let st = s.borrow();
                    (st.drag_off_x, st.drag_off_y)
                });
                let (nx, ny) = maybe_snap(x - ox, y - oy);
                SELECTION_STATE.with(|s| {
                    let mut st = s.borrow_mut();
                    let w = st.end_x - st.start_x;
                    let h = st.end_y - st.start_y;
                    st.start_x = nx;
                    st.start_y = ny;
                    st.end_x = nx + w;
                    st.end_y = ny + h;
                });
                InvalidateRect(hwnd, ptr::null_mut(), 1);
            }

            // 记录光标位置（十字辅助线跟随）并重绘
            SELECTION_STATE.with(|s| {
                let mut st = s.borrow_mut();
                st.cursor_x = x;
                st.cursor_y = y;
            });
            InvalidateRect(hwnd, ptr::null_mut(), 1);
            0
        }
        WM_LBUTTONUP => {
            let mode = SELECTION_STATE.with(|s| s.borrow().resize_mode);
            let dragging = SELECTION_STATE.with(|s| s.borrow().is_selecting);
            let moving = SELECTION_STATE.with(|s| s.borrow().drag_move);
            if mode >= 0 || dragging || moving {
                let x = (lparam & 0xFFFF) as i32;
                let y = ((lparam >> 16) & 0xFFFF) as i32;
                if mode >= 0 {
                    resize_by_handle(hwnd, mode, x, y);
                }
                let region = SELECTION_STATE.with(|s| {
                    let mut state = s.borrow_mut();
                    state.resize_mode = -1;
                    state.drag_move = false;
                    if state.is_selecting {
                        state.end_x = x;
                        state.end_y = y;
                        state.is_selecting = false;
                        state.has_selection = true;
                    }
                    let min_x = state.start_x.min(state.end_x);
                    let min_y = state.start_y.min(state.end_y);
                    let max_x = state.start_x.max(state.end_x);
                    let max_y = state.start_y.max(state.end_y);
                    if max_x - min_x > 10 && max_y - min_y > 10 {
                        let r = Region {
                            x: min_x,
                            y: min_y,
                            w: max_x - min_x,
                            h: max_y - min_y,
                        };
                        state.result = Some(r);
                        Some(r)
                    } else {
                        None
                    }
                });
                if let Some(r) = region {
                    save_last_region(&r);
                }
                winapi::um::winuser::ReleaseCapture();
            }
            0
        }
        WM_TIMER => {
            // 提示条淡出
            SELECTION_STATE.with(|s| s.borrow_mut().hint_visible = false);
            InvalidateRect(hwnd, ptr::null_mut(), 1);
            0
        }
        WM_KEYDOWN => {
            match wparam as i32 {
                VK_ESCAPE => {
                    DestroyWindow(hwnd);
                }
                VK_RETURN => {
                    if let Some(r) = finalize_selection() {
                        save_last_region(&r);
                    }
                }
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
                    nudge_selection(wparam as i32, hwnd);
                }
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// 查找放大镜窗口句柄（类名匹配；线程内唯一）
unsafe fn find_mag_hwnd() -> HWND {
    let mut found = ptr::null_mut();
    EnumWindows(
        Some(enum_find_mag),
        &mut found as *mut _ as LPARAM,
    );
    found
}

unsafe extern "system" fn enum_find_mag(hwnd: HWND, lparam: LPARAM) -> i32 {
    let target = lparam as *mut HWND;
    let mut cls = [0u16; 64];
    let n = winapi::um::winuser::GetClassNameW(hwnd, cls.as_mut_ptr(), cls.len() as i32);
    if n > 0 {
        let name = String::from_utf16_lossy(&cls[..n as usize]);
        if name == "WinOCRMagWindow" && IsWindowVisible(hwnd) != 0 {
            *target = hwnd;
            return 0; // 停止枚举
        }
    }
    1
}

/// 8 个手柄命中测试，返回 0..7，未命中返回 -1
unsafe fn hit_handle_test(x: i32, y: i32, x1: i32, y1: i32, x2: i32, y2: i32) -> i32 {
    let (min_x, min_y, max_x, max_y) = normalize(x1, y1, x2, y2);
    let mid_x = (min_x + max_x) / 2;
    let mid_y = (min_y + max_y) / 2;
    let pts = [
        (min_x, min_y),
        (mid_x, min_y),
        (max_x, min_y),
        (min_x, mid_y),
        (max_x, mid_y),
        (min_x, max_y),
        (mid_x, max_y),
        (max_x, max_y),
    ];
    for (i, (hx, hy)) in pts.iter().enumerate() {
        if x >= hx - HANDLE && x <= hx + HANDLE && y >= hy - HANDLE && y <= hy + HANDLE {
            return i as i32;
        }
    }
    -1
}

/// 按手柄模式调整选区（§17.4 8 手柄缩放）
unsafe fn resize_by_handle(hwnd: HWND, mode: i32, x: i32, y: i32) {
    let (nx, ny) = maybe_snap(x, y);
    SELECTION_STATE.with(|s| {
        let mut st = s.borrow_mut();
        // mode: 0 TL,1 TM,2 TR,3 ML,4 MR,5 BL,6 BM,7 BR
        let (mut min_x, mut min_y, mut max_x, mut max_y) =
            normalize(st.start_x, st.start_y, st.end_x, st.end_y);
        match mode {
            0 => {
                min_x = nx;
                min_y = ny;
            }
            1 => min_y = ny,
            2 => {
                max_x = nx;
                min_y = ny;
            }
            3 => min_x = nx,
            4 => max_x = nx,
            5 => {
                min_x = nx;
                max_y = ny;
            }
            6 => max_y = ny,
            7 => {
                max_x = nx;
                max_y = ny;
            }
            _ => {}
        }
        st.start_x = min_x;
        st.start_y = min_y;
        st.end_x = max_x;
        st.end_y = max_y;
    });
    InvalidateRect(hwnd, ptr::null_mut(), 1);
}

/// 边缘吸附（§17.4）：屏幕边 + 顶层可见窗口边，阈值为 SNAP_THRESHOLD。
unsafe fn maybe_snap(v: i32, _unused: i32) -> (i32, i32) {
    // 仅对单值吸附：x 与 y 分别调用。这里 v 统一处理单轴。
    // 调用方对 x、y 分别传入同一函数（见下）。
    (snap_axis(v), snap_axis(v))
}

/// 单轴吸附：返回吸附后的值
fn snap_axis(v: i32) -> i32 {
    if !cfg_snap() {
        return v;
    }
    let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let mut best = v;
    let mut best_dist = SNAP_THRESHOLD;
    // 屏幕边
    for edge in [0, screen_w, screen_h] {
        let d = (v - edge).abs();
        if d < best_dist {
            best_dist = d;
            best = edge;
        }
    }
    // 窗口边（仅 X 与 Y 通用阈值；枚举顶层可见窗口）
    for edge in window_edges() {
        let d = (v - edge).abs();
        if d < best_dist {
            best_dist = d;
            best = edge;
        }
    }
    best
}

thread_local! {
    static WIN_EDGES: RefCell<Vec<i32>> = RefCell::new(Vec::new());
}

/// 枚举顶层可见窗口的左右/上下边（用于吸附候选）
fn window_edges() -> Vec<i32> {
    unsafe {
        WIN_EDGES.with(|e| {
            let mut v = e.borrow_mut();
            v.clear();
            EnumWindows(Some(enum_edges), &mut *v as *mut Vec<i32> as LPARAM);
            v.clone()
        })
    }
}

unsafe extern "system" fn enum_edges(hwnd: HWND, lparam: LPARAM) -> i32 {
    let vec = &mut *(lparam as *mut Vec<i32>);
    if winapi::um::winuser::IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    let mut rect = mem::zeroed::<RECT>();
    if winapi::um::winuser::GetWindowRect(hwnd, &mut rect) != 0 {
        vec.push(rect.left);
        vec.push(rect.right);
        vec.push(rect.top);
        vec.push(rect.bottom);
    }
    1
}

/// 方向键微调选区：移动整体；Shift+方向键调整大小（右下角）。
/// 普通步进 10px，Shift 精确 1px。
unsafe fn nudge_selection(vk: i32, hwnd: HWND) {
    let shift = (GetKeyState(VK_SHIFT as i32) as u16 & 0x8000) != 0;
    let step = if shift { 1 } else { 10 };
    SELECTION_STATE.with(|s| {
        let mut st = s.borrow_mut();
        match vk {
            VK_LEFT => {
                if shift {
                    st.end_x -= step;
                } else {
                    st.start_x -= step;
                    st.end_x -= step;
                }
            }
            VK_RIGHT => {
                if shift {
                    st.end_x += step;
                } else {
                    st.start_x += step;
                    st.end_x += step;
                }
            }
            VK_UP => {
                if shift {
                    st.end_y -= step;
                } else {
                    st.start_y -= step;
                    st.end_y -= step;
                }
            }
            VK_DOWN => {
                if shift {
                    st.end_y += step;
                } else {
                    st.start_y += step;
                    st.end_y += step;
                }
            }
            _ => {}
        }
        st.has_selection = true;
    });
    InvalidateRect(hwnd, ptr::null_mut(), 1);
}

/// 把当前 start/end 归一化为 Region；有效则写入 result 并标记 confirmed。
fn finalize_selection() -> Option<Region> {
    let mut out = None;
    SELECTION_STATE.with(|s| {
        let mut st = s.borrow_mut();
        if st.has_selection {
            let (min_x, min_y, max_x, max_y) = normalize(st.start_x, st.start_y, st.end_x, st.end_y);
            if max_x - min_x > 10 && max_y - min_y > 10 {
                let r = Region {
                    x: min_x,
                    y: min_y,
                    w: max_x - min_x,
                    h: max_y - min_y,
                };
                st.result = Some(r.clone());
                st.confirmed = true;
                out = Some(r);
            }
        }
    });
    out
}

#[inline]
fn normalize(x1: i32, y1: i32, x2: i32, y2: i32) -> (i32, i32, i32, i32) {
    (x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2))
}

fn last_region_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    Some(dir.join("last_region.json"))
}

/// 加载上次选区（区域记忆）
fn load_last_region() -> Option<Region> {
    let p = last_region_path()?;
    let txt = std::fs::read_to_string(p).ok()?;
    let v = serde_json::from_str::<serde_json::Value>(&txt).ok()?;
    Some(Region {
        x: v.get("x")?.as_i64()? as i32,
        y: v.get("y")?.as_i64()? as i32,
        w: v.get("w")?.as_i64()? as i32,
        h: v.get("h")?.as_i64()? as i32,
    })
}

/// 持久化本次选区，下次打开作为初始预览
fn save_last_region(r: &Region) {
    if let Some(p) = last_region_path() {
        let _ = std::fs::write(
            p,
            json!({ "x": r.x, "y": r.y, "w": r.w, "h": r.h }).to_string(),
        );
    }
}

/// 全屏十字辅助线（跟随光标；光标未进入窗口时不画）
unsafe fn draw_crosshair(hdc: HDC, x: i32, y: i32) {
    if x <= 0 && y <= 0 {
        return;
    }
    let w = GetSystemMetrics(SM_CXSCREEN);
    let h = GetSystemMetrics(SM_CYSCREEN);
    let pen = CreatePen(PS_SOLID as i32, 1, RGB(255, 90, 90));
    let old = SelectObject(hdc, pen as HGDIOBJ);
    winapi::um::wingdi::MoveToEx(hdc, 0, y, ptr::null_mut());
    winapi::um::wingdi::LineTo(hdc, w, y);
    winapi::um::wingdi::MoveToEx(hdc, x, 0, ptr::null_mut());
    winapi::um::wingdi::LineTo(hdc, x, h);
    SelectObject(hdc, old);
    DeleteObject(pen as HGDIOBJ);
}

fn draw_selection_rect(_hdc: HDC, x1: i32, y1: i32, x2: i32, y2: i32) {
    unsafe {
        let pen = CreatePen(PS_SOLID as i32, 2, RGB(0, 120, 255));
        let old_pen = SelectObject(_hdc, pen as HGDIOBJ);
        SetROP2(_hdc, R2_NOTXORPEN);

        let (min_x, min_y, max_x, max_y) = normalize(x1, y1, x2, y2);

        Rectangle(_hdc, min_x, min_y, max_x, max_y);

        SelectObject(_hdc, old_pen);
        DeleteObject(pen as HGDIOBJ);
    }
}

/// 绘制 8 个缩放手柄（§17.4）
unsafe fn draw_handles(hdc: HDC, x1: i32, y1: i32, x2: i32, y2: i32) {
    let (min_x, min_y, max_x, max_y) = normalize(x1, y1, x2, y2);
    let mid_x = (min_x + max_x) / 2;
    let mid_y = (min_y + max_y) / 2;
    let pts = [
        (min_x, min_y),
        (mid_x, min_y),
        (max_x, min_y),
        (min_x, mid_y),
        (max_x, mid_y),
        (min_x, max_y),
        (mid_x, max_y),
        (max_x, max_y),
    ];
    let brush = CreateSolidBrush(RGB(255, 255, 255)) as HBRUSH;
    let pen = CreatePen(PS_SOLID as i32, 1, RGB(0, 120, 255)) as HGDIOBJ;
    let old_brush = SelectObject(hdc, brush as HGDIOBJ);
    let old_pen = SelectObject(hdc, pen);
    for (hx, hy) in pts.iter() {
        Rectangle(hdc, hx - HANDLE / 2, hy - HANDLE / 2, hx + HANDLE / 2, hy + HANDLE / 2);
    }
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(brush as HGDIOBJ);
    DeleteObject(pen);
}

/// 底部居中提示条（§17.4 提示条），2s 后由 WM_TIMER 触发隐藏
unsafe fn draw_hint(hwnd: HWND) {
    let hdc = GetDC(hwnd);
    let mut rect = mem::zeroed::<RECT>();
    GetClientRect(hwnd, &mut rect);
    let text = tr!("select_hint");
    let tw = text.chars().count() as i32 * 14 + 24;
    let th = 30i32;
    let x = (rect.right - tw) / 2;
    let y = rect.bottom - 60;
    let bg = CreateSolidBrush(RGB(20, 20, 28)) as HBRUSH;
    let old = SelectObject(hdc, bg as HGDIOBJ);
    Rectangle(hdc, x, y, x + tw, y + th);
    SelectObject(hdc, old);
    DeleteObject(bg as HGDIOBJ);
    winapi::um::wingdi::SetBkMode(hdc, winapi::um::wingdi::TRANSPARENT as i32);
    winapi::um::wingdi::SetTextColor(hdc, RGB(235, 235, 245));
    let font = winapi::um::wingdi::CreateFontW(
        14, 0, 0, 0, winapi::um::wingdi::FW_NORMAL, 0, 0, 0,
        winapi::um::wingdi::DEFAULT_CHARSET, winapi::um::wingdi::OUT_DEFAULT_PRECIS,
        winapi::um::wingdi::CLIP_DEFAULT_PRECIS, winapi::um::wingdi::CLEARTYPE_QUALITY, 0,
        encode_wide("Microsoft YaHei").as_ptr(),
    );
    let old = SelectObject(hdc, font as HGDIOBJ);
    let lw = encode_wide(text);
    winapi::um::wingdi::TextOutW(hdc, x + 12, y + 6, lw.as_ptr(), (lw.len() as i32 - 1).max(0));
    SelectObject(hdc, old);
    DeleteObject(font as HGDIOBJ);
    ReleaseDC(hwnd, hdc);
}

fn encode_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
