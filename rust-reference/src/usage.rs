//! 月度用量统计与配额降级（§15.8 / P2-6）
//!
//! 设计：
//! - 按「自然月 + 引擎」累计翻译字符数，JSON 持久化（tmp + rename 原子写）；
//! - `monthly_char_limit > 0` 时生效：达到上限即降级到免费引擎并提示；
//! - 跨月自动开新账期，旧月份数据保留（供 `--usage` 回看）；
//! - 无第三方依赖：本地年月由「1970 起天数 → 公历」换算（Hinnant 算法）。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 用量文件路径
fn usage_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|p| p.join("WinOCR-Html").join("usage.json"))
}

/// 付费/需配额的引擎（超限后降级到免费引擎）
const PAID_ENGINES: &[&str] = &["sf", "siliconflow", "youdao", "ollama"];
/// 降级后的免费引擎
const FREE_ENGINE: &str = "mymemory";

#[derive(Serialize, Deserialize, Default, Clone)]
struct MonthUsage {
    #[serde(default)]
    total: u64,
    #[serde(default)]
    engines: HashMap<String, u64>,
}

#[derive(Serialize, Deserialize, Default)]
struct Persisted {
    #[serde(default)]
    months: HashMap<String, MonthUsage>,
}

struct State {
    months: HashMap<String, MonthUsage>,
    dirty: usize,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| {
        let months = load().map(|p| p.months).unwrap_or_default();
        Mutex::new(State { months, dirty: 0 })
    })
}

fn load() -> Option<Persisted> {
    let p = usage_path()?;
    let s = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str::<Persisted>(&s).ok()
}

fn flush_locked(s: &mut State) {
    s.dirty = 0;
    let Some(p) = usage_path() else {
        return;
    };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let payload = Persisted {
        months: s.months.clone(),
    };
    let Ok(json) = serde_json::to_string(&payload) else {
        return;
    };
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, &p);
}

/// 天数 → 公历（年, 月, 日）；z 为 1970-01-01 起的天数
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let (y, m) = (if m <= 2 { y + 1 } else { y }, m as u32);
    (y, m, d as u32)
}

/// 当前本地年月字符串 `YYYY-MM`
pub fn current_month() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let (y, m, _d) = civil_from_days(days);
    format!("{:04}-{:02}", y, m)
}

/// 记账：某引擎消耗 n 个字符
pub fn record(engine: &str, chars: u64) {
    if chars == 0 {
        return;
    }
    let mut s = state().lock().unwrap();
    let month = current_month();
    let entry = s.months.entry(month).or_default();
    entry.total += chars;
    *entry.engines.entry(engine.to_string()).or_insert(0) += chars;
    s.dirty += 1;
    if s.dirty >= 4 {
        flush_locked(&mut s);
    }
}

/// 本月累计字符数
pub fn month_total() -> u64 {
    let s = state().lock().unwrap();
    s.months.get(&current_month()).map(|m| m.total).unwrap_or(0)
}

/// 本月分引擎用量
pub fn month_by_engine() -> HashMap<String, u64> {
    let s = state().lock().unwrap();
    s.months
        .get(&current_month())
        .map(|m| m.engines.clone())
        .unwrap_or_default()
}

/// 是否已达配额上限（limit=0 表示不限）
pub fn is_over_limit(limit: u64) -> bool {
    limit > 0 && month_total() >= limit
}

/// 配额降级：超限且首选引擎为付费引擎时，回退免费引擎
pub fn resolve_engine(preferred: &str, limit: u64) -> String {
    if is_over_limit(limit) && PAID_ENGINES.contains(&preferred) {
        log::warn!(
            "月度配额已用尽（{} ≥ {}），引擎 {} 降级为 {}",
            month_total(),
            limit,
            preferred,
            FREE_ENGINE
        );
        FREE_ENGINE.to_string()
    } else {
        preferred.to_string()
    }
}

/// `--usage` 输出的可读摘要
pub fn summary() -> String {
    let s = state().lock().unwrap();
    let month = current_month();
    let mut lines = Vec::new();
    let mut months: Vec<&String> = s.months.keys().collect();
    months.sort();
    for m in months {
        let u = &s.months[m];
        let mut engines: Vec<(&String, &u64)> = u.engines.iter().collect();
        engines.sort_by(|a, b| b.1.cmp(a.1));
        let detail = engines
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(", ");
        let mark = if *m == month { "（本月）" } else { "" };
        lines.push(format!("{}{}：共 {} 字符 [{}]", m, mark, u.total, detail));
    }
    if lines.is_empty() {
        lines.push("暂无用量记录".to_string());
    }
    lines.join("\n")
}

/// 手动落盘
pub fn flush() {
    let mut s = state().lock().unwrap();
    flush_locked(&mut s);
}
