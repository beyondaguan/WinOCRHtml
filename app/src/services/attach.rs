// ============================================================
// Services · 划词（全局选区监听）P3-3
// 架构（重要）：
//   线程 A（winocr-selection-hook）：安装 WH_MOUSE_LL 钩子 + GetMessageW 消息循环。
//       钩子回调里【只做极轻量判定】（WH_MOUSE_LL 回调超时会被系统静默卸载，
//       禁止在回调里做剪贴板/延迟/翻译等耗时操作），命中即 try_send 到 channel 立刻返回。
//   线程 B（winocr-selection-work）：消费 channel，做「延时 → 保存剪贴板 → 模拟 Ctrl+C
//       → 读剪贴板 → 恢复剪贴板」，取到非空文本才回调 on_event。
//   两个线程均为【进程生命周期常驻】，不做 stop（开关走 set_enabled）。
//   非 Windows 平台整体 no-op（本工程主要跑 Windows，但保持可编译）。
//
// 注意：本文件是**从零实现**。原先注释写的「移植来源 rust-reference/selection_window.rs」
// 是错的：rust-reference/src/selection_window.rs 实为**截图选框窗**（select_region），
// 已被移植进 capture.rs；全仓库没有桌面原生划词实现，因此这里没有任何可参考的原始代码，
// 唯一借用的是 rust-reference/src/window.rs 里剪贴板 API 的**用法思路**
// （OpenClipboard / GetClipboardData / SetClipboardData / GlobalLock），
// 但那是老版 winapi crate，符号名与本工程 windows-sys 完全不同，未照抄任何类型名。
// ============================================================
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;

#[cfg(windows)]
use std::cell::{Cell, RefCell};
#[cfg(windows)]
use std::ptr;
#[cfg(windows)]
use std::sync::mpsc::{self, Receiver, SyncSender};
#[cfg(windows)]
use std::sync::Mutex;
#[cfg(windows)]
use std::thread::{self, JoinHandle};
#[cfg(windows)]
use std::time::Duration;

#[cfg(windows)]
use windows_sys::Win32::Foundation::{GlobalFree, HWND, LPARAM, LRESULT, RECT, WPARAM};
#[cfg(windows)]
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(windows)]
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
#[cfg(windows)]
use windows_sys::Win32::System::Ole::CF_UNICODETEXT;
#[cfg(windows)]
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_C, VK_CONTROL,
};
#[cfg(windows)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, FindWindowW, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId,
    SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, SetWindowsHookExW, TranslateMessage,
    UnhookWindowsHookEx, GWL_EXSTYLE, HWND_TOPMOST, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CYSCREEN,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WH_MOUSE_LL, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// 划词命中（已确认取到非空文本）
#[derive(Debug, Clone)]
pub struct SelectionHit {
    /// 选中文本（已 trim，非空）
    pub text: String,
    /// 鼠标抬起时的屏幕坐标（虚拟屏幕坐标，供气泡定位；注意可能是负值，多显示器）
    pub x: i32,
    pub y: i32,
}

/// 上抛给 UI 层的事件
#[derive(Debug, Clone)]
pub enum SelectionEvent {
    /// 拖选并成功取到文本
    Hit(SelectionHit),
    /// 鼠标左键按下（任何位置）。UI 层用它实现「点击气泡外部即隐藏」。
    ClickDown { x: i32, y: i32 },
}

/// 划词监听服务。
///
/// - `start` / `set_enabled` / `enabled` 均取 `&self`，可并行调用（内部为 `AtomicBool`）。
/// - `enabled` 默认 **false**：划词是设置页里的**可选**功能，若默认开启会在用户未知情时
///   对全局拖选模拟 Ctrl+C（复制/覆盖剪贴板），有副作用风险；故必须由 UI 读配置后显式
///   `set_enabled(true)`。
pub struct Attach {
    /// 运行时开关，被钩子回调（线程 A）与工作线程（线程 B）共享读取。
    enabled: Arc<AtomicBool>,
    /// 幂等标记：重复 start 不再起第二组线程。
    #[cfg(windows)]
    started: AtomicBool,
    /// 常驻线程句柄（进程生命周期内不 join，仅持有以避免句柄悬空）。
    #[cfg(windows)]
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Attach {
    pub fn new() -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(false)),
            #[cfg(windows)]
            started: AtomicBool::new(false),
            #[cfg(windows)]
            threads: Mutex::new(Vec::new()),
        }
    }

    /// 运行时开关（设置页「划词翻译」）。false 时钩子仍装但不产生 Hit（ClickDown 也一并停发）。
    /// 钩子回调读的是同一个原子变量，故**立即生效**。
    pub fn set_enabled(&self, v: bool) {
        self.enabled.store(v, Ordering::SeqCst);
        log::info!(
            "[services::attach] 划词监听已{}",
            if v { "启用" } else { "停用" }
        );
    }

    /// 当前是否启用
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// 启动监听（幂等：重复调用不会起第二个线程）。
    /// `on_event` 在**线程 B（winocr-selection-work）**上调用；
    /// 调用方需自行 `slint::invoke_from_event_loop` 投递回 UI 线程。
    #[cfg(windows)]
    pub fn start(&self, on_event: Box<dyn Fn(SelectionEvent) + Send + 'static>) -> Result<()> {
        if self.started.swap(true, Ordering::SeqCst) {
            log::info!("[services::attach] start() 重复调用，忽略（幂等，监听已在运行）");
            return Ok(());
        }
        let r = self.start_inner(on_event);
        if r.is_err() {
            // 安装失败允许稍后重试
            self.started.store(false, Ordering::SeqCst);
        }
        r
    }

    #[cfg(not(windows))]
    pub fn start(&self, _on_event: Box<dyn Fn(SelectionEvent) + Send + 'static>) -> Result<()> {
        log::warn!("[services::attach] 非 Windows 平台：划词监听不可用（no-op）");
        Ok(())
    }

    #[cfg(windows)]
    fn start_inner(&self, on_event: Box<dyn Fn(SelectionEvent) + Send + 'static>) -> Result<()> {
        // SyncSender + try_send：Sender 没有非阻塞发送，钩子回调绝不能阻塞，
        // 容量 64 只是缓冲上限，满了直接丢事件（丢一次划词远比卡住全局鼠标好）。
        let (tx, rx): (SyncSender<RawEvent>, Receiver<RawEvent>) = mpsc::sync_channel(64);
        let (init_tx, init_rx) = mpsc::channel::<Result<()>>();
        let enabled_a = Arc::clone(&self.enabled);

        // ---- 线程 A：装钩子 + 消息循环 ----
        let hook_thread = thread::Builder::new()
            .name("winocr-selection-hook".into())
            .spawn(move || {
                // 钩子回调是 extern "system" 自由函数，无法捕获闭包环境；但 WH_MOUSE_LL
                // 的回调**必然在安装钩子的这个线程上执行**（经由本线程消息队列投递），
                // 因此把「发送端 + 开关」放进本线程的 thread_local，回调即可安全取用，
                // 且不引入全局可变 static。
                HOOK_SINK.with(|slot| {
                    *slot.borrow_mut() = Some(HookSink {
                        tx,
                        enabled: enabled_a,
                    });
                });

                // SetWindowsHookExW 对全局钩子要求 hmod 非空：传本进程 exe 模块句柄。
                let hmod = unsafe { GetModuleHandleW(ptr::null()) };
                let hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), hmod, 0) };
                if hook.is_null() {
                    let e = std::io::Error::last_os_error();
                    HOOK_SINK.with(|slot| *slot.borrow_mut() = None);
                    let _ = init_tx.send(Err(anyhow::anyhow!(
                        "SetWindowsHookExW(WH_MOUSE_LL) 安装失败: {e}"
                    )));
                    return;
                }
                log::info!("[services::attach] WH_MOUSE_LL 钩子已安装，开始消息循环");
                let _ = init_tx.send(Ok(()));

                // 低级鼠标钩子必须由装了钩子的线程持续泵消息，否则系统收不到回调。
                let mut msg: MSG = unsafe { std::mem::zeroed() };
                loop {
                    // GetMessageW：0 = WM_QUIT，-1 = 错误
                    let r = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
                    if r == 0 {
                        break;
                    }
                    if r == -1 {
                        log::error!(
                            "[services::attach] GetMessageW 返回 -1（错误）: {:?}",
                            std::io::Error::last_os_error()
                        );
                        break;
                    }
                    unsafe {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                unsafe { UnhookWindowsHookEx(hook) };
                HOOK_SINK.with(|slot| *slot.borrow_mut() = None);
                log::info!("[services::attach] 钩子消息循环退出，钩子已卸载");
            })?;

        // 等线程 A 报告安装结果，让 start() 的成败是确定的
        match init_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = hook_thread.join();
                return Err(e);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("划词钩子线程未在 3s 内报告安装结果: {e}"));
            }
        }

        // ---- 线程 B：消费事件、取词、回调 ----
        let enabled_b = Arc::clone(&self.enabled);
        let work_thread = thread::Builder::new()
            .name("winocr-selection-work".into())
            .spawn(move || {
                // rx.recv() 阻塞等事件；发送端（线程 A 的 thread_local）在进程生命周期内
                // 一直存活，故这里只在进程退出时才会 Err 而退出循环。
                while let Ok(ev) = rx.recv() {
                    match ev {
                        RawEvent::ClickDown { x, y } => {
                            // 钩子回调已按开关过滤，这里直接转发（用于「点击气泡外部隐藏」）
                            on_event(SelectionEvent::ClickDown { x, y });
                        }
                        RawEvent::DragEnd { x, y } => {
                            // 再查一次开关：从钩子投递到这里存在微小时间窗
                            if !enabled_b.load(Ordering::SeqCst) {
                                continue;
                            }
                            if let Some(text) = capture_selection() {
                                on_event(SelectionEvent::Hit(SelectionHit { text, x, y }));
                            }
                        }
                    }
                }
                log::info!("[services::attach] 取词线程退出");
            })?;

        if let Ok(mut guard) = self.threads.lock() {
            guard.push(hook_thread);
            guard.push(work_thread);
        }
        log::info!("[services::attach] 划词监听已启动（默认停用，等待 set_enabled(true)）");
        Ok(())
    }
}

impl Default for Attach {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 划词气泡浮层窗口适配（供 UI 层调用）
// 这些函数按**窗口标题**查找顶层窗口（`FindWindowW` 精确匹配 lpwindowname）。
// 返回 false = 没找到窗口（窗口可能尚未创建/尚未显示），调用方可稍后重试。
//
// 【已冻结的调用协议】UI 层必须按此顺序，且**永不调用 Slint 的 hide()**：
//   ① let prev = foreground_hwnd();        // 先记下宿主前台窗口
//   ② bubble.show();                       // 首次 show() 会短暂抢焦点
//   ③ overlay_make_nonactivating(TITLE);   // 打 WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW + 置顶
//   ④ overlay_place(TITLE, x, y);          // 定位 + 显示（SWP_SHOWWINDOW），不激活
//   ⑤ restore_foreground(prev);            // 把焦点还给宿主
//   此后每次命中只调 ④；隐藏也走 ④：overlay_place(TITLE, -32000, -32000)。
// 为什么不用 hide()：winit 的 apply_diff 只要样式有 diff 就会用缓存值整体重写
//   GWL_EXSTYLE，把 WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW 清掉并 ShowWindow 抢焦点。
// ============================================================

/// 把标题为 `title` 的顶层窗口设为「不抢焦点 + 不进任务栏 + 置顶」，适用于无边框浮层。
/// 幂等，可重复调用。返回 false = 没找到窗口。非 Windows 恒返回 false。
#[cfg(windows)]
pub fn overlay_make_nonactivating(title: &str) -> bool {
    unsafe {
        let hwnd = find_by_title(title);
        if hwnd.is_null() {
            return false;
        }
        // WS_EX_NOACTIVATE：点击浮层本身也不把焦点从宿主应用抢走（气泡上的按钮仍可点）。
        // WS_EX_TOOLWINDOW：不出现在任务栏/Alt+Tab。
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let new_ex = (ex as u32 | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) as isize;
        if new_ex != ex {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex);
        }
        // 置顶但不激活、不移动、不改尺寸。不在这里强制显示——显示交给 overlay_place。
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        true
    }
}

#[cfg(not(windows))]
pub fn overlay_make_nonactivating(_title: &str) -> bool {
    false
}

/// 把标题为 `title` 的窗口移动到 (x, y) 并置顶显示，**不激活**、不改尺寸。
/// 返回 false = 没找到窗口。非 Windows 恒返回 false。
#[cfg(windows)]
pub fn overlay_place(title: &str, x: i32, y: i32) -> bool {
    unsafe {
        let hwnd = find_by_title(title);
        if hwnd.is_null() {
            return false;
        }
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        true
    }
}

#[cfg(not(windows))]
pub fn overlay_place(_title: &str, _x: i32, _y: i32) -> bool {
    false
}

/// 返回标题为 `title` 的窗口的**实际物理像素**矩形，格式 `(left, top, right, bottom)`。
/// 取不到窗口返回 `None`。非 Windows 恒返回 `None`。
///
/// 用途：Slint 的窗口宽高是【逻辑像素】，高 DPI 下与物理像素不等（125% 缩放时
/// 360 逻辑 px = 450 物理 px）。UI 层**不能**用 Slint 的逻辑尺寸去做「点击气泡外部命中判定」
/// （ui/mod.rs 的矩形包含测试）和「越界回退」，必须问本函数要真实物理矩形，否则高 DPI 下
/// 命中矩形偏小、点气泡右下角会被误判为「外部」而错误收起。
/// 实现：`FindWindowW` → `GetWindowRect`。气泡是 `no-frame: true`（无非客户区），
/// 故该矩形即等于内容区尺寸。
///
/// 注：`SetWindowPos` 由窗口管理器异步处理，刚 `overlay_place` 完立刻调用本函数可能取到旧矩形。
/// 这不影响 UI 的用法——「place 到某位置 → rect 量真实尺寸 → 按真实尺寸再 place 一次」中，
/// 尺寸与位置无关，两次调用之间无需等待。
#[cfg(windows)]
pub fn overlay_rect(title: &str) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let hwnd = find_by_title(title);
        if hwnd.is_null() {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }
        Some((rect.left, rect.top, rect.right, rect.bottom))
    }
}

#[cfg(not(windows))]
pub fn overlay_rect(_title: &str) -> Option<(i32, i32, i32, i32)> {
    None
}

/// 主显示器尺寸（宽, 高）。供 UI 判断气泡是否越界（右下溢出则回退到选区上方/左移）。
/// 用 SM_CXSCREEN/SM_CYSCREEN：主屏像素尺寸，与 SetWindowPos 的绝对坐标同坐标系。
///
/// 【已明确标注的局限 · v1.0.0】本函数返回的是**主屏**尺寸，不是虚拟屏尺寸。因此 UI 侧
/// `px = (sw - BUBBLE_W - 8).max(0)` 这类越界回退，在「第二显示器位于主屏右侧」时会把
/// 气泡拉回主屏。v1.0.0 **有意**只按主屏做越界回退（本机以单屏为主，不值得为此扩大 API 面）；
/// 精确的多显示器回退需改用 SM_XVIRTUALSCREEN / SM_YVIRTUALSCREEN / SM_CXVIRTUALSCREEN /
/// SM_CYVIRTUALSCREEN（注意虚拟屏原点可能为负，届时签名多半要一并带出原点），留待 v1.1。
///
/// 非 Windows 返回 (0, 0)。
#[cfg(windows)]
pub fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

#[cfg(not(windows))]
pub fn screen_size() -> (i32, i32) {
    (0, 0)
}

/// 取当前前台窗口，以 `isize` 形式返回（不向调用方暴露裸指针类型，且天然满足 Send）。
/// 用途：气泡首次 show() 会短暂抢焦点，先记下宿主窗口，show 完再还回去。
///
/// 返回值仅在**本进程内**作为不透明令牌使用（只应原样交回 `restore_foreground`）；
/// **不要**解释为可跨进程传递的 HWND —— 窗口句柄跨进程无意义，`isize` 只是本模块为
/// 避免在 UI 层暴露裸指针而做的编码，并非 Win32 的稳定表示。
///
/// 无前台窗口时返回 0。非 Windows 恒返回 0。
#[cfg(windows)]
pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow() as isize }
}

#[cfg(not(windows))]
pub fn foreground_hwnd() -> isize {
    0
}

/// 把前台焦点还给 `prev`（`foreground_hwnd()` 的返回值）。
/// `prev == 0` 时 no-op 并返回 false。返回 false 亦表示 SetForegroundWindow 调用失败。
/// 非 Windows 恒返回 false。
///
/// 说明：SetForegroundWindow 受系统「前台锁定」约束——只有当前台窗口属于本进程时才允许
/// 切换。协议里是「本进程 show() 刚抢到前台 → 立刻切走」，满足该前提，正常会成功；
/// 若宿主应用是管理员权限等特殊情形导致失败，返回 false 属可接受的降级（气泡仍可用）。
#[cfg(windows)]
pub fn restore_foreground(prev: isize) -> bool {
    if prev == 0 {
        return false;
    }
    unsafe { SetForegroundWindow(prev as HWND) != 0 }
}

#[cfg(not(windows))]
pub fn restore_foreground(_prev: isize) -> bool {
    false
}

/// 把文本写入系统剪贴板（供 UI 层使用，例如划词气泡的「复制译文」）。
///
/// 与取词流程共用同一套剪贴板 helper（内部即 `clipboard_set_text`，封装不外露）；
/// `OpenClipboard` 自带重试，但剪贴板可能被其他进程长时间锁定，故仍有失败可能 ——
/// 返回 false 由调用方降级提示，**不要 panic、不要报错**，失败时内部已 `log::warn!` 记一行。
///
/// 空串按现有 helper 语义处理（写入一个仅含 NUL 的 CF_UNICODETEXT）。
/// 非 Windows 恒返回 false。
#[cfg(windows)]
pub fn copy_text_to_clipboard(text: &str) -> bool {
    clipboard_set_text(text)
}

#[cfg(not(windows))]
pub fn copy_text_to_clipboard(_text: &str) -> bool {
    false
}

#[cfg(windows)]
unsafe fn find_by_title(title: &str) -> HWND {
    let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    // lpclassname = null（不限定类名），lpwindowname = 标题（精确匹配）
    FindWindowW(ptr::null(), wide.as_ptr())
}

// ============================================================
// 以下均为 Windows 实现细节
// ============================================================

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
enum RawEvent {
    ClickDown { x: i32, y: i32 },
    DragEnd { x: i32, y: i32 },
}

#[cfg(windows)]
struct HookSink {
    tx: SyncSender<RawEvent>,
    enabled: Arc<AtomicBool>,
}

#[cfg(windows)]
thread_local! {
    // 钩子回调与消息循环同线程 → 用 thread_local 传递「发送端 + 开关」，避免全局可变 static
    static HOOK_SINK: RefCell<Option<HookSink>> = RefCell::new(None);
    // 记录 mousedown 位置，用于在 mouseup 时判定「拖选 vs 单击」
    static DOWN_POS: Cell<Option<(i32, i32)>> = Cell::new(None);
}

/// 低级鼠标钩子回调。
///
/// 铁律：本回调**必须极快返回**——WH_MOUSE_LL 的钩子超时后系统会静默摘掉钩子
/// （且大概率不再恢复），导致划词功能彻底失效。因此这里只做：
/// 读坐标 → 与 mousedown 比位移 → try_send（非阻塞）→ 立刻返回。
/// 剪贴板/延时/Ctrl+C/翻译等一律放到线程 B。
#[cfg(windows)]
unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // code < 0：系统保留值，必须无条件透传，禁止处理
    if code < 0 {
        return CallNextHookEx(ptr::null_mut(), code, wparam, lparam);
    }
    let msg = wparam as u32;
    if msg == WM_LBUTTONDOWN || msg == WM_LBUTTONUP {
        let info = lparam as *const MSLLHOOKSTRUCT;
        if !info.is_null() {
            let pt = (*info).pt;
            HOOK_SINK.with(|slot| {
                // try_borrow：万一重入（不应发生），宁可不处理也不 panic
                if let Ok(borrow) = slot.try_borrow() {
                    if let Some(sink) = borrow.as_ref() {
                        // 开关关闭时连 ClickDown 也停发（UI 需求：关了就完全不打扰）
                        if !sink.enabled.load(Ordering::SeqCst) {
                            return;
                        }
                        if msg == WM_LBUTTONDOWN {
                            DOWN_POS.with(|p| p.set(Some((pt.x, pt.y))));
                            // try_send 非阻塞：缓冲满就丢事件，绝不阻塞钩子线程
                            let _ = sink.tx.try_send(RawEvent::ClickDown { x: pt.x, y: pt.y });
                        } else {
                            // mouseup：与 mousedown 位移 >= 6px 才视为拖选（单击抖动通常 0-2px）
                            let down = DOWN_POS.with(|p| p.take());
                            if let Some((dx, dy)) = down {
                                if (pt.x - dx).abs() >= 6 || (pt.y - dy).abs() >= 6 {
                                    let _ =
                                        sink.tx.try_send(RawEvent::DragEnd { x: pt.x, y: pt.y });
                                }
                            }
                        }
                    }
                }
            });
        }
    }
    // 无论是否命中都要透传：绝不吞掉用户的鼠标事件
    CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
}

/// 线程 B 的取词流水线：确认是外部应用的前景窗口 → 延时让宿主完成选区绘制 →
/// 用「清空剪贴板做哨兵」的方式模拟 Ctrl+C 并读回选中文本 → 还原用户原剪贴板。
///
/// 返回 `Some(trim 后非空文本)` 才算命中；否则 `None`（不弹气泡）。
#[cfg(windows)]
fn capture_selection() -> Option<String> {
    // 1) 若前景窗口属于本进程，跳过：避免用户在我们自己的主窗口/气泡里拖选也弹气泡。
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.is_null() {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(fg, &mut pid);
            if pid == std::process::id() {
                log::info!("[services::attach] 前景窗口属于本进程，跳过取词");
                return None;
            }
        }
    }

    // 2) 120ms：鼠标抬起后，宿主应用还需要一点时间完成选区绘制与内部状态更新
    //    （尤其是浏览器/Electron 这类合成选区），过早 Ctrl+C 可能复制到上一次的内容。
    thread::sleep(Duration::from_millis(120));

    // 3) 保存用户原剪贴板文本（可能为 None = 原本没有文本），稍后必须还原。
    let orig = clipboard_get_text();

    // 4) 清空剪贴板作为**哨兵**：若接下来的 Ctrl+C 没生效，剪贴板会保持为空，
    //    我们就能区分「真的没有选中」与「复制到了残留的旧文本」，避免误弹气泡。
    clipboard_clear();

    // 5) 30ms：释放剪贴板占用。上一步 CloseClipboard 后，目标应用才能拿到剪贴板所有权。
    thread::sleep(Duration::from_millis(30));

    // 6) 模拟 Ctrl+C（Ctrl↓ C↓ C↑ Ctrl↑）
    send_ctrl_c();

    // 7) 80ms：等目标应用把选中文本写进剪贴板（复制是异步的，跨进程还要排队）。
    thread::sleep(Duration::from_millis(80));

    // 8) 读回
    let sel = clipboard_get_text();

    // 9) 还原用户原剪贴板（不打扰用户的复制历史）。
    //    还原失败不影响本次取词结果，故忽略返回值（我们只关心「有没有取到选中文本」）。
    match &orig {
        Some(t) => {
            let _ = clipboard_set_text(t);
        }
        None => clipboard_clear(),
    }

    // 10) 过滤：trim 后非空才算命中，否则丢弃（不弹气泡）
    match sel {
        Some(t) if !t.trim().is_empty() => Some(t.trim().to_string()),
        _ => None,
    }
}

/// 模拟 Ctrl+C 按键序列。
///
/// 用 SendInput（现代推荐 API，较 keybd_event 更不易被部分应用过滤）。
/// windows-sys 0.59 的 `INPUT` 用 `Anonymous: INPUT_0` union 承载按键信息，
/// 构造 `INPUT_0 { ki: KEYBDINPUT { .. } }` 即可，并不别扭，故无需降级到 keybd_event。
#[cfg(windows)]
fn send_ctrl_c() {
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // 按下顺序：Ctrl↓ C↓ C↑ Ctrl↑ —— 保证「Ctrl 仍按住时发 C」，反之会变成单个 c
    let inputs = [
        key(VK_CONTROL, false),
        key(VK_C, false),
        key(VK_C, true),
        key(VK_CONTROL, true),
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        log::warn!(
            "[services::attach] SendInput(Ctrl+C) 仅注入 {sent}/{} 个事件",
            inputs.len()
        );
    }
}

// ---- 剪贴板 helper（CF_UNICODETEXT）----

/// OpenClipboard 重试：别的进程可能短暂持有剪贴板，退避 ~5 次（每次 15ms）后再放弃。
#[cfg(windows)]
fn clipboard_open_retry() -> bool {
    for _ in 0..5 {
        if unsafe { OpenClipboard(ptr::null_mut()) } != 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(15));
    }
    log::warn!("[services::attach] OpenClipboard 重试 5 次仍失败，剪贴板本次不可用");
    false
}

/// 读取剪贴板文本；失败或无文本返回 None。
#[cfg(windows)]
fn clipboard_get_text() -> Option<String> {
    if !clipboard_open_retry() {
        return None;
    }
    let mut out = None;
    unsafe {
        let h = GetClipboardData(CF_UNICODETEXT as u32);
        if !h.is_null() {
            let p = GlobalLock(h) as *const u16;
            if !p.is_null() {
                // CF_UNICODETEXT 是 NUL 结尾的 UTF-16 串，先求长度
                let mut len = 0usize;
                while *p.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(p, len);
                out = Some(String::from_utf16_lossy(slice));
                GlobalUnlock(h);
            }
        }
        CloseClipboard();
    }
    out
}

/// 把文本以 CF_UNICODETEXT 写入剪贴板（会先清空）。
/// 返回 false = 打开剪贴板 / 内存分配 / SetClipboardData 任一步失败
/// （调用方自行降级提示即可，本函数不 panic、不报错）。
#[cfg(windows)]
fn clipboard_set_text(text: &str) -> bool {
    if !clipboard_open_retry() {
        return false;
    }
    unsafe {
        EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if h.is_null() {
            CloseClipboard();
            return false;
        }
        let p = GlobalLock(h) as *mut u16;
        if p.is_null() {
            // 分配成功但加锁失败：句柄仍归我们，必须自己释放
            GlobalFree(h);
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        GlobalUnlock(h);
        // 关键：SetClipboardData 成功后所有权**移交系统**，绝不能再 GlobalFree；
        // 只有失败（返回 null）时句柄仍归我们，才需要释放。
        let ok = !SetClipboardData(CF_UNICODETEXT as u32, h).is_null();
        if !ok {
            GlobalFree(h);
        }
        CloseClipboard();
        ok
    }
}

/// 清空剪贴板（用于取词前的哨兵清空，以及原剪贴板无文本时的还原）。
#[cfg(windows)]
fn clipboard_clear() {
    if !clipboard_open_retry() {
        return;
    }
    unsafe {
        EmptyClipboard();
        CloseClipboard();
    }
}
