//! 文件日志（P1-8）：按日期 logs/winocr-host-YYYYMMDD.log 滚动，保留 7 天；
//! 同时输出到 stderr（保留控制台可见性）。
//!
//! 遥测开关见 `Config::telemetry`（默认 off，§15.6）。本模块只负责日志落盘，
//! 不含任何遥测上报逻辑——遥测上报若启用应单独实现且绝不携带文本/密钥。

use log::{Level, LevelFilter, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

struct FileLogger {
    level: LevelFilter,
    inner: Mutex<Option<File>>,
}

/// 初始化日志：文件 + stderr 双写。level 同 env_logger 过滤字符串。
pub fn init_logger(level: &str) {
    let lvl = match level.to_lowercase().as_str() {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Info,
    };
    cleanup_old(7);
    let logger = Box::new(FileLogger {
        level: lvl,
        inner: Mutex::new(open_today()),
    });
    let _ = log::set_boxed_logger(logger).map(|()| log::set_max_level(lvl));
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_record(record);
        eprintln!("{}", line);
        if let Ok(mut g) = self.inner.lock() {
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "{}", line);
                let _ = f.flush();
            }
        }
    }

    fn flush(&self) {}
}

fn format_record(record: &Record) -> String {
    let now = local_time_string();
    format!(
        "[{}] {} {}: {}",
        now,
        record.level(),
        record.target(),
        record.args()
    )
}

fn local_time_string() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
    )
}

fn log_dir() -> PathBuf {
    dirs::data_local_dir()
        .map(|p| p.join("WinOCR-Html").join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

fn today_filename() -> String {
    let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    format!(
        "winocr-host-{:04}{:02}{:02}.log",
        st.wYear, st.wMonth, st.wDay
    )
}

fn open_today() -> Option<File> {
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join(today_filename());
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

/// 清理 N 天前的日志文件（按 mtime 判断），避免无限增长。
fn cleanup_old(days: u64) {
    let dir = log_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let cutoff = std::time::Duration::from_secs(days * 86400);
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("winocr-host-") || !name.ends_with(".log") {
            continue;
        }
        if let Ok(meta) = e.metadata() {
            if let Ok(m) = meta.modified() {
                if let Ok(age) = now.duration_since(m) {
                    if age > cutoff {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
        }
    }
}
