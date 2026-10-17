// ============================================================
// WinOCR 原生入口（Rust + Slint）
// 严格分层：main → ui → services → core
// ============================================================
mod core;
mod services;
mod ui;

use anyhow::Result;
use env_logger::Env;

fn main() -> Result<()> {
    // 默认 info 级（未设 RUST_LOG 时），否则 env_logger 只输出 error，热键等服务日志不可见
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    // P2-4: 单实例守护 —— 若已有实例运行，激活旧窗口后退出。
    // 标题常量唯一真源在 ui::MAIN_TITLE（与 main_window.slint 的 title 逐字一致，
    // ui/mod.rs 的 bring_main_to_front 也按它 FindWindowW）。刻意**不带版本号**：
    // 否则跨版本升级时新旧实例标题不同，第二个实例会静默退出且无法唤出旧窗口。
    let app_title = ui::MAIN_TITLE;
    let _guard = match services::single_instance::SingleInstanceGuard::new(app_title)? {
        Some(guard) => guard,
        None => {
            log::info!("[main] 已有实例运行，退出");
            return Ok(());
        }
    };

    // P0/P2: 首次启动把 open-dictionary 词典数据同步到 %APPDATA%/WinOCR/dict/
    //（数据独立于 exe 分发；已同步则跳过）
    if let Err(e) = services::dict::sync_from_assets() {
        log::warn!("[main] 词典数据同步失败: {e:#}");
    }

    ui::run()
}
