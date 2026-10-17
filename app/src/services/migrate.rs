// ============================================================
// Services · 旧数据迁移（Phase 2）
// 把旧 JSONL（history.jsonl / vocab.jsonl）迁移到 SQLite。
// 迁移完成后将旧文件重命名为 *.bak，避免重复迁移。
// 幂等：每次启动检测到旧文件才执行；无旧文件则跳过。
// ============================================================
use anyhow::{Context, Result};
use log::{info, warn};
use rusqlite::{params, Connection};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::core::types::HistoryItem;
use crate::services::collect::CollectItem;
use crate::services::persist::{data_dir, db_conn};

/// 迁移入口：检查并迁移旧 JSONL 文件。
/// 在 Persist::new 或程序启动时调用一次。
pub fn migrate_if_needed() -> Result<()> {
    let dir = data_dir();
    let history = dir.join("history.jsonl");
    let vocab = dir.join("vocab.jsonl");

    let mut any = false;
    if history.exists() {
        migrate_history(&history)?;
        any = true;
    }
    if vocab.exists() {
        migrate_vocab(&vocab)?;
        any = true;
    }
    if any {
        info!("[services::migrate] 旧数据迁移完成");
    }
    Ok(())
}

/// 迁移历史记录（JSONL → SQLite history 表）
fn migrate_history(path: &Path) -> Result<()> {
    let items = read_jsonl::<HistoryItem>(path)?;
    if items.is_empty() {
        // 空文件也标记为已迁移
        rename_to_bak(path)?;
        return Ok(());
    }

    let conn = db_conn()?;
    let c = conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
    let mut count = 0usize;
    for item in &items {
        let r = c.execute(
            r#"
            INSERT OR IGNORE INTO history
                (id, ts, src, tgt, src_lang, tgt_lang, engine, screenshot, project, collected, note, tags, exported, favorited)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                item.id,
                item.ts,
                item.src,
                item.tgt,
                item.src_lang,
                item.tgt_lang,
                item.engine,
                item.screenshot,
                item.project,
                item.collected as i64,
                item.note,
                item.tags,
                item.exported as i64,
                item.favorited as i64,
            ],
        );
        match r {
            Ok(n) if n > 0 => count += 1,
            Ok(_) => {}
            Err(e) => warn!("[migrate] 历史条目写入失败 {}: {e}", item.id),
        }
    }
    info!("[services::migrate] 历史迁移 {} 条", count);
    drop(c);
    rename_to_bak(path)?;
    Ok(())
}

/// 迁移生词本（vocab.jsonl 已是 CollectItem 格式）
fn migrate_vocab(path: &Path) -> Result<()> {
    let items = read_jsonl::<CollectItem>(path)?;
    if items.is_empty() {
        rename_to_bak(path)?;
        return Ok(());
    }

    let conn = db_conn()?;
    let c = conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
    ensure_collect_table(&c)?;
    let mut count = 0usize;
    for item in &items {
        let r = c.execute(
            r#"
            INSERT OR IGNORE INTO collect
                (id, ts, word, translation, src_lang, tgt_lang, mastery, ease_factor, interval, repetitions, next_review, project, context, level, phonetic, definition_en, example, example_zh, roots, etymology, source)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
            "#,
            params![
                item.id,
                item.ts,
                item.word,
                item.translation,
                item.src_lang,
                item.tgt_lang,
                item.mastery,
                item.ease_factor,
                item.interval,
                item.repetitions,
                item.next_review,
                item.project,
                item.context,
                item.level,
                item.phonetic,
                item.definition_en,
                item.example,
                item.example_zh,
                item.roots,
                item.etymology,
                item.source,
            ],
        );
        match r {
            Ok(n) if n > 0 => count += 1,
            Ok(_) => {}
            Err(e) => warn!("[migrate] 生词写入失败 {}: {e}", item.id),
        }
    }
    info!("[services::migrate] 生词本迁移 {} 条", count);
    drop(c);
    rename_to_bak(path)?;
    Ok(())
}

/// 确保 collect 表存在（migrate 可能先于 Collect::new 执行）
fn ensure_collect_table(c: &Connection) -> Result<()> {
    c.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS collect (
            id           TEXT PRIMARY KEY,
            ts           INTEGER NOT NULL,
            word         TEXT NOT NULL,
            translation  TEXT NOT NULL,
            src_lang     TEXT NOT NULL,
            tgt_lang     TEXT NOT NULL,
            mastery      INTEGER NOT NULL DEFAULT 0,
            ease_factor  REAL NOT NULL DEFAULT 2.5,
            interval     INTEGER NOT NULL DEFAULT 0,
            repetitions  INTEGER NOT NULL DEFAULT 0,
            next_review  INTEGER NOT NULL,
            project      TEXT NOT NULL DEFAULT '未分类',
            context      TEXT NOT NULL DEFAULT '',
            level        TEXT NOT NULL DEFAULT '未分级',
            phonetic     TEXT NOT NULL DEFAULT '',
            definition_en TEXT NOT NULL DEFAULT '',
            example      TEXT NOT NULL DEFAULT '',
            example_zh   TEXT NOT NULL DEFAULT '',
            roots        TEXT NOT NULL DEFAULT '',
            etymology    TEXT NOT NULL DEFAULT '',
            source       TEXT NOT NULL DEFAULT ''
        );
        "#,
    )
    .context("初始化 collect 表失败")
}

/// 读取 JSONL 文件为 Vec<T>（忽略空行与解析失败的行）
fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let f = File::open(path)?;
    let reader = BufReader::new(f);
    let mut items = Vec::new();
    for line in reader.lines().flatten() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(it) = serde_json::from_str::<T>(line) {
            items.push(it);
        }
    }
    Ok(items)
}

/// 将旧文件重命名为 .bak（若已存在则覆盖）
fn rename_to_bak(path: &Path) -> Result<()> {
    let bak = path.with_extension("jsonl.bak");
    if bak.exists() {
        fs::remove_file(&bak)?;
    }
    fs::rename(path, &bak).with_context(|| {
        format!(
            "重命名旧数据文件失败: {} → {}",
            path.display(),
            bak.display()
        )
    })
}

/// 是否还有待迁移的旧文件（供启动日志/自检）
#[allow(dead_code)] // 预留：启动自检日志
pub fn has_pending() -> bool {
    let dir = data_dir();
    dir.join("history.jsonl").exists() || dir.join("vocab.jsonl").exists()
}