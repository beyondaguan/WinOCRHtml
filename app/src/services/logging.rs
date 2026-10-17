// ============================================================
// Services · 日志初始化（v1.1.0 控制台隐藏配套）
//
// 背景：main.rs 加了 `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`
// 后，release 版不再有控制台，env_logger 默认打到 stdout 就"消失"了。
//
// 策略：
//   - debug（cargo run）：保持控制台直出，开发体验不变。
//   - release（安装版）：日志写入 %APPDATA%/WinOCR/logs/winocr.log，
//     每次启动滚动一次（winocr.log → .1 → .2 → .3 → .4，最多保留 5 份），
//     排查问题看最新 winocr.log 即可。
// ============================================================
use std::fs::{File, OpenOptions};
use std::path::PathBuf;

pub fn init() {
    let filter = env_logger::Env::default().default_filter_or("info");
    #[cfg(debug_assertions)]
    {
        env_logger::Builder::from_env(filter).init();
    }
    #[cfg(not(debug_assertions))]
    {
        match rotate_and_open() {
            Some(file) => {
                env_logger::Builder::from_env(filter)
                    .target(env_logger::Target::Pipe(Box::new(file)))
                    .init();
            }
            // 日志文件打不开（极端环境）则退回 stdout，聊胜于无
            None => env_logger::Builder::from_env(filter).init(),
        }
    }
}

/// 滚动旧日志并打开新日志文件（仅 release 编译）
#[cfg(not(debug_assertions))]
fn rotate_and_open() -> Option<File> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from)?;
    let dir = base.join("WinOCR").join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    // 滚动：删最旧的 .4，依次后移，winocr.log → .1
    let _ = std::fs::remove_file(dir.join("winocr.log.4"));
    for i in (1..=3).rev() {
        let _ = std::fs::rename(
            dir.join(format!("winocr.log.{i}")),
            dir.join(format!("winocr.log.{}", i + 1)),
        );
    }
    let _ = std::fs::rename(dir.join("winocr.log"), dir.join("winocr.log.1"));
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("winocr.log"))
        .ok()
}
