// ============================================================
// Services · 交互式截图（真实实现）
// 移植自 rust-reference/screenshot.rs（GDI 抓屏路径）+ selection_window.rs（框选蒙版核心路径）。
// 流程：冻结背景（BitBlt 屏幕到内存 DC）→ 全屏 popup 窗显示背景 → 拖选（含尺寸提示、Esc 取消）
//      → GDI 抓取选区 → PNG 存 %TEMP% → 返回路径（供 OCR 步骤使用）。
// 绘制策略：背景/压暗/边框先合成到 frame_dc，最后一性 BitBlt 上屏（零撕裂）；
//          压暗用 AlphaBlend 常量 alpha（不用 1bpp 抖动刷，避免条纹观感）。
// MVP 范围：主显示器；无放大镜/8 手柄/边缘吸附（参考实现有，后置打磨）。
// ============================================================
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{anyhow, Result};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePen, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, GetDC, GetDIBits, GetStockObject, InvalidateRect,
    IntersectClipRect, NULL_BRUSH, PatBlt, ReleaseDC, SelectClipRgn, SelectObject, SetBkMode,
    SetTextColor, AlphaBlend, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BLACKNESS, BI_RGB,
    DIB_RGB_COLORS, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW,
    GetSystemMetrics, LoadCursorW, PostQuitMessage, RegisterClassW, SetForegroundWindow,
    ShowWindow, TranslateMessage, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_DESTROY, WM_KEYDOWN,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows_sys::Win32::Graphics::Gdi::AC_SRC_OVER;

// ============================================================
// 公共 API
// ============================================================

/// 一次截图的结果
#[derive(Debug, Clone)]
pub struct CaptureResult {
    /// 24bpp RGB 像素（紧凑，无行对齐 padding），直接进入 OCR，免去 PNG 编码/落盘/读盘/解码的往返。
    pub rgb: Vec<u8>,
    pub width: i32,
    pub height: i32,
    /// 仅调试：设环境变量 WINOCR_SAVE_PNG 时把截图落盘到此路径；默认 None（不走磁盘）。
    pub path: Option<PathBuf>,
}

static LAST_CAPTURE: Mutex<Option<CaptureResult>> = Mutex::new(None);
/// 防重入：蒙版会话进行中时忽略新的截图请求（连按 Alt+Q 不叠加多个蒙版）
static CAPTURE_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 最近一次截图结果（供 OCR 步骤读取）
pub fn last_capture() -> Option<CaptureResult> {
    LAST_CAPTURE.lock().unwrap().clone()
}

fn set_last(r: CaptureResult) {
    *LAST_CAPTURE.lock().unwrap() = Some(r);
}

/// 交互式截图：弹全屏蒙版让用户框选，抓取选区并存 PNG。
/// 阻塞调用方线程直到用户完成/取消；必须在非 UI 线程调用。
/// 返回 None = 用户取消（Esc / 点选过小）或已有会话进行中。
pub fn capture_interactive() -> Result<Option<CaptureResult>> {
    use std::sync::atomic::Ordering;
    if CAPTURE_BUSY.swap(true, Ordering::SeqCst) {
        log::info!("[services::capture] capture session already running, ignored");
        return Ok(None);
    }
    let result = capture_interactive_inner();
    CAPTURE_BUSY.store(false, Ordering::SeqCst);
    result
}

fn capture_interactive_inner() -> Result<Option<CaptureResult>> {
    let rect = match select_region()? {
        Some(r) => r,
        None => return Ok(None),
    };
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);

    let rgb = capture_region_gdi(rect.left, rect.top, w, h)?;

    // A 优化：默认不落盘，rgb 直接交给 OCR（见 services::ocr::recognize_capture），
    // 去掉「编码 PNG → 写盘 → 读盘 → 解码」的往返。设 WINOCR_SAVE_PNG 时才写盘，用于调试比对。
    let dbg_path = if std::env::var("WINOCR_SAVE_PNG").is_ok() {
        Some(save_png(&rgb, w, h)?)
    } else {
        None
    };

    let result = CaptureResult { rgb, width: w, height: h, path: dbg_path };
    log::info!(
        "[services::capture] captured {}x{} (rgb {} bytes){}",
        w, h, result.rgb.len(),
        result.path.as_ref().map(|p| format!(" -> {}", p.display())).unwrap_or_default()
    );
    set_last(result.clone());
    Ok(Some(result))
}

// ============================================================
// 框选蒙版（全屏 popup 窗，冻结背景 + 拖选）
// ============================================================

const CLASS_NAME: &str = "WinOCR_SelectionOverlay";
const WINDOW_NAME: &str = "WinOCR-Capture";
/// 最小有效选区（像素），小于此视为误点取消
const MIN_SELECT: i32 = 4;
/// 品牌色 #6366f0 的 COLORREF（0x00BBGGRR）
const ACCENT_COLORREF: u32 = 0xF06663;
/// 选区外压暗强度（0-255，越小越暗）
const DIM_ALPHA: u8 = 90;

/// 四个内存 DC 的句柄包（isize 存储：静态 Mutex 要求 Send）
#[derive(Clone, Copy)]
struct Dcs {
    mem: isize,    // 冻结背景（干净、未压暗）
    frame: isize,  // 组合缓冲（背景+覆盖层）
    dim: isize,    // 纯黑源（仅用于预渲染压暗底图那一次 AlphaBlend）
    dimmed: isize, // A 优化：预渲染好的「全屏压暗底图」= mem 整屏压暗一次的副本
}

struct OverlayState {
    dcs: Dcs,
    w: i32,
    h: i32,
    dragging: bool,
    start_x: i32,
    start_y: i32,
    cur_x: i32,
    cur_y: i32,
    /// B 优化：上一帧已绘制的选区，用于脏矩形重绘（仅还原/重绘 prev∪cur 区域）。
    prev_rect: Option<RECT>,
}

static STATE: Mutex<Option<OverlayState>> = Mutex::new(None);
static RESULT: Mutex<Option<RECT>> = Mutex::new(None);

/// 弹出全屏框选蒙版，阻塞直到用户完成/取消。必须在非 UI 线程调用。
fn select_region() -> Result<Option<RECT>> {
    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);

        // 1. 冻结背景：先把整个屏幕抓到内存 DC（在窗口显示之前，避免拍到蒙版自身）
        let hdc_screen = GetDC(std::ptr::null_mut());
        if hdc_screen.is_null() {
            return Err(anyhow!("GetDC failed"));
        }
        let mem_dc = CreateCompatibleDC(hdc_screen);
        let mem_bmp = CreateCompatibleBitmap(hdc_screen, screen_w, screen_h);
        let old_mem = SelectObject(mem_dc, mem_bmp);
        BitBlt(mem_dc, 0, 0, screen_w, screen_h, hdc_screen, 0, 0, SRCCOPY);

        // 2. 组合缓冲 frame（背景+覆盖层合成后一次性上屏）与压暗源 dim（全屏纯黑）
        let frame_dc = CreateCompatibleDC(hdc_screen);
        let frame_bmp = CreateCompatibleBitmap(hdc_screen, screen_w, screen_h);
        let old_frame = SelectObject(frame_dc, frame_bmp);
        let dim_dc = CreateCompatibleDC(hdc_screen);
        let dim_bmp = CreateCompatibleBitmap(hdc_screen, screen_w, screen_h);
        let old_dim = SelectObject(dim_dc, dim_bmp);
        PatBlt(dim_dc, 0, 0, screen_w, screen_h, BLACKNESS);

        // A 优化：预渲染「全屏压暗底图」= 干净背景 + 整屏压暗一次（整个会话只做这一次）。
        // 之后每帧拖拽只需 2 次 BitBlt（铺压暗底图 + 还原选区内部为清晰背景），
        // 取代原先「对选区外 4 块各做一次 AlphaBlend」——AlphaBlend 是逐像素混合，
        // 比 BitBlt 慢得多，这是高分辨率下拖拽掉帧的主因。
        let dimmed_dc = CreateCompatibleDC(hdc_screen);
        let dimmed_bmp = CreateCompatibleBitmap(hdc_screen, screen_w, screen_h);
        let old_dimmed = SelectObject(dimmed_dc, dimmed_bmp);
        BitBlt(dimmed_dc, 0, 0, screen_w, screen_h, mem_dc, 0, 0, SRCCOPY);
        let bf_dim = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: DIM_ALPHA,
            AlphaFormat: 0,
        };
        AlphaBlend(dimmed_dc, 0, 0, screen_w, screen_h, dim_dc, 0, 0, screen_w, screen_h, bf_dim);

        let dcs = Dcs {
            mem: mem_dc as isize,
            frame: frame_dc as isize,
            dim: dim_dc as isize,
            dimmed: dimmed_dc as isize,
        };
        ReleaseDC(std::ptr::null_mut(), hdc_screen);

        // 3. 初始化共享状态
        {
            let mut st = STATE.lock().unwrap();
            *st = Some(OverlayState {
                dcs,
                w: screen_w,
                h: screen_h,
                dragging: false,
                start_x: 0,
                start_y: 0,
                cur_x: 0,
                cur_y: 0,
                prev_rect: None,
            });
        }
        *RESULT.lock().unwrap() = None;

        // 4. 注册窗口类 + 创建全屏 popup 窗
        let hinstance = GetModuleHandleW(std::ptr::null());
        let class_w = to_wide(CLASS_NAME);
        let title_w = to_wide(WINDOW_NAME);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(selection_wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), 32515 as _), // IDC_CROSS
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_w.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            // 1410 = ERROR_CLASS_ALREADY_EXISTS：首次截图已注册过该类，属正常，继续复用
            if windows_sys::Win32::Foundation::GetLastError() != 1410 {
                cleanup_all(&dcs, mem_bmp, old_mem, frame_bmp, old_frame,
                    dim_bmp, old_dim, dimmed_bmp, old_dimmed);
                return Err(anyhow!("RegisterClassW failed"));
            }
            log::info!("[services::capture] window class already registered, reuse");
        }
        // WS_EX_COMPOSITED：整窗双缓冲；不带 WS_VISIBLE：先画好首帧再显示（防闪屏）
        const WS_EX_COMPOSITED: u32 = 0x0200_0000;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_COMPOSITED,
            class_w.as_ptr(),
            title_w.as_ptr(),
            WS_POPUP,
            0,
            0,
            screen_w,
            screen_h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            cleanup_all(&dcs, mem_bmp, old_mem, frame_bmp, old_frame,
                dim_bmp, old_dim, dimmed_bmp, old_dimmed);
            return Err(anyhow!("CreateWindowExW failed"));
        }

        // 5. 预绘制首帧：背景先合成到 frame，再一次性上屏；然后才显示窗口
        compose_frame(&dcs, screen_w, screen_h, None);
        let hdc_win = GetDC(hwnd);
        BitBlt(hdc_win, 0, 0, screen_w, screen_h, frame_dc, 0, 0, SRCCOPY);
        ReleaseDC(hwnd, hdc_win);
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd); // 拿到键盘焦点，Esc 才有效

        // 6. 消息循环（结束时 wndproc 已把结果写入 RESULT）
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if r == 0 || r == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // 7. 清理 GDI 资源
        cleanup_all(&dcs, mem_bmp, old_mem, frame_bmp, old_frame,
            dim_bmp, old_dim, dimmed_bmp, old_dimmed);
        *STATE.lock().unwrap() = None;

        Ok(*RESULT.lock().unwrap())
    }
}

/// 把「背景 + 选区覆盖层」合成到 frame_dc（不直接碰屏幕）
fn compose_frame(dcs: &Dcs, w: i32, h: i32, sel: Option<(i32, i32, i32, i32)>) {
    unsafe {
        match sel {
            // 无选区（初始/结束）：直接铺干净背景，不压暗
            None => {
                BitBlt(dcs.frame as HWND, 0, 0, w, h, dcs.mem as HWND, 0, 0, SRCCOPY);
            }
            // 有选区：draw_overlay 内部先铺预渲染的压暗底图，再把选区内部还原为清晰背景
            Some((x1, y1, x2, y2)) => {
                draw_overlay(dcs.frame as HWND, dcs, w, h, x1, y1, x2, y2);
            }
        }
    }
}

fn cleanup_all(
    dcs: &Dcs,
    mem_bmp: HWND, old_mem: HWND,
    frame_bmp: HWND, old_frame: HWND,
    dim_bmp: HWND, old_dim: HWND,
    dimmed_bmp: HWND, old_dimmed: HWND,
) {
    unsafe {
        SelectObject(dcs.mem as HWND, old_mem);
        DeleteObject(mem_bmp);
        DeleteDC(dcs.mem as HWND);
        SelectObject(dcs.frame as HWND, old_frame);
        DeleteObject(frame_bmp);
        DeleteDC(dcs.frame as HWND);
        SelectObject(dcs.dim as HWND, old_dim);
        DeleteObject(dim_bmp);
        DeleteDC(dcs.dim as HWND);
        SelectObject(dcs.dimmed as HWND, old_dimmed);
        DeleteObject(dimmed_bmp);
        DeleteDC(dcs.dimmed as HWND);
    }
}

unsafe extern "system" fn selection_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut st_guard = STATE.lock().unwrap();
            if let Some(st) = st_guard.as_mut() {
                let w = st.w;
                let h = st.h;
                if st.dragging {
                    let cur = norm_rect(st.start_x, st.start_y, st.cur_x, st.cur_y);
                    match st.prev_rect {
                        Some(prev) => {
                            // B 优化：脏矩形重绘 —— 仅还原 prev∪cur 区域到干净背景，再画新选区，
                            // 不再每帧全屏 BitBlt + 4 条 AlphaBlend（4K 拖拽掉帧的根因）。
                            let rb = dirty_bbox(prev, cur, w, h);
                            let rbw = rb.right - rb.left;
                            let rbh = rb.bottom - rb.top;
                            // A 优化：不再需要先「从冻结背景还原脏区」——draw_overlay 会用预压暗底图
                            // 整块覆盖脏区（自动清掉上一帧的边框/标签残留，不会累积变暗），
                            // 再把选区内部还原为清晰背景。省掉一次 BitBlt + 4 次 AlphaBlend。
                            // 1) 裁剪到脏矩形后画新选区覆盖层（压暗底图/选区还原/边框/尺寸提示）
                            IntersectClipRect(st.dcs.frame as HWND, rb.left, rb.top, rb.right, rb.bottom);
                            draw_overlay(
                                st.dcs.frame as HWND, &st.dcs, w, h,
                                cur.left, cur.top, cur.right, cur.bottom,
                            );
                            SelectClipRgn(st.dcs.frame as HWND, std::ptr::null_mut());
                            // 2) 仅把脏矩形上屏
                            BitBlt(hdc, rb.left, rb.top, rbw, rbh, st.dcs.frame as HWND, rb.left, rb.top, SRCCOPY);
                        }
                        None => {
                            // 拖拽首帧：全屏合成一次（之后都走脏矩形）
                            compose_frame(&st.dcs, w, h, Some((cur.left, cur.top, cur.right, cur.bottom)));
                            BitBlt(hdc, 0, 0, w, h, st.dcs.frame as HWND, 0, 0, SRCCOPY);
                        }
                    }
                    st.prev_rect = Some(cur);
                } else {
                    // 未拖拽（初始/结束）：全屏合成一次
                    compose_frame(&st.dcs, w, h, None);
                    BitBlt(hdc, 0, 0, w, h, st.dcs.frame as HWND, 0, 0, SRCCOPY);
                }
            }
            EndPaint(hwnd, &ps);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lparam_xy(lparam);
            let mut st_guard = STATE.lock().unwrap();
            if let Some(st) = st_guard.as_mut() {
                st.dragging = true;
                st.start_x = x;
                st.start_y = y;
                st.cur_x = x;
                st.cur_y = y;
                st.prev_rect = None; // 下一帧 WM_PAINT 走全屏合成首帧
            }
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_xy(lparam);
            let mut st_guard = STATE.lock().unwrap();
            if let Some(st) = st_guard.as_mut() {
                if st.dragging {
                    st.cur_x = x.clamp(0, st.w - 1);
                    st.cur_y = y.clamp(0, st.h - 1);
                    drop(st_guard);
                    // 只标记重绘即可；消息循环下一轮处理 WM_PAINT（合成→单次上屏）
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lparam);
            let rect;
            {
                let mut st_guard = STATE.lock().unwrap();
                if let Some(st) = st_guard.as_mut() {
                    st.cur_x = x;
                    st.cur_y = y;
                    st.dragging = false;
                }
                let (sx, sy, cx, cy) = st_guard
                    .as_ref()
                    .map(|st| (st.start_x, st.start_y, st.cur_x, st.cur_y))
                    .unwrap_or((0, 0, 0, 0));
                rect = RECT {
                    left: sx.min(cx),
                    top: sy.min(cy),
                    right: sx.max(cx),
                    bottom: sy.max(cy),
                };
            }
            // 过小选区 = 误点，按取消处理
            let valid =
                (rect.right - rect.left) >= MIN_SELECT && (rect.bottom - rect.top) >= MIN_SELECT;
            *RESULT.lock().unwrap() = if valid { Some(rect) } else { None };
            PostQuitMessage(0);
            0
        }
        WM_KEYDOWN => {
            if wparam == 0x1B {
                // VK_ESCAPE：取消
                *RESULT.lock().unwrap() = None;
                PostQuitMessage(0);
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

// —— 覆盖层：选区外 AlphaBlend 压暗 + 品牌色空心边框 + 尺寸提示 ——
unsafe fn draw_overlay(
    hdc: HWND, dcs: &Dcs, screen_w: i32, screen_h: i32,
    x1: i32, y1: i32, x2: i32, y2: i32,
) {
    let l = x1.min(x2);
    let t = y1.min(y2);
    let r = x1.max(x2);
    let b = y1.max(y2);

    // A 优化：一次 BitBlt 铺「预渲染好的全屏压暗底图」，取代原先对选区外 4 块各做一次
    // AlphaBlend（AlphaBlend 逐像素混合，比 BitBlt 慢得多）。
    // 调用方已 IntersectClipRect 到脏矩形，故整屏坐标的 BitBlt 实际只刷脏区那点像素。
    // 附带好处：压暗底图整块覆盖，自动清掉上一帧的边框/标签残留，不会累积变暗。
    BitBlt(hdc, 0, 0, screen_w, screen_h, dcs.dimmed as HWND, 0, 0, SRCCOPY);
    // 选区内部还原为未压暗的清晰背景（第二次 BitBlt）
    if r > l && b > t {
        BitBlt(hdc, l, t, r - l, b - t, dcs.mem as HWND, l, t, SRCCOPY);
    }

    // 品牌色空心边框（2px）
    let pen = CreatePen(0 /*PS_SOLID*/, 2, ACCENT_COLORREF);
    let old_pen = SelectObject(hdc, pen);
    let old_brush = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    windows_sys::Win32::Graphics::Gdi::Rectangle(hdc, l, t, r + 1, b + 1);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    DeleteObject(pen);

    // 尺寸提示（选区左上角上方；贴顶则画在框内）
    let label = format!("{} × {}", r - l, b - t);
    let wide = to_wide(&label);
    let mut text_rect = RECT {
        left: l,
        top: if t >= 28 { t - 26 } else { t + 6 },
        right: l + 120,
        bottom: if t >= 28 { t - 4 } else { t + 28 },
    };
    SetBkMode(hdc, TRANSPARENT as i32);
    SetTextColor(hdc, ACCENT_COLORREF);
    DrawTextW(hdc, wide.as_ptr(), wide.len() as i32, &mut text_rect, 0x100); // DT_NOCLIP
}

fn lparam_xy(lparam: isize) -> (i32, i32) {
    let l = lparam as i32;
    ((l as i16) as i32, ((l >> 16) as i16) as i32)
}

/// 由起点/当前点归一化出标准 RECT（left<=right, top<=bottom）
fn norm_rect(x0: i32, y0: i32, x1: i32, y1: i32) -> RECT {
    RECT {
        left: x0.min(x1),
        top: y0.min(y1),
        right: x0.max(x1),
        bottom: y0.max(y1),
    }
}

/// 尺寸标签所占区域（与 draw_overlay 中 DrawTextW 的 text_rect 保持一致）：
/// 画在选区左上角上方；贴顶时改画在框内。水平只占 left..left+130（标签 120 宽 + 余量）。
fn label_rect(r: RECT) -> RECT {
    let top = if r.top >= 28 { r.top - 26 } else { r.top + 6 };
    let bottom = if r.top >= 28 { r.top - 4 } else { r.top + 28 };
    RECT { left: r.left, top, right: r.left + 130, bottom }
}

/// B 优化：脏矩形 = {prev 选区, cur 选区, prev 标签区, cur 标签区} 的并集，再留 4px 边框余量。
/// 旧实现无条件「右边界 +130」，但标签实际上只出现在选区的**左边缘**；
/// 宽选区时右侧那 130px 完全白刷。改为取 max(选区右边界, 标签右边界) 后，
/// 宽选区可省掉上百像素宽的重绘，且结果始终 <= 旧实现的脏区。
fn dirty_bbox(prev: RECT, cur: RECT, w: i32, h: i32) -> RECT {
    let lp = label_rect(prev);
    let lc = label_rect(cur);
    let l = prev.left.min(cur.left).min(lp.left).min(lc.left) - 4;
    let t = prev.top.min(cur.top).min(lp.top).min(lc.top) - 4;
    let r = prev.right.max(cur.right).max(lp.right).max(lc.right) + 4;
    let b = prev.bottom.max(cur.bottom).max(lp.bottom).max(lc.bottom) + 4;
    RECT {
        left: l.max(0),
        top: t.max(0),
        right: r.min(w),
        bottom: b.min(h),
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ============================================================
// GDI 抓屏（移植自 rust-reference/screenshot.rs::capture_gdi，24bpp RGB）
// ============================================================
fn capture_region_gdi(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>> {
    unsafe {
        let hdc_screen = GetDC(std::ptr::null_mut());
        if hdc_screen.is_null() {
            return Err(anyhow!("GetDC failed"));
        }
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbitmap = CreateCompatibleBitmap(hdc_screen, w, h);
        SelectObject(hdc_mem, hbitmap);
        BitBlt(hdc_mem, 0, 0, w, h, hdc_screen, x, y, SRCCOPY);

        let row_size = ((w * 3 + 3) / 4) * 4;
        let mut pixels = vec![0u8; (row_size * h) as usize];
        // windows-sys 的 BITMAPINFO/BITMAPINFOHEADER 未实现 Default，手动清零 + 逐字段赋值
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // 顶行在前
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        let ok = GetDIBits(
            hdc_mem,
            hbitmap,
            0,
            h as u32,
            pixels.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        DeleteObject(hbitmap);
        DeleteDC(hdc_mem);
        ReleaseDC(std::ptr::null_mut(), hdc_screen);
        if ok == 0 {
            return Err(anyhow!("GetDIBits failed"));
        }

        // 去掉行对齐 padding，输出紧凑 RGB
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for row in 0..h as usize {
            let base = row * row_size as usize;
            for col in 0..w as usize {
                let i = base + col * 3;
                rgb.extend_from_slice(&[pixels[i], pixels[i + 1], pixels[i + 2]]);
            }
        }
        Ok(rgb)
    }
}

fn save_png(rgb: &[u8], w: i32, h: i32) -> Result<PathBuf> {
    use image::ImageBuffer;

    let img: ImageBuffer<image::Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_raw(w as u32, h as u32, rgb.to_vec())
            .ok_or_else(|| anyhow!("invalid image data"))?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    let dir = std::env::temp_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("winocr_capture_{ts}.png"));
    let mut writer = std::io::BufWriter::new(std::fs::File::create(&path)?);
    img.write_to(&mut writer, image::ImageFormat::Png)?;
    Ok(path)
}
