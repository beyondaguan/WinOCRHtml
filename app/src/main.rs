// ============================================================
// WinOCR 原生入口（Rust + Slint）
// 严格分层：main → ui → services → core
//
// release 不弹控制台（GUI 子系统）；debug（cargo run）保留控制台看日志。
// 日志去向见 services/logging.rs：debug → 控制台；release →
// %APPDATA%/WinOCR/logs/winocr.log（滚动保留 5 份）。
// ============================================================
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core;
mod services;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    // debug → 控制台；release → %APPDATA%/WinOCR/logs/winocr.log（services/logging.rs）
    services::logging::init();

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
