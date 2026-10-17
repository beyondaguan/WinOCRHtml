//! Global Hotkey Registration and Listener (Windows RegisterHotKey)
//! Supports Ctrl+Shift+M for capture, Ctrl+Alt+Q for quit
//! Enhanced: F1-F12, number keys, special keys support

use crate::config::Config;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use winapi::um::winuser::{
    DispatchMessageW, GetMessageW, RegisterHotKey, TranslateMessage, UnregisterHotKey, MSG,
    WM_HOTKEY,
};
use winapi::um::winuser::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

/// Hotkey IDs
pub const HOTKEY_ID_CAPTURE: u32 = 1;
pub const HOTKEY_ID_QUIT: u32 = 2;
pub const HOTKEY_ID_TRANSLATE: u32 = 3;
pub const HOTKEY_ID_SETTINGS: u32 = 4;
pub const HOTKEY_ID_PAUSE: u32 = 5;
pub const HOTKEY_ID_SILENT: u32 = 6;
pub const HOTKEY_ID_MAIN: u32 = 7;
pub const HOTKEY_ID_INPUT: u32 = 8;
pub const HOTKEY_ID_READ: u32 = 9;
pub const HOTKEY_ID_COPY: u32 = 10;
pub const HOTKEY_ID_REPLACE: u32 = 11;

/// 暂停时仍保留的热键（不可被卸载）：退出 + 暂停自身
const PERSISTENT_IDS: [u32; 2] = [HOTKEY_ID_QUIT, HOTKEY_ID_PAUSE];
/// 暂停时会被卸载、恢复时重新注册的功能热键
const FUNCTIONAL_IDS: [u32; 9] = [
    HOTKEY_ID_CAPTURE,
    HOTKEY_ID_TRANSLATE,
    HOTKEY_ID_SETTINGS,
    HOTKEY_ID_SILENT,
    HOTKEY_ID_MAIN,
    HOTKEY_ID_INPUT,
    HOTKEY_ID_READ,
    HOTKEY_ID_COPY,
    HOTKEY_ID_REPLACE,
];

/// Hotkey event types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HotkeyEvent {
    Capture,
    Quit,
    Translate,
    Settings,
    Pause,
    Silent,
    OpenMain,
    InputTranslate,
    Read,
    Copy,
    Replace,
}

/// Parsed hotkey information
#[derive(Debug, Clone, Copy)]
pub struct Hotkey {
    pub modifiers: u32,
    pub key_code: u16,
    pub id: u32,
}

/// Hotkey manager for registration/unregistration
pub struct HotkeyManager {
    hotkeys: Vec<Hotkey>,
    rx: Receiver<HotkeyEvent>,
    _thread: JoinHandle<()>,
    paused: bool,
}

impl HotkeyManager {
    /// Create and register hotkeys from config
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let hotkeys = Self::build_hotkeys(config);

        // Create channel for hotkey events
        let (tx, rx) = channel::<HotkeyEvent>();
        let tx = Arc::new(Mutex::new(tx));

        // 将热键数据移动到线程中
        let hotkeys_clone = hotkeys.clone();

        // Start listener thread (注册热键和消息循环都在同一线程)
        let thread = thread::spawn(move || {
            // 在线程中注册热键
            for hk in &hotkeys_clone {
                unsafe {
                    if RegisterHotKey(
                        std::ptr::null_mut(),
                        hk.id as i32,
                        hk.modifiers,
                        hk.key_code as u32,
                    ) == 0
                    {
                        log::warn!(
                            "Failed to register hotkey {} (modifiers=0x{:X}, key=0x{:X}), may be in use",
                            hk.id,
                            hk.modifiers,
                            hk.key_code
                        );
                    } else {
                        log::info!(
                            "Hotkey registered: id={}, modifiers=0x{:X}, key=0x{:X}",
                            hk.id,
                            hk.modifiers,
                            hk.key_code
                        );
                    }
                }
            }

            let mut msg: MSG = unsafe { std::mem::zeroed() };
            loop {
                unsafe {
                    let result = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
                    if result == 0 || result == -1 {
                        break; // WM_QUIT or error
                    }
                    if msg.message == WM_HOTKEY {
                        let id = msg.wParam as u32;
                        if let Some(tx) = tx.lock().ok() {
                            let event = match id {
                                HOTKEY_ID_CAPTURE => Some(HotkeyEvent::Capture),
                                HOTKEY_ID_QUIT => Some(HotkeyEvent::Quit),
                                HOTKEY_ID_TRANSLATE => Some(HotkeyEvent::Translate),
                                HOTKEY_ID_SETTINGS => Some(HotkeyEvent::Settings),
                                HOTKEY_ID_PAUSE => Some(HotkeyEvent::Pause),
                                HOTKEY_ID_SILENT => Some(HotkeyEvent::Silent),
                                HOTKEY_ID_MAIN => Some(HotkeyEvent::OpenMain),
                                HOTKEY_ID_INPUT => Some(HotkeyEvent::InputTranslate),
                                HOTKEY_ID_READ => Some(HotkeyEvent::Read),
                                HOTKEY_ID_COPY => Some(HotkeyEvent::Copy),
                                HOTKEY_ID_REPLACE => Some(HotkeyEvent::Replace),
                                _ => None,
                            };
                            if let Some(evt) = event {
                                let _ = tx.send(evt);
                            }
                        }
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });

        Ok(Self {
            hotkeys,
            rx,
            _thread: thread,
            paused: false,
        })
    }

    /// Build hotkey list from config (capture / quit / settings)
    fn build_hotkeys(config: &Config) -> Vec<Hotkey> {
        vec![
            Hotkey {
                modifiers: parse_modifiers(&config.hotkey),
                key_code: parse_key(&config.hotkey),
                id: HOTKEY_ID_CAPTURE,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.quit_hotkey),
                key_code: parse_key(&config.quit_hotkey),
                id: HOTKEY_ID_QUIT,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.settings_hotkey),
                key_code: parse_key(&config.settings_hotkey),
                id: HOTKEY_ID_SETTINGS,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.pause_hotkey),
                key_code: parse_key(&config.pause_hotkey),
                id: HOTKEY_ID_PAUSE,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.silent_hotkey),
                key_code: parse_key(&config.silent_hotkey),
                id: HOTKEY_ID_SILENT,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.main_hotkey),
                key_code: parse_key(&config.main_hotkey),
                id: HOTKEY_ID_MAIN,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.input_hotkey),
                key_code: parse_key(&config.input_hotkey),
                id: HOTKEY_ID_INPUT,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.read_hotkey),
                key_code: parse_key(&config.read_hotkey),
                id: HOTKEY_ID_READ,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.copy_hotkey),
                key_code: parse_key(&config.copy_hotkey),
                id: HOTKEY_ID_COPY,
            },
            Hotkey {
                modifiers: parse_modifiers(&config.replace_hotkey),
                key_code: parse_key(&config.replace_hotkey),
                id: HOTKEY_ID_REPLACE,
            },
        ]
    }

    /// 热更新：注销旧热键并按新配置重新注册
    /// 返回 Err(失败的热键描述) 供上层提示
    pub fn reconfigure(&mut self, config: &Config) -> Result<(), String> {
        let new_hotkeys = Self::build_hotkeys(config);

        // 先注销全部旧热键
        unsafe {
            for hk in &self.hotkeys {
                UnregisterHotKey(std::ptr::null_mut(), hk.id as i32);
            }
        }

        // 注册新热键（失败则回滚到旧配置）
        let mut failed: Vec<String> = Vec::new();
        for hk in &new_hotkeys {
            unsafe {
                if RegisterHotKey(
                    std::ptr::null_mut(),
                    hk.id as i32,
                    hk.modifiers,
                    hk.key_code as u32,
                ) == 0
                {
                    failed.push(format!("id={}(0x{:X}+0x{:X})", hk.id, hk.modifiers, hk.key_code));
                }
            }
        }

        if failed.is_empty() {
            self.hotkeys = new_hotkeys;
            log::info!("Hotkeys reconfigured OK");
            Ok(())
        } else {
            // 回滚：注销刚才注册成功的新键，恢复旧键
            unsafe {
                for hk in &new_hotkeys {
                    UnregisterHotKey(std::ptr::null_mut(), hk.id as i32);
                }
                for hk in &self.hotkeys {
                    RegisterHotKey(
                        std::ptr::null_mut(),
                        hk.id as i32,
                        hk.modifiers,
                        hk.key_code as u32,
                    );
                }
            }
            Err(failed.join(", "))
        }
    }

    /// Get receiver for hotkey events
    pub fn receiver(&self) -> &Receiver<HotkeyEvent> {
        &self.rx
    }

    /// 卸载功能热键（截图/翻译/设置），保留退出与暂停热键
    fn unregister_functional(&self) {
        unsafe {
            for hk in &self.hotkeys {
                if FUNCTIONAL_IDS.iter().any(|&id| id == hk.id) {
                    UnregisterHotKey(std::ptr::null_mut(), hk.id as i32);
                }
            }
        }
    }

    /// 重新注册功能热键（按当前 hotkeys 列表）
    fn register_functional(&self) {
        unsafe {
            for hk in &self.hotkeys {
                if FUNCTIONAL_IDS.iter().any(|&id| id == hk.id) {
                    let r = RegisterHotKey(
                        std::ptr::null_mut(),
                        hk.id as i32,
                        hk.modifiers,
                        hk.key_code as u32,
                    );
                    if r == 0 {
                        log::warn!("恢复热键 id={} 失败（可能与其他程序冲突）", hk.id);
                    }
                }
            }
        }
    }

    /// 暂停：卸载功能热键，仅保留退出/暂停（三态模型，§15.4 / §16.6）
    pub fn pause(&mut self) {
        if self.paused {
            return;
        }
        self.unregister_functional();
        self.paused = true;
        log::info!("已暂停：截图/翻译/设置热键失效，退出与暂停热键保留");
    }

    /// 恢复：重新注册功能热键
    pub fn resume(&mut self) {
        if !self.paused {
            return;
        }
        self.register_functional();
        self.paused = false;
        log::info!("已恢复：功能热键重新生效");
    }

    /// 切换暂停/恢复，返回切换后的暂停状态（true=已暂停）
    pub fn toggle_pause(&mut self) -> bool {
        if self.paused {
            self.resume();
        } else {
            self.pause();
        }
        self.paused
    }

    /// 是否处于暂停态
    pub fn is_paused(&self) -> bool {
        self.paused
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        // Unregister hotkeys on drop
        for hk in &self.hotkeys {
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), hk.id as i32);
            }
        }
    }
}

/// Parse modifier keys from hotkey string
fn parse_modifiers(hotkey: &str) -> u32 {
    let mut modifiers: u32 = 0;
    let parts: Vec<&str> = hotkey.split('+').collect();

    for part in &parts {
        let part = part.trim().to_lowercase();
        match part.as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL as u32,
            "shift" => modifiers |= MOD_SHIFT as u32,
            "alt" => modifiers |= MOD_ALT as u32,
            "win" | "meta" => modifiers |= MOD_WIN as u32,
            _ => {} // Not a modifier
        }
    }

    modifiers
}

/// Parse main key from hotkey string (returns virtual key code)
fn parse_key(hotkey: &str) -> u16 {
    let parts: Vec<&str> = hotkey.split('+').collect();
    let last_part = parts.last().unwrap_or(&"").trim().to_lowercase();

    // Function keys F1-F12
    if last_part.starts_with('f') && last_part.len() <= 3 {
        if let Ok(n) = last_part[1..].parse::<u16>() {
            if n >= 1 && n <= 12 {
                return 0x70 + (n - 1); // VK_F1 = 0x70
            }
        }
    }

    // Number keys 0-9
    if last_part.len() == 1 {
        let ch = last_part.chars().next().unwrap();
        if ch.is_ascii_digit() {
            return ch as u16; // '0' = 0x30
        }
        if ch.is_ascii_alphabetic() {
            return ch.to_ascii_uppercase() as u16; // 'A' = 0x41
        }
    }

    // Special keys
    match last_part.as_str() {
        "esc" | "escape" => 0x1B,
        "tab" => 0x09,
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "backspace" | "bksp" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "up" | "uparrow" => 0x26,
        "down" | "downarrow" => 0x28,
        "left" | "leftarrow" => 0x25,
        "right" | "rightarrow" => 0x27,
        "printscreen" | "prtsc" => 0x2C,
        "scrolllock" | "scrlk" => 0x91,
        "pause" | "break" => 0x13,
        "capslock" | "caps" => 0x14,
        "numlock" => 0x90,
        "menu" | "apps" => 0x5D,
        _ => {
            // Try single character
            if let Some(ch) = last_part.chars().next() {
                ch.to_ascii_uppercase() as u16
            } else {
                0
            }
        }
    }
}

/// Wait for capture event (blocking)
#[allow(dead_code)]
pub fn wait_for_capture(rx: &Receiver<HotkeyEvent>) -> Option<Region> {
    match rx.recv() {
        Ok(HotkeyEvent::Capture) => Some(Region::default()),
        _ => None,
    }
}

/// Capture region for OCR
#[derive(Debug, Clone, Copy, Default)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Region {
    /// 全屏区域（主屏）
    pub fn full_screen() -> Self {
        unsafe {
            Self {
                x: 0,
                y: 0,
                w: winapi::um::winuser::GetSystemMetrics(
                    winapi::um::winuser::SM_CXSCREEN,
                ),
                h: winapi::um::winuser::GetSystemMetrics(
                    winapi::um::winuser::SM_CYSCREEN,
                ),
            }
        }
    }
}

/// Virtual key code constants (Windows VK_*)
#[allow(dead_code)]
pub mod vk {
    pub const VK_LBUTTON: u16 = 0x01;
    pub const VK_RBUTTON: u16 = 0x02;
    pub const VK_CANCEL: u16 = 0x03;
    pub const VK_MBUTTON: u16 = 0x04;
    pub const VK_XBUTTON1: u16 = 0x05;
    pub const VK_XBUTTON2: u16 = 0x06;
    pub const VK_BACK: u16 = 0x08;
    pub const VK_TAB: u16 = 0x09;
    pub const VK_CLEAR: u16 = 0x0C;
    pub const VK_RETURN: u16 = 0x0D;
    pub const VK_SHIFT: u16 = 0x10;
    pub const VK_CONTROL: u16 = 0x11;
    pub const VK_MENU: u16 = 0x12;
    pub const VK_PAUSE: u16 = 0x13;
    pub const VK_CAPITAL: u16 = 0x14;
    pub const VK_ESCAPE: u16 = 0x1B;
    pub const VK_SPACE: u16 = 0x20;
    pub const VK_PRIOR: u16 = 0x21;
    pub const VK_NEXT: u16 = 0x22;
    pub const VK_END: u16 = 0x23;
    pub const VK_HOME: u16 = 0x24;
    pub const VK_LEFT: u16 = 0x25;
    pub const VK_UP: u16 = 0x26;
    pub const VK_RIGHT: u16 = 0x27;
    pub const VK_DOWN: u16 = 0x28;
    pub const VK_SNAPSHOT: u16 = 0x2C;
    pub const VK_INSERT: u16 = 0x2D;
    pub const VK_DELETE: u16 = 0x2E;
    pub const VK_LWIN: u16 = 0x5B;
    pub const VK_RWIN: u16 = 0x5C;
    pub const VK_NUMPAD0: u16 = 0x60;
    pub const VK_NUMPAD1: u16 = 0x61;
    pub const VK_NUMPAD2: u16 = 0x62;
    pub const VK_NUMPAD3: u16 = 0x63;
    pub const VK_NUMPAD4: u16 = 0x64;
    pub const VK_NUMPAD5: u16 = 0x65;
    pub const VK_NUMPAD6: u16 = 0x66;
    pub const VK_NUMPAD7: u16 = 0x67;
    pub const VK_NUMPAD8: u16 = 0x68;
    pub const VK_NUMPAD9: u16 = 0x69;
    pub const VK_F1: u16 = 0x70;
    pub const VK_F2: u16 = 0x71;
    pub const VK_F3: u16 = 0x72;
    pub const VK_F4: u16 = 0x73;
    pub const VK_F5: u16 = 0x74;
    pub const VK_F6: u16 = 0x75;
    pub const VK_F7: u16 = 0x76;
    pub const VK_F8: u16 = 0x77;
    pub const VK_F9: u16 = 0x78;
    pub const VK_F10: u16 = 0x79;
    pub const VK_F11: u16 = 0x7A;
    pub const VK_F12: u16 = 0x7B;
}

/// Helper to format hotkey string from modifiers and key code
#[allow(dead_code)]
pub fn format_hotkey(modifiers: u32, key_code: u16) -> String {
    let mut parts = Vec::new();

    if modifiers & MOD_CONTROL as u32 != 0 {
        parts.push("Ctrl");
    }
    if modifiers & MOD_ALT as u32 != 0 {
        parts.push("Alt");
    }
    if modifiers & MOD_SHIFT as u32 != 0 {
        parts.push("Shift");
    }
    if modifiers & MOD_WIN as u32 != 0 {
        parts.push("Win");
    }

    // Format key
    let key_str = match key_code {
        0x30..=0x39 => ((key_code as u8) as char).to_string(), // 0-9
        0x41..=0x5A => ((key_code as u8) as char).to_string(), // A-Z
        0x70..=0x7B => format!("F{}", key_code - 0x70 + 1),   // F1-F12
        0x1B => "Esc".to_string(),
        0x09 => "Tab".to_string(),
        0x20 => "Space".to_string(),
        0x0D => "Enter".to_string(),
        0x08 => "Backspace".to_string(),
        0x2E => "Delete".to_string(),
        0x2D => "Insert".to_string(),
        0x24 => "Home".to_string(),
        0x23 => "End".to_string(),
        0x26 => "Up".to_string(),
        0x28 => "Down".to_string(),
        0x25 => "Left".to_string(),
        0x27 => "Right".to_string(),
        _ => format!("0x{:02X}", key_code),
    };

    parts.push(&key_str);
    parts.join("+")
}
