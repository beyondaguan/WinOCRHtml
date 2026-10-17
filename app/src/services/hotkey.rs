// ============================================================
// Services · 全局热键（真实实现）
// 移植自 rust-reference/hotkey.rs 核心路径：RegisterHotKey + 线程内消息循环。
// 热键表（2026-10-14 P0-1 扩展，原仅 Alt+Q）：
//   Alt+Q / Alt+T → 截图翻译（同动作，左右手均可）
//   Alt+D         → 显示/置前主窗口
//   Alt+W         → 开/关划词取词
// 扩展位：配置驱动热键表（Ctrl+Shift+V 粘贴翻译等）随 config 服务接入。
// ============================================================
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{anyhow, Result};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_HOTKEY,
};

/// 热键 ID：截图（对应 UI 的 capture 回调）
pub const HOTKEY_ID_CAPTURE: i32 = 1;
/// 热键 ID：截图第二键（Alt+T，与 Alt+Q 同动作；ID 错开 10 避免 match 模式内算术）
pub const HOTKEY_ID_CAPTURE_ALT: i32 = HOTKEY_ID_CAPTURE + 10;
/// 热键 ID：开关划词取词
pub const HOTKEY_ID_TOGGLE_SELECTION: i32 = 2;
/// 热键 ID：显示/置前主窗口
pub const HOTKEY_ID_SHOW_WINDOW: i32 = 3;

/// 热键触发的动作（经 on_trigger 报给调用方）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Capture,
    ToggleSelection,
    ShowWindow,
}

/// 'Q' / 'T' / 'W' / 'D' 的虚拟键码
const VK_Q: u32 = 0x51;
const VK_T: u32 = 0x54;
const VK_W: u32 = 0x57;
const VK_D: u32 = 0x44;

pub struct Hotkey {
    _handle: Option<JoinHandle<()>>,
}

/// 单个热键注册（失败仅返回 Err，不 panic；由调用方决定是否致命）
unsafe fn register_one(id: i32, mods: u32, vk: u32) -> Result<()> {
    // null HWND = 绑定当前线程消息队列
    if RegisterHotKey(std::ptr::null_mut(), id, mods, vk) == 0 {
        return Err(anyhow!("RegisterHotKey(id={id}, mods={mods:#x}, vk={vk:#x}) 失败：热键可能已被占用"));
    }
    Ok(())
}

/// 注册划词开关热键，带回退链：Alt+W → Ctrl+Alt+W → Shift+Alt+W。
/// 返回实际生效的组合键名（全部失败返回 None）。
unsafe fn register_toggle_selection() -> Option<&'static str> {
    const CHAIN: &[(&str, u32, u32)] = &[
        ("Alt+W", MOD_ALT, VK_W),
        ("Ctrl+Alt+W", MOD_CONTROL | MOD_ALT, VK_W),
        ("Shift+Alt+W", MOD_SHIFT | MOD_ALT, VK_W),
    ];
    for (name, mods, vk) in CHAIN {
        match register_one(HOTKEY_ID_TOGGLE_SELECTION, *mods, *vk) {
            Ok(()) => {
                log::info!("[services::hotkey] {name} registered (toggle-selection)");
                return Some(name);
            }
            Err(e) => log::warn!("[services::hotkey] {name} 注册失败: {e:#}"),
        }
    }
    log::warn!("[services::hotkey] 划词开关热键全部被占用，未注册（功能仍可在设置页开关）");
    None
}

impl Hotkey {
    /// 注册全局热键（Alt+Q/T 截图、划词开关（Alt+W 带回退链）、Alt+D 唤起主窗）并启动消息循环线程。
    ///
    /// - `on_trigger`：热键触发时在**热键线程**上调用（内部需自行投递回 UI 线程），
    ///   参数指明是哪个动作
    /// - `on_registered`：注册流程结束后调用一次（同样在热键线程上），
    ///   参数为热键摘要文案（供状态条显示实际生效的组合键）。
    ///   注意：Alt+Q 注册失败视为致命（多半已有 WinOCR 实例在跑）→ 返回 Err；
    ///   其余键失败仅记日志、不影响其余热键。
    ///
    /// 注意：RegisterHotKey 会把热键绑定到调用线程的消息队列，
    /// 因此注册与 GetMessage 必须在同一线程——都在本服务内部线程上执行，
    /// 注册结果经 channel 回传给调用方。
    pub fn start(
        on_trigger: Box<dyn Fn(HotkeyAction) + Send + 'static>,
        on_registered: Box<dyn Fn(String) + Send + 'static>,
    ) -> Result<Self> {
        let (tx, rx) = mpsc::channel::<Result<()>>();

        let handle = std::thread::Builder::new()
            .name("winocr-hotkey".into())
            .spawn(move || unsafe {
                // Alt+Q（截图）：失败视为致命（与历史行为一致）
                if let Err(e) = register_one(HOTKEY_ID_CAPTURE, MOD_ALT, VK_Q) {
                    let _ = tx.send(Err(anyhow!(
                        "RegisterHotKey(Alt+Q) 失败：热键可能已被占用（如已有 WinOCR 实例在运行）：{e:#}"
                    )));
                    return;
                }
                log::info!("[services::hotkey] Alt+Q registered (capture)");
                // Alt+T（截图，同动作）：失败不阻塞
                if let Err(e) = register_one(HOTKEY_ID_CAPTURE_ALT, MOD_ALT, VK_T) {
                    log::warn!("[services::hotkey] Alt+T 注册失败，跳过: {e:#}");
                } else {
                    log::info!("[services::hotkey] Alt+T registered (capture)");
                }
                // 划词开关：带回退链，取实际生效的组合键
                let sel_key = register_toggle_selection().unwrap_or("划词键(被占用)");
                // Alt+D（唤起主窗）：失败不阻塞
                if let Err(e) = register_one(HOTKEY_ID_SHOW_WINDOW, MOD_ALT, VK_D) {
                    log::warn!("[services::hotkey] Alt+D 注册失败，跳过: {e:#}");
                } else {
                    log::info!("[services::hotkey] Alt+D registered (show-window)");
                }

                let _ = tx.send(Ok(()));
                on_registered(format!(
                    "● 模型已就绪 · Alt+Q/Alt+T 截图 · {sel_key} 划词 · Alt+D 唤起"
                ));

                let mut msg: MSG = std::mem::zeroed();
                loop {
                    // GetMessageW：0 = WM_QUIT，-1 = 错误，均退出循环
                    let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                    if r == 0 || r == -1 {
                        break;
                    }
                    if msg.message == WM_HOTKEY {
                        let id = msg.wParam as i32;
                        let action = match id {
                            HOTKEY_ID_CAPTURE | HOTKEY_ID_CAPTURE_ALT => HotkeyAction::Capture,
                            HOTKEY_ID_TOGGLE_SELECTION => HotkeyAction::ToggleSelection,
                            HOTKEY_ID_SHOW_WINDOW => HotkeyAction::ShowWindow,
                            _ => HotkeyAction::Capture,
                        };
                        log::info!("[services::hotkey] hotkey id={id} triggered ({action:?})");
                        on_trigger(action);
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID_CAPTURE);
                UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID_CAPTURE_ALT);
                UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID_TOGGLE_SELECTION);
                UnregisterHotKey(std::ptr::null_mut(), HOTKEY_ID_SHOW_WINDOW);
                log::info!("[services::hotkey] message loop exited, hotkeys unregistered");
            })?;

        // 等注册结果，让调用方立刻知道成败
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(r) => r.map(|_| Self { _handle: Some(handle) }),
            Err(e) => Err(anyhow!("hotkey thread did not report: {e}")),
        }
    }
}
