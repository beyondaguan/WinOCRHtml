//! 全局应用上下文（§17.5 / §17.7 共享状态）。
//!
//! 浮窗「打开设置」「复制诊断」等操作按钮需要访问配置与路径，但 toast/浮窗
//! 回调是全局自由函数、不持有业务句柄。这里用 `OnceLock` 在 `run_resident`
//! 启动时注册全局引用，供各窗口回调安全取用。所有访问都是只读或受锁保护。

use crate::config::Config;
use std::sync::{Arc, Mutex, OnceLock};

pub struct AppGlobals {
    pub config: Arc<Mutex<Config>>,
    pub config_path: String,
    /// 最近一次诊断报告（脱敏文本），供「复制诊断」按钮使用
    pub last_diagnostic: Mutex<String>,
}

static GLOBALS: OnceLock<AppGlobals> = OnceLock::new();

/// 注册全局上下文（常驻启动时调用一次）。
pub fn set_globals(config: Arc<Mutex<Config>>, config_path: String) {
    let _ = GLOBALS.set(AppGlobals {
        config,
        config_path,
        last_diagnostic: Mutex::new(String::new()),
    });
}

/// 取全局上下文；未初始化时返回 None（理论上不会发生在常驻态）。
pub fn with_globals<R>(f: impl FnOnce(&AppGlobals) -> R) -> Option<R> {
    GLOBALS.get().map(f)
}

/// 写最近一次诊断文本（main 运行诊断后调用）。
pub fn set_last_diagnostic(text: &str) {
    if let Some(g) = GLOBALS.get() {
        *g.last_diagnostic.lock().unwrap() = text.to_string();
    }
}

/// 读最近一次诊断文本。
pub fn last_diagnostic() -> String {
    with_globals(|g| g.last_diagnostic.lock().unwrap().clone()).unwrap_or_default()
}
