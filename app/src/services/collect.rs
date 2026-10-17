// ============================================================
// Services · 采集队列（v2.0 → Phase 2 SQLite）
// 采集条目管理 + SQLite 持久化（复用 persist.rs 连接）
// 表：collect（见 persist.rs init_schema）
// ============================================================
use anyhow::{Context, Result};
use log::{error, info};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use super::persist::{new_id, now_secs};

pub const DEFAULT_PROJECT: &str = "未分类";

/// 采集条目（含 SM-2 参数 + 学习卡片字段）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectItem {
    pub id: String,
    pub ts: i64,
    pub word: String,
    pub translation: String,
    pub src_lang: String,
    pub tgt_lang: String,
    pub mastery: u8,        // 0..5
    pub ease_factor: f32,   // SM-2 EF，最小 1.3
    pub interval: i64,      // 间隔（天）
    pub repetitions: u32,   // 连续答对次数
    pub next_review: i64,   // 下次复习（unix 秒）
    pub project: String,    // 学科分类（GB/T 13745 名称；旧数据为来源标签）
    #[serde(default)]
    pub context: String,    // 原句上下文
    #[serde(default)]
    pub level: String,      // 难度档（专业 / 基础 / 未分级 / CEFR A1–C2）
    #[serde(default)]
    pub phonetic: String,     // 音标（如 /nɪˈfraɪtɪs/）
    #[serde(default)]
    pub definition_en: String,   // 英英释义（open-dictionary 首义项）
    #[serde(default)]
    pub example: String,      // 双语例句（英文原句）
    #[serde(default)]
    pub example_zh: String,   // 双语例句（中文翻译）
    #[serde(default)]
    pub roots: String,        // 词根拆解文本（如 nephro-（肾）+ -itis（炎症））
    #[serde(default)]
    pub etymology: String,    // 词源链文本
    #[serde(default)]
    pub source: String,        // 来源：browser/selection/dict/import/manual
}

impl CollectItem {
    /// 学习状态派生（2026-10-17 分类选学，零迁移复用 SM-2 字段）：
    /// 0=未学（repetitions=0，入库后从未过卡）/ 1=学习中（已学未到期）/
    /// 2=待复习（已学且 next_review 到期）
    pub fn study_status(&self) -> u8 {
        if self.repetitions == 0 {
            0
        } else if self.next_review <= now_secs() {
            2
        } else {
            1
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        word: String,
        translation: String,
        src_lang: String,
        tgt_lang: String,
        project: String,
        context: String,
        level: String,
    ) -> Self {
        Self {
            id: new_id(),
            ts: now_secs(),
            word,
            translation,
            src_lang,
            tgt_lang,
            mastery: 0,
            ease_factor: 2.5,
            interval: 0,
            repetitions: 0,
            next_review: now_secs(),
            project,
            context,
            level,
            phonetic: String::new(),
            definition_en: String::new(),
            example: String::new(),
            example_zh: String::new(),
            roots: String::new(),
            etymology: String::new(),
            source: String::new(),
        }
    }
}

/// 采集队列服务
pub struct Collect {
    conn: &'static Mutex<Connection>,
}

impl Collect {
    pub fn new() -> Self {
        let conn = super::persist::db_conn().expect("获取数据库连接失败");
        let s = Self { conn };
        // 确保表存在（幂等；migrate 未运行时也安全）
        // 不静默吞错：建表/迁移失败必须可见，否则后续采集会以
        // "table collect has no column named xxx" 的 SQL 错误暴露在用户面前。
        if let Err(e) = s.ensure_table() {
            error!("[services::collect] 初始化 collect 表失败: {e:#}");
        }
        s
    }

    /// 确保 collect 表存在（与 history 表在同一 DB）
    fn ensure_table(&self) -> Result<()> {
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        // 注意顺序：先 CREATE TABLE（IF NOT EXISTS，旧表不受影响），
        // 再补齐旧表缺失的列，最后才建索引 —— 因为 idx_collect_level 依赖 level 列，
        // 若旧表尚无该列，先建索引会报错并提前返回，导致后面的列迁移永远不执行。
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
        .context("初始化 collect 表失败")?;
        // 旧库（Phase 2 已存在）补齐新列（ALTER 幂等）；必须在建索引之前
        Self::migrate_collect_columns(&c)?;
        c.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_collect_project ON collect(project);
            CREATE INDEX IF NOT EXISTS idx_collect_level ON collect(level);
            "#,
        )
        .context("创建 collect 索引失败")?;
        Ok(())
    }

/// 旧 collect 表补齐 context/level/学习卡片字段列（幂等；已存在则跳过）。
fn migrate_collect_columns(c: &Connection) -> Result<()> {
    let cols: std::collections::HashSet<String> = c
        .prepare("PRAGMA table_info(collect)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .flatten()
        .collect();
    let defs = [
        ("context", "ALTER TABLE collect ADD COLUMN context TEXT NOT NULL DEFAULT ''"),
        ("level", "ALTER TABLE collect ADD COLUMN level TEXT NOT NULL DEFAULT '未分级'"),
        ("phonetic", "ALTER TABLE collect ADD COLUMN phonetic TEXT NOT NULL DEFAULT ''"),
        ("definition_en", "ALTER TABLE collect ADD COLUMN definition_en TEXT NOT NULL DEFAULT ''"),
        ("example", "ALTER TABLE collect ADD COLUMN example TEXT NOT NULL DEFAULT ''"),
        ("example_zh", "ALTER TABLE collect ADD COLUMN example_zh TEXT NOT NULL DEFAULT ''"),
        ("roots", "ALTER TABLE collect ADD COLUMN roots TEXT NOT NULL DEFAULT ''"),
        ("etymology", "ALTER TABLE collect ADD COLUMN etymology TEXT NOT NULL DEFAULT ''"),
        ("source", "ALTER TABLE collect ADD COLUMN source TEXT NOT NULL DEFAULT ''"),
    ];
    for (name, ddl) in defs {
        if !cols.contains(name) {
            c.execute_batch(ddl)
                .with_context(|| format!("补齐 collect 列 {name} 失败"))?;
        }
    }
    Ok(())
}

    /// 添加生词（去重：同 word 视为重复）
    pub fn add_collect(
        &self,
        word: &str,
        translation: &str,
        src_lang: &str,
        tgt_lang: &str,
        project: &str,
        context: &str,
        level: &str,
    ) -> Result<()> {
        let word = word.trim();
        let translation = translation.trim();
        // 释义允许为空：用户双击候选词即可先收进生词本，释义后续再补（见 add_word_pending）。
        if word.is_empty() {
            return Ok(());
        }
        // 空释义统一记为空串（NOT NULL 列存 ''，展示层再渲染成「待补释义」占位）。
        let translation = if translation.is_empty() { "" } else { translation };
        let proj = if project.trim().is_empty() {
            DEFAULT_PROJECT.to_string()
        } else {
            project.trim().to_string()
        };
        let context = context.trim().to_string();
        let level = if level.trim().is_empty() {
            "未分级".to_string()
        } else {
            level.trim().to_string()
        };

        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        // 去重检查：同一个 word 只保留一条（释义不同也视为重复，避免双击产生多条待补条目）。
        // 旧逻辑按 word+translation 去重，空释义时会反复插入同一条，故改为按 word 去重。
        let exists: bool = c.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM collect WHERE word = ?1
             )",
            params![word],
            |row| row.get(0),
        )?;
        if exists {
            return Ok(());
        }
        let item = CollectItem::new(
            word.to_string(),
            translation.to_string(),
            src_lang.to_string(),
            tgt_lang.to_string(),
            proj,
            context,
            level,
        );
        c.execute(
            r#"
            INSERT INTO collect
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
        )?;
        info!("[services::collect] 添加生词: {word} → {translation} [{} / {}]", item.project, item.level);
        Ok(())
    }

    /// 批量采集：将一段中英混排文本切出英文词，按学科/难度分类后逐条入库。
    ///
    /// 正向保守（见对话记录）：
    /// - 术语库（medical.json）命中 → 精确学科 + 专业 + 中文释义
    /// - 词根/后缀命中 → 推断学科 + 专业
    /// - 其余通用词 → 通用医学 + CEFR 基础档；`medical_only` 开时跳过非医学词
    ///
    /// `context` 为原句，所有拆分出的词共享同一上下文；返回采集数量与明细供前端展示。
    pub fn ingest_text(
        &self,
        text: &str,
        src_lang: &str,
        tgt_lang: &str,
        context: &str,
        medical_only: bool,
        ignore: &[String],
        termdb: &crate::services::terms::TermDb,
    ) -> Result<IngestReport> {
        let mut report = IngestReport::default();
        // 同一次采集内去重：原文里重复出现的词只入库一次，report 也只记一条。
        // （add_collect 本身按 word 去重，但那会静默吞掉重复项，导致 count 虚高）
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for raw in split_english_tokens(text) {
            let w = raw.to_lowercase();
            // 只认英文：token 必须全部由 ASCII 字母/撇号组成（含数字的片段已在切词阶段断开）
            if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') {
                continue;
            }
            if ignore.iter().any(|x| x.eq_ignore_ascii_case(&w)) {
                continue;
            }
            // 词形归一：小写去重（Prostate / PROSTATE / prostate 视为同一个词）
            if !seen.insert(w.clone()) {
                continue;
            }
            let (project, level, translation) = if let Some(entry) = termdb.entry(&w) {
                (entry.cat.clone(), entry.level.clone(), entry.zh.clone())
            } else if let Some(c) = crate::services::classify::classify(&w, ignore) {
                if medical_only && !c.is_medical {
                    continue;
                }
                (c.project, c.level, String::new())
            } else {
                continue; // 忽略词
            };
            self.add_collect(&raw, &translation, src_lang, tgt_lang, &project, context, &level)?;
            report.items.push(IngestItem {
                word: raw,
                project,
                level,
            });
            report.count += 1;
        }
        Ok(report)
    }

    /// 删除生词（按 id）
    pub fn remove_collect(&self, id: &str) -> Result<()> {
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute("DELETE FROM collect WHERE id = ?1", params![id])?;
        info!("[services::collect] 删除生词: {id}");
        Ok(())
    }

    /// 全部生词
    pub fn list_all(&self) -> Vec<CollectItem> {
        let c = match self.conn.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match c.prepare(
            "SELECT id, ts, word, translation, src_lang, tgt_lang, mastery, ease_factor, interval, repetitions, next_review, project, context, level, phonetic, definition_en, example, example_zh, roots, etymology, source
             FROM collect ORDER BY ts DESC",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], row_to_item);
        match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 按项目筛选（「全部」返回所有）
    pub fn get_by_project(&self, project: &str) -> Vec<CollectItem> {
        let items = self.list_all();
        if project == "全部" || project.trim().is_empty() {
            items
        } else {
            items.into_iter().filter(|i| i.project == project).collect()
        }
    }

    /// 待复习队列（next_review <= now）
    #[allow(dead_code)] // v1.1: SM-2 复习提醒
    pub fn get_review_queue(&self) -> Vec<CollectItem> {
        let now = now_secs();
        self.list_all()
            .into_iter()
            .filter(|i| i.next_review <= now)
            .collect()
    }

    /// 所有项目名（去重 + 排序，首项为「全部」）
    pub fn get_tags(&self) -> Vec<String> {
        let items = self.list_all();
        let mut set: std::collections::HashSet<String> =
            items.iter().map(|i| i.project.clone()).collect();
        let mut projects: Vec<String> = set.drain().collect();
        projects.sort();
        let mut result = vec!["全部".to_string()];
        if let Some(pos) = projects.iter().position(|p| p == DEFAULT_PROJECT) {
            let p = projects.remove(pos);
            result.push(p);
        }
        result.extend(projects);
        result
    }

    /// SM-2 评分（quality: 0..5）
    #[allow(dead_code)] // Phase 3: 生词复习
    pub fn grade_collect(&self, id: &str, quality: u8) -> Result<()> {
        let q = quality.clamp(0, 5) as f32;
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;

        // 读取当前条目
        let item: Option<CollectItem> = c
            .query_row(
                "SELECT id, ts, word, translation, src_lang, tgt_lang, mastery, ease_factor, interval, repetitions, next_review, project, context, level, phonetic, definition_en, example, example_zh, roots, etymology, source
                 FROM collect WHERE id = ?1",
                params![id],
                row_to_item,
            )
            .optional()?;

        let Some(mut item) = item else {
            return Ok(());
        };

        // SM-2 更新 EF
        item.ease_factor =
            (item.ease_factor + (0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02))).max(1.3);
        if quality >= 3 {
            item.repetitions += 1;
            item.interval = match item.repetitions {
                1 => 1,
                2 => 6,
                _ => (item.interval as f32 * item.ease_factor).max(1.0) as i64,
            };
        } else {
            item.repetitions = 0;
            item.interval = 1;
        }
        item.mastery = quality.min(5);
        item.next_review = now_secs() + item.interval * 86_400;

        c.execute(
            r#"
            UPDATE collect SET
                mastery = ?1, ease_factor = ?2, interval = ?3,
                repetitions = ?4, next_review = ?5
            WHERE id = ?6
            "#,
            params![
                item.mastery,
                item.ease_factor,
                item.interval,
                item.repetitions,
                item.next_review,
                item.id,
            ],
        )?;
        info!(
            "[services::collect] 评分 {id} quality={quality} next={}",
            item.interval
        );
        Ok(())
    }

    /// 回填学习卡片字段（enrich 服务调用；只更新非空字段）。
    pub fn update_enrichment(
        &self,
        id: &str,
        phonetic: &str,
        definition_en: &str,
        example: &str,
        example_zh: &str,
        roots: &str,
        etymology: &str,
    ) -> Result<()> {
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute(
            r#"
            UPDATE collect SET
                phonetic = CASE WHEN ?2 != '' THEN ?2 ELSE phonetic END,
                definition_en = CASE WHEN ?3 != '' THEN ?3 ELSE definition_en END,
                example = CASE WHEN ?4 != '' THEN ?4 ELSE example END,
                example_zh = CASE WHEN ?5 != '' THEN ?5 ELSE example_zh END,
                roots = CASE WHEN ?6 != '' THEN ?6 ELSE roots END,
                etymology = CASE WHEN ?7 != '' THEN ?7 ELSE etymology END
            WHERE id = ?1
            "#,
            params![id, phonetic, definition_en, example, example_zh, roots, etymology],
        )?;
        Ok(())
    }

    /// 回填中文释义（批量补全用：空 translation 跑 Argos 后写入；非空不覆盖）。
    pub fn update_translation(&self, id: &str, translation: &str) -> Result<()> {
        let translation = translation.trim();
        if translation.is_empty() {
            return Ok(());
        }
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute(
            "UPDATE collect SET translation = ?2 WHERE id = ?1 AND translation = ''",
            params![id, translation],
        )?;
        Ok(())
    }

    /// 标记来源（study::introduce_word 引入后补写；browser/selection/dict/import/manual/ocr）
    pub fn update_source(&self, id: &str, source: &str) -> Result<()> {
        if source.trim().is_empty() {
            return Ok(());
        }
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute(
            "UPDATE collect SET source = ?2 WHERE id = ?1 AND source = ''",
            params![id, source.trim()],
        )?;
        Ok(())
    }

    /// 重标难度档（批量补全用：仅覆盖「未分级」，避免踩掉已有档位）。
    pub fn update_level_if_ungraded(&self, id: &str, level: &str) -> Result<()> {
        let level = level.trim();
        if level.is_empty() || level == "未分级" {
            return Ok(());
        }
        let c = self.conn.lock().map_err(|e| anyhow::anyhow!("连接锁中毒: {e}"))?;
        c.execute(
            "UPDATE collect SET level = ?2 WHERE id = ?1 AND level = '未分级'",
            params![id, level],
        )?;
        Ok(())
    }

    /// 统计（总数, 今日待复习数）
    pub fn stats(&self) -> (usize, usize) {
        let items = self.list_all();
        let total = items.len();
        let now = now_secs();
        let due = items.iter().filter(|i| i.next_review <= now).count();
        (total, due)
    }
}

/// 行 → CollectItem 映射
fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<CollectItem> {
    Ok(CollectItem {
        id: row.get(0)?,
        ts: row.get(1)?,
        word: row.get(2)?,
        translation: row.get(3)?,
        src_lang: row.get(4)?,
        tgt_lang: row.get(5)?,
        mastery: row.get(6)?,
        ease_factor: row.get(7)?,
        interval: row.get(8)?,
        repetitions: row.get(9)?,
        next_review: row.get(10)?,
        project: row.get(11)?,
        context: row.get(12)?,
        level: row.get(13)?,
        phonetic: row.get(14)?,
        definition_en: row.get(15)?,
        example: row.get(16)?,
        example_zh: row.get(17)?,
        roots: row.get(18)?,
        etymology: row.get(19)?,
        source: row.get(20)?,
    })
}

/// 单条采集结果（用于回传前端展示）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestItem {
    pub word: String,
    pub project: String,
    pub level: String,
}

/// 批量采集报告
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IngestReport {
    pub count: usize,
    pub items: Vec<IngestItem>,
}

/// 从文本切出英文单词 token。
///
/// 只认 ASCII 拉丁字母（`is_ascii_alphabetic`）：
/// - 数字不算字母，"360"、"WTT" 这类含数字的片段会在数字处断开
/// - 中文/日文/西里尔等非 ASCII 字母一律视为分隔符（重要：`char::is_alphabetic`
///   对汉字返回 true，早期实现用它会把整句中文当成一个"词"收进生词本）
/// - 撇号 `'` 视为词内字符（don't / patient's 保持完整），其余符号断开
///
/// 医学多词术语在此按单词拆分（批量采集的语义）；如需短语匹配可后续扩展。
fn split_english_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_alphabetic() {
            cur.push(ch);
        } else if ch == '\'' {
            // 词内撇号：后接字母则保留（don't），否则视为分隔（'em 引号用法）
            match chars.peek() {
                Some(&n) if n.is_ascii_alphabetic() => cur.push(ch),
                _ => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                }
            }
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}