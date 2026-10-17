//! 历史记录与生词本存储（§17.6 数据层）
//!
//! - 历史：`data_local_dir/WinOCR-Html/history.jsonl`，追加写，加载时倒序（最新在前）。
//! - 生词本：`data_local_dir/WinOCR-Html/vocab.json`，整文件重写（条目量较小）。
//! - 生词本复习采用 SM-2 间隔重复算法（§17.6 复习面板）。
//!
//! 全部为本机文件，不携带密钥/网络内容。JSON 序列化失败仅记日志不崩溃。

use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

fn data_dir() -> PathBuf {
    dirs::data_local_dir()
        .map(|p| p.join("WinOCR-Html"))
        .unwrap_or_else(|| PathBuf::from("WinOCR-Html"))
}

fn history_path() -> PathBuf {
    data_dir().join("history.jsonl")
}

fn vocab_path() -> PathBuf {
    data_dir().join("vocab.json")
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 生成唯一 ID（时间戳 + 进程内自增，避免依赖外部 crate）
fn new_id() -> String {
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", now_secs(), n)
}

#[derive(Serialize, Deserialize, Clone)]
pub struct HistoryItem {
    pub id: String,
    pub ts: i64,
    pub src: String,
    pub tgt: String,
    pub engine: String,
    pub src_lang: String,
    pub tgt_lang: String,
    pub favorited: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct VocabItem {
    pub id: String,
    pub word: String,
    pub meaning: String,
    pub added: i64,
    pub tags: Vec<String>,
    // SM-2 状态
    pub ease: f32,
    pub interval: i32,
    pub reps: i32,
    pub due: i64,
}

impl VocabItem {
    fn new(word: &str, meaning: &str) -> Self {
        let ts = now_secs();
        Self {
            id: new_id(),
            word: word.to_string(),
            meaning: meaning.to_string(),
            added: ts,
            tags: vec![],
            ease: 2.5,
            interval: 0,
            reps: 0,
            due: ts,
        }
    }
}

/// 追加一条历史记录（JSONL 追加写）
pub fn append_history(item: &HistoryItem) {
    let dir = data_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        log::warn!("创建历史目录失败: {}", e);
        return;
    }
    let path = history_path();
    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            let line = match serde_json::to_string(item) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("历史序列化失败: {}", e);
                    return;
                }
            };
            if let Err(e) = writeln!(f, "{}", line) {
                log::warn!("写入历史失败: {}", e);
            }
        }
        Err(e) => log::warn!("打开历史文件失败: {}", e),
    }
}

/// 构造并追加一条历史记录（便捷封装）
pub fn record_translation(
    src: &str,
    tgt: &str,
    engine: &str,
    src_lang: &str,
    tgt_lang: &str,
) -> HistoryItem {
    let item = HistoryItem {
        id: new_id(),
        ts: now_secs(),
        src: src.to_string(),
        tgt: tgt.to_string(),
        engine: engine.to_string(),
        src_lang: src_lang.to_string(),
        tgt_lang: tgt_lang.to_string(),
        favorited: false,
    };
    append_history(&item);
    item
}

/// 加载全部历史（已按时间倒序：最新在前）
pub fn load_history() -> Vec<HistoryItem> {
    let path = history_path();
    let Ok(f) = File::open(&path) else {
        return vec![];
    };
    let reader = BufReader::new(f);
    let mut items = Vec::new();
    for line in reader.lines().flatten() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(it) = serde_json::from_str::<HistoryItem>(&line) {
            items.push(it);
        }
    }
    items.reverse();
    items
}

/// 清空历史
pub fn clear_history() {
    let path = history_path();
    let _ = fs::write(&path, "");
}

/// 导出历史为指定格式
pub fn export_history(items: &[HistoryItem], format: &str, path: &str) -> anyhow::Result<()> {
    let content = match format {
        "csv" => history_to_csv(items),
        "md" => history_to_md(items),
        _ => history_to_json(items),
    };
    fs::write(path, content).map_err(|e| anyhow::anyhow!("导出失败: {}", e))
}

fn history_to_json(items: &[HistoryItem]) -> String {
    serde_json::to_string_pretty(items).unwrap_or_default()
}

fn history_to_csv(items: &[HistoryItem]) -> String {
    let mut s = String::from("time,source,translation,engine,src_lang,tgt_lang\n");
    for it in items {
        let ts = format_time(it.ts);
        s.push_str(&format!(
            "{},{},{},{},{},{}\n",
            csv_escape(&ts),
            csv_escape(&it.src),
            csv_escape(&it.tgt),
            csv_escape(&it.engine),
            csv_escape(&it.src_lang),
            csv_escape(&it.tgt_lang)
        ));
    }
    s
}

fn history_to_md(items: &[HistoryItem]) -> String {
    let mut s = String::from("# WinOCR-Html 翻译历史\n\n");
    for it in items {
        s.push_str(&format!(
            "## {} ({} → {})\n\n**原文**: {}\n\n**译文**: {}\n\n*引擎: {}*\n\n---\n\n",
            format_time(it.ts),
            it.src_lang,
            it.tgt_lang,
            it.src,
            it.tgt,
            it.engine
        ));
    }
    s
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 将 unix 秒格式化为可读时间
pub fn format_time(ts: i64) -> String {
    let secs = (ts % 60) as u32;
    let mins = ((ts / 60) % 60) as u32;
    let hours = ((ts / 3600) % 24) as u32;
    let days = (ts / 86400) as i64;
    // 简化显示：相对天数 + 时分秒
    if days > 0 {
        format!("{}d {:02}:{:02}:{:02}", days, hours, mins, secs)
    } else {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }
}

// ---------------- 生词本 ----------------

/// 加载生词本
pub fn load_vocab() -> Vec<VocabItem> {
    let path = vocab_path();
    let Ok(s) = fs::read_to_string(&path) else {
        return vec![];
    };
    serde_json::from_str::<Vec<VocabItem>>(&s).unwrap_or_default()
}

/// 保存生词本（整文件重写）
pub fn save_vocab(items: &[VocabItem]) {
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);
    let path = vocab_path();
    if let Ok(s) = serde_json::to_string_pretty(items) {
        let _ = fs::write(&path, s);
    }
}

/// 添加生词：若已存在（大小写不敏感）则更新含义，返回最终条目
pub fn add_vocab(word: &str, meaning: &str) -> VocabItem {
    let mut items = load_vocab();
    let lower = word.to_lowercase();
    if let Some(pos) = items.iter().position(|v| v.word.to_lowercase() == lower) {
        items[pos].meaning = meaning.to_string();
        let item = items[pos].clone();
        save_vocab(&items);
        item
    } else {
        let item = VocabItem::new(word, meaning);
        items.push(item.clone());
        save_vocab(&items);
        item
    }
}

/// 删除生词
pub fn remove_vocab(id: &str) {
    let items: Vec<VocabItem> = load_vocab().into_iter().filter(|v| v.id != id).collect();
    save_vocab(&items);
}

/// 待复习列表（due <= now）
pub fn due_vocab(items: &[VocabItem]) -> Vec<VocabItem> {
    let now = now_secs();
    items.iter().filter(|v| v.due <= now).cloned().collect()
}

/// 已掌握（连续复习 >=4 次且间隔 >=21 天）
pub fn mastered_count(items: &[VocabItem]) -> usize {
    items
        .iter()
        .filter(|v| v.reps >= 4 && v.interval >= 21)
        .count()
}

/// SM-2 评分：quality 0..5（UI 映射 1-3 -> 0-2 或直接使用）。返回更新后的条目（已落盘）。
pub fn review_grade(item: &VocabItem, quality: u8) -> VocabItem {
    let q = quality.min(5) as i32;
    let mut ease = item.ease;
    let mut interval = item.interval;
    let mut reps = item.reps;

    ease += 0.1 - (5 - q) as f32 * (0.08 + (5 - q) as f32 * 0.02);
    if ease < 1.3 {
        ease = 1.3;
    }

    if q >= 3 {
        if reps == 0 {
            interval = 1;
        } else if reps == 1 {
            interval = 6;
        } else {
            interval = (interval as f32 * ease).round() as i32;
        }
        reps += 1;
    } else {
        reps = 0;
        interval = 1;
    }

    let mut updated = item.clone();
    updated.ease = ease;
    updated.interval = interval;
    updated.reps = reps;
    updated.due = now_secs() + interval as i64 * 86400;

    let mut items = load_vocab();
    if let Some(pos) = items.iter().position(|v| v.id == item.id) {
        items[pos] = updated.clone();
        save_vocab(&items);
    }
    updated
}

/// 导出生词本为 Anki 可导入的 TSV（word\tmeaning）
pub fn export_vocab_anki(items: &[VocabItem], path: &str) -> anyhow::Result<()> {
    let mut s = String::new();
    for it in items {
        s.push_str(&format!("{}\t{}\n", it.word, it.meaning));
    }
    fs::write(path, s).map_err(|e| anyhow::anyhow!("导出失败: {}", e))
}

/// 导出生词本为 CSV
pub fn export_vocab_csv(items: &[VocabItem], path: &str) -> anyhow::Result<()> {
    let mut s = String::from("word,meaning,added,interval,reps,due\n");
    for it in items {
        s.push_str(&format!(
            "{},{},{},{},{},{}\n",
            csv_escape(&it.word),
            csv_escape(&it.meaning),
            csv_escape(&format_time(it.added)),
            it.interval,
            it.reps,
            csv_escape(&format_time(it.due))
        ));
    }
    fs::write(path, s).map_err(|e| anyhow::anyhow!("导出失败: {}", e))
}
