// ============================================================
// Services · 持久化（SQLite，Phase 2 重写）
// 表结构见 docs/WinOCR-开发指南.md §4.2：
//   history（唯一真源）+ ocr_cache（OCR 结果缓存）
// 单连接 + Mutex（桌面单机应用并发极低，够用且最简单）
// ============================================================
use anyhow::{Context, Result};
use log::{info, warn};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::types::HistoryItem;

/// ID 生成器（时间戳 + 进程内自增，避免外部依赖）
static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    let n = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{:x}", now_secs(), n)
}

/// 数据目录：%APPDATA%/WinOCR（与 config.rs 保持一致）
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("WinOCR")
}

fn db_path() -> PathBuf {
    data_dir().join("winocr.db")
}

/// 全局数据库连接（OnceLock 保证只初始化一次）
/// 注意：存储 Result<String, ...> 以便在稳定 Rust 上用 get_or_init（get_or_try_init 是不稳定特性）
static CONN: OnceLock<Result<Mutex<Connection>, String>> = OnceLock::new();

/// 供 collect 等兄弟服务复用同一数据库连接
pub fn db_conn() -> Result<&'static Mutex<Connection>> {
    conn()
}

fn conn() -> Result<&'static Mutex<Connection>> {
    let guard = CONN.get_or_init(|| {
        let init = (|| -> Result<Mutex<Connection>> {
            let dir = data_dir();
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("创建数据目录失败: {}", dir.display()))?;
            let path = db_path();
            let c = Connection::open(&path)
                .with_context(|| format!("打开数据库失败: {}", path.display()))?;
            c.pragma_update(None, "journal_mode", "WAL").ok();
            c.pragma_update(None, "busy_timeout", 5000).ok();
            c.pragma_update(None, "foreign_keys", "ON").ok();
            init_schema(&c)?;
            info!("[services::persist] SQLite 已初始化: {}", path.display());
            Ok(Mutex::new(c))
        })();
        init.map_err(|e: anyhow::Error| format!("{e:#}"))
    });
    guard
        .as_ref()
        .map_err(|e| anyhow::anyhow!("数据库连接初始化失败: {e}"))
}

fn init_schema(c: &Connection) -> Result<()> {
    c.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS history (
            id          TEXT PRIMARY KEY,
            ts          INTEGER NOT NULL,
            src         TEXT NOT NULL,
            tgt         TEXT NOT NULL,
            src_lang    TEXT NOT NULL,
            tgt_lang    TEXT NOT NULL,
            engine      TEXT NOT NULL,
            screenshot  TEXT DEFAULT '',
            project     TEXT DEFAULT '',
            collected   INTEGER DEFAULT 0,
            note        TEXT DEFAULT '',
            tags        TEXT DEFAULT '',
            exported    INTEGER DEFAULT 0,
            favorited   INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_history_ts ON history(ts DESC);
        CREATE INDEX IF NOT EXISTS idx_history_collected ON history(collected);
        CREATE INDEX IF NOT EXISTS idx_history_project ON history(project);

        CREATE TABLE IF NOT EXISTS ocr_cache (
            image_hash TEXT PRIMARY KEY,
            result_json TEXT NOT NULL,
            ts INTEGER NOT NULL
        );
        "#,
    )
    .context("初始化数据库表结构失败")
}

pub struct Persist;

impl Persist {
    pub fn new() -> Self {
        Self
    }

    /// 追加一条历史记录（INSERT OR REPLACE，幂等）
    pub fn append_history(&self, item: &HistoryItem) {
        let c = match conn() {
            Ok(c) => c,
            Err(e) => {
                warn!("[persist] 获取连接失败: {e:#}");
                return;
            }
        };
        let c = match c.lock() {
            Ok(g) => g,
            Err(e) => {
                warn!("[persist] 连接锁中毒: {e}");
                return;
            }
        };
        let r = c.execute(
            r#"
            INSERT OR REPLACE INTO history
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
        if let Err(e) = r {
            warn!("[persist] 写入历史失败: {e}");
        }
    }

    /// 便捷封装：构造并追加一条历史记录
    pub fn record_translation(
        &self,
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
            src_lang: src_lang.to_string(),
            tgt_lang: tgt_lang.to_string(),
            engine: engine.to_string(),
            screenshot: String::new(),
            project: String::new(),
            collected: false,
            note: String::new(),
            tags: String::new(),
            exported: false,
            favorited: false,
        };
        self.append_history(&item);
        item
    }

    /// 加载全部历史（倒序：最新在前）
    pub fn load_history(&self) -> Result<Vec<HistoryItem>> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        let mut stmt = c.prepare(
            r#"
            SELECT id, ts, src, tgt, src_lang, tgt_lang, engine,
                   screenshot, project, collected, note, tags, exported, favorited
            FROM history
            ORDER BY ts DESC
            "#,
        )?;
        let rows = stmt.query_map([], row_to_item)?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// 按条件筛选历史（高级筛选，供历史抽屉）
    /// - project: 项目名（空/「全部」= 不过滤）
    /// - q: 搜索词（匹配 src/tgt/note，LIKE 模糊）
    /// - collected: Option<bool>，Some 时按采集状态过滤
    #[allow(dead_code)] // 历史抽屉高级筛选（UI 对接后启用）
    pub fn query_history(
        &self,
        project: &str,
        q: &str,
        collected: Option<bool>,
        limit: i64,
    ) -> Result<Vec<HistoryItem>> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        let mut sql = String::from(
            "SELECT id, ts, src, tgt, src_lang, tgt_lang, engine,
                    screenshot, project, collected, note, tags, exported, favorited
             FROM history WHERE 1=1",
        );
        // 提升到块级避免临时值悬垂
        let project_owned = project.to_string();
        let like_owned = format!("%{}%", q.trim());
        let has_collected = collected.is_some();
        let collected_b = collected.map(|b| b as i64).unwrap_or(0);
        let limit_v = limit;
        let mut args: Vec<&dyn rusqlite::ToSql> = Vec::new();
        if !project_owned.is_empty() && project_owned != "全部" {
            sql.push_str(" AND project = ?");
            args.push(&project_owned);
        }
        if !q.trim().is_empty() {
            sql.push_str(" AND (src LIKE ? OR tgt LIKE ? OR note LIKE ?)");
            args.push(&like_owned);
            args.push(&like_owned);
            args.push(&like_owned);
        }
        if has_collected {
            sql.push_str(" AND collected = ?");
            args.push(&collected_b);
        }
        sql.push_str(" ORDER BY ts DESC LIMIT ?");
        args.push(&limit_v);

        let mut stmt = c.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter().copied()), row_to_item)?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row?);
        }
        Ok(items)
    }

    /// 清空历史
    pub fn clear_history(&self) -> Result<()> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute("DELETE FROM history", [])?;
        info!("[services::persist] 历史已清空");
        Ok(())
    }

    /// 删除单条历史（按 id）
    #[allow(dead_code)] // UI 单条删除（后续对接）
    pub fn remove_history(&self, id: &str) -> Result<()> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute("DELETE FROM history WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 更新采集状态（collected 标记）
    #[allow(dead_code)] // Phase 3: UI 采集状态切换
    pub fn set_collected(&self, id: &str, collected: bool) -> Result<()> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute(
            "UPDATE history SET collected = ?1 WHERE id = ?2",
            params![collected as i64, id],
        )?;
        Ok(())
    }

    // ---------------- OCR 缓存 ----------------

    /// 写入 OCR 缓存（仅缓存非空结果）
    #[allow(dead_code)] // Phase 3: OCR 缓存命中
    pub fn ocr_cache_put(&self, image_hash: &str, result_json: &str) -> Result<()> {
        if result_json.trim().is_empty() {
            return Ok(());
        }
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute(
            r#"
            INSERT OR REPLACE INTO ocr_cache (image_hash, result_json, ts)
            VALUES (?1, ?2, ?3)
            "#,
            params![image_hash, result_json, now_secs()],
        )?;
        Ok(())
    }

    /// 查询 OCR 缓存
    #[allow(dead_code)] // Phase 3: OCR 缓存命中
    pub fn ocr_cache_get(&self, image_hash: &str) -> Result<Option<String>> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        let v = c
            .query_row(
                "SELECT result_json FROM ocr_cache WHERE image_hash = ?1",
                params![image_hash],
                |row| row.get(0),
            )
            .optional()?;
        Ok(v)
    }

    /// 清空 OCR 缓存（设置/排障用）
    #[allow(dead_code)]
    pub fn ocr_cache_clear(&self) -> Result<()> {
        let c = conn()?;
        let c = c.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute("DELETE FROM ocr_cache", [])?;
        Ok(())
    }
}

/// 行 → HistoryItem 映射
fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryItem> {
    Ok(HistoryItem {
        id: row.get(0)?,
        ts: row.get(1)?,
        src: row.get(2)?,
        tgt: row.get(3)?,
        src_lang: row.get(4)?,
        tgt_lang: row.get(5)?,
        engine: row.get(6)?,
        screenshot: row.get(7)?,
        project: row.get(8)?,
        collected: row.get::<_, i64>(9)? != 0,
        note: row.get(10)?,
        tags: row.get(11)?,
        exported: row.get::<_, i64>(12)? != 0,
        favorited: row.get::<_, i64>(13)? != 0,
    })
}