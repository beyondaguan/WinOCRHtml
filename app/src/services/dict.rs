// ============================================================
// Services · 英英词典（open-dictionary）
//
// 数据源：mhollingshead/open-dictionary（Wiktionary 解析，26 万词）
//   - 703 个 JSON 分片，按首字母目录 api/{a}/{ab}.json
//   - 每个词条结构：
//     {
//       "word": { "etymologies": [ { "etymology": "...",
//                   "partsOfSpeech": [ { "partOfSpeech": "Noun",
//                                        "senses": [ { "sense": "...", "examples": ["..."] } ] } ] } ] }
//   - 数据独立于 exe 分发：运行时从 %APPDATA%/WinOCR/dict/open-dictionary/ 读取
//     （开发期数据源在 app/assets/dict/open-dictionary/，首次启动可同步）
//
// 查询方式：按词首字母定位分片，仅加载单个分片（~100KB），内存开销可控。
// ============================================================
use anyhow::{Context, Result};
use log::info;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::persist::data_dir;

/// 词典条目（查询结果，供 UI/扩展展示）
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DictEntry {
    pub word: String,
    pub phonetic: String,          // 音标（open-dictionary 无；ECDICT 提供）
    pub definitions: Vec<DictSense>, // 英英释义列表（ECDICT：definition 按分号拆）
    pub examples: Vec<String>,     // 例句（聚合去重，向后兼容旧路径）
    pub etymology: String,         // 词源文本（词条自带，多数为空）
    pub translation: String,       // 中文释义（ECDICT translation，英汉模式核心字段）
}

/// 单个义项（按词性分组内的条目，例句跟随义项）
#[derive(Debug, Clone, serde::Serialize)]
pub struct DictSense {
    pub pos: String,        // 词性：Noun / Verb / Adjective ...
    pub sense: String,      // 英英释义
    #[serde(default)]
    pub date: String,       // 首次记载年代（open-dictionary，常为空）
    #[serde(default)]
    pub examples: Vec<String>, // 该义项下的例句
}

/// 词典服务
pub struct Dict {
    root: PathBuf,
}

impl Dict {
    pub fn new() -> Self {
        Self {
            root: data_dir().join("dict").join("open-dictionary"),
        }
    }

    /// 数据根目录（供首次启动同步 / 调试）
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 检查词典数据是否已就绪
    pub fn available(&self) -> bool {
        self.root.join("api").is_dir()
    }

    /// 查询单词（原形精确匹配；可扩展词形还原）。
    /// 未命中返回 Ok(None)。
    pub fn lookup(&self, word: &str) -> Result<Option<DictEntry>> {
        let w = word.trim().to_lowercase();
        if w.is_empty() {
            return Ok(None);
        }
        let first = &w[..1];
        // 分片：api/{first}/{first}{second}.json；单字母词用 api/{first}/{first}.json
        let second = w.chars().nth(1).map(|c| c.to_string()).unwrap_or_default();
        let fname = if second.is_empty() {
            format!("{first}.json")
        } else {
            format!("{first}{second}.json")
        };
        let path = self.root.join("api").join(first).join(&fname);
        if !path.exists() {
            return Ok(None);
        }
        let data: HashMap<String, RawWord> = serde_json::from_slice(
            &std::fs::read(&path).with_context(|| format!("读取词典分片失败: {}", path.display()))?,
        )
        .context("解析词典分片失败")?;
        let Some(raw) = data.get(&w) else {
            return Ok(None);
        };
        Ok(Some(convert(raw, &w)))
    }
}

/// 原始词条（open-dictionary JSON 结构）
#[derive(Debug, Deserialize)]
#[allow(non_snake_case)] // JSON 字段名来自 open-dictionary（partsOfSpeech / partOfSpeech）
struct RawWord {
    #[serde(default)]
    etymologies: Vec<RawEtymology>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct RawEtymology {
    #[serde(default)]
    etymology: String,
    #[serde(default)]
    partsOfSpeech: Vec<RawPos>,
}

#[derive(Debug, Deserialize)]
#[allow(non_snake_case)]
struct RawPos {
    #[serde(default)]
    partOfSpeech: String,
    #[serde(default)]
    senses: Vec<RawSense>,
}

#[derive(Debug, Deserialize)]
struct RawSense {
    #[serde(default)]
    sense: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    examples: Vec<String>,
}

fn convert(raw: &RawWord, word: &str) -> DictEntry {
    let mut entry = DictEntry {
        word: word.to_string(),
        ..Default::default()
    };
    let mut seen_examples = std::collections::HashSet::new();
    let mut ety_parts: Vec<String> = Vec::new();
    for et in &raw.etymologies {
        if !et.etymology.trim().is_empty() {
            ety_parts.push(et.etymology.trim().to_string());
        }
        for pos in &et.partsOfSpeech {
            for s in &pos.senses {
                if !s.sense.is_empty() {
                    entry.definitions.push(DictSense {
                        pos: pos.partOfSpeech.clone(),
                        sense: s.sense.clone(),
                        date: s.date.trim().to_string(),
                        examples: s
                            .examples
                            .iter()
                            .map(|e| e.trim().to_string())
                            .filter(|e| !e.is_empty())
                            .collect(),
                    });
                }
                for ex in &s.examples {
                    let ex = ex.trim();
                    if !ex.is_empty() && seen_examples.insert(ex.to_string()) {
                        entry.examples.push(ex.to_string());
                    }
                }
            }
        }
    }
    entry.etymology = ety_parts.join("\n\n");
    entry
}

/// 词性英文 → 中文显示名
fn pos_zh(pos: &str) -> &'static str {
    match pos {
        "Noun" => "名词",
        "Verb" => "动词",
        "Adjective" => "形容词",
        "Adverb" => "副词",
        "Preposition" => "介词",
        "Conjunction" => "连词",
        "Interjection" => "感叹词",
        "Pronoun" => "代词",
        "Numeral" => "数词",
        "Article" => "冠词",
        "Phrase" => "短语",
        "Proverb" => "谚语",
        "Prefix" => "前缀",
        "Suffix" => "后缀",
        "Particle" => "小品词",
        "Symbol" => "符号",
        "Letter" => "字母",
        _ => "",
    }
}

/// 把完整词条格式化为词卡「词典全文」显示文本：
/// 按词性分块 → 义项编号 → 义项例句缩进跟随 → 词源段收尾。
/// 顺序保持词典原始顺序（Wiktionary 排序即常用度排序）。
pub fn format_full(entry: &DictEntry) -> String {
    let mut out = String::with_capacity(512);
    // 按词性分块（保持首次出现顺序）
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&DictSense>> = HashMap::new();
    for d in &entry.definitions {
        if !groups.contains_key(&d.pos) {
            order.push(d.pos.clone());
        }
        groups.entry(d.pos.clone()).or_default().push(d);
    }
    for pos in &order {
        let zh = pos_zh(pos);
        let head = if zh.is_empty() {
            pos.clone()
        } else {
            format!("{zh} · {pos}")
        };
        out.push_str(&format!("〖{head}〗\n"));
        for (i, d) in groups[pos].iter().enumerate() {
            let date = if d.date.is_empty() {
                String::new()
            } else {
                format!(" 〔{}〕", d.date)
            };
            out.push_str(&format!("{}. {}{}\n", i + 1, d.sense, date));
            for ex in &d.examples {
                out.push_str(&format!("    「{ex}」\n"));
            }
        }
        out.push('\n');
    }
    if !entry.etymology.is_empty() {
        out.push_str("—— 词源 ——\n");
        // 词源链截断（与 enrich.rs 同策略），防止超长
        let ety = if entry.etymology.chars().count() > 300 {
            let t: String = entry.etymology.chars().take(300).collect();
            format!("{t}…")
        } else {
            entry.etymology.clone()
        };
        out.push_str(&ety);
    }
    out.trim_end().to_string()
}

/// IPA 音标查询（ipa_en.json：word → 音标，16.8 万词，5.9MB）。
/// 进程内 OnceLock 缓存整表；文件缺失/损坏返回空 map，查询恒 None。
pub fn phonetic_of(word: &str) -> Option<String> {
    static IPA: std::sync::OnceLock<HashMap<String, String>> = std::sync::OnceLock::new();
    let map = IPA.get_or_init(|| {
        let path = data_dir().join("dict").join("ipa_en.json");
        std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<HashMap<String, String>>(&bytes).ok())
            .unwrap_or_default()
    });
    map.get(&word.trim().to_lowercase()).cloned()
}

impl Default for Dict {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// ECDICT 英汉词典（SQLite 直读，2026-10-17 用户拍板"零转换"方案）
//
// 数据源：skywind3000/ECDICT 官方 release ecdict-sqlite-28.zip（207MB）
//   解压得 stardict.db（表 stardict：word/phonetic/definition/translation/
//   pos/collins/oxford/tag/bnc/frq/exchange/detail/audio，word 建索引）
//   → 放到 %APPDATA%/WinOCR/dict/ecdict/stardict.db
//   （体积 ~1GB，不进 assets 不随 exe 分发；tools/install_ecdict.py 负责解压安装）
//
// 查询：rusqlite 只读连接（进程级 OnceLock + Mutex 复用），
//   SELECT ... WHERE word = ?1 COLLATE NOCASE LIMIT 1
// ============================================================

/// ECDICT 数据库路径：%APPDATA%/WinOCR/dict/ecdict/stardict.db
pub fn ecdict_db_path() -> PathBuf {
    data_dir().join("dict").join("ecdict").join("stardict.db")
}

/// ECDICT 数据是否已就绪
pub fn ecdict_available() -> bool {
    ecdict_db_path().exists()
}

/// 进程级只读连接（首次成功后复用；db 缺失时返回 None，装好数据后重启生效）
fn ecdict_conn() -> Option<&'static std::sync::Mutex<rusqlite::Connection>> {
    static CONN: std::sync::OnceLock<Option<std::sync::Mutex<rusqlite::Connection>>> =
        std::sync::OnceLock::new();
    CONN.get_or_init(|| {
        let path = ecdict_db_path();
        if !path.exists() {
            return None;
        }
        match rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(conn) => {
                info!("[services::dict] ECDICT sqlite 已就绪: {}", path.display());
                Some(std::sync::Mutex::new(conn))
            }
            Err(e) => {
                log::warn!("[services::dict] ECDICT sqlite 打开失败: {e}");
                None
            }
        }
    })
    .as_ref()
}

/// 当前学习词典来源（config.study_dict："open-dictionary" 默认 / "ecdict"）
pub fn study_dict_name() -> String {
    crate::core::config::Config::load()
        .map(|c| c.study_dict)
        .unwrap_or_else(|_| "open-dictionary".into())
}

/// 查询 ECDICT（英汉）：词形精确匹配（COLLATE NOCASE）。
/// definitions = definition（英英）按分号拆义项；translation = 中文释义原文。
pub fn lookup_ecdict(word: &str) -> Result<Option<DictEntry>> {
    let Some(conn) = ecdict_conn() else {
        return Ok(None);
    };
    let w = word.trim().to_lowercase();
    if w.is_empty() {
        return Ok(None);
    }
    let conn = conn.lock().unwrap();
    let mut stmt = conn
        .prepare_cached("SELECT phonetic, definition, translation, pos, tag FROM stardict WHERE word = ?1 COLLATE NOCASE LIMIT 1")
        .context("ECDICT 预编译查询失败")?;
    let mut rows = stmt.query([&w]).context("ECDICT 查询失败")?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let phonetic: String = row.get(0)?;
    let definition: String = row.get(1)?;
    let translation: String = row.get(2)?;
    let pos: String = row.get(3)?;
    let tag: String = row.get(4)?;
    let mut entry = DictEntry {
        word: w,
        phonetic: phonetic.trim().trim_start_matches('/').trim_end_matches('/').to_string(),
        translation: translation.trim().to_string(),
        ..Default::default()
    };
    // 英英释义按分号拆义项（ECDICT definition 风格："an acute disease...; inflammation..."）
    for part in definition.split(';') {
        let s = part.trim();
        if !s.is_empty() {
            entry.definitions.push(DictSense {
                pos: pos.clone(),
                sense: s.to_string(),
                date: String::new(),
                examples: Vec::new(),
            });
        }
    }
    if entry.definitions.is_empty() {
        // 无英英时用 tag 顶位（zk/gk/cet4/cet6/ky/toefl/ielts/gre/oxford），供 UI 显示标签
        if !tag.is_empty() {
            entry.definitions.push(DictSense {
                pos: String::new(),
                sense: format!("〔考纲标签〕{tag}"),
                date: String::new(),
                examples: Vec::new(),
            });
        }
    }
    Ok(Some(entry))
}

/// 中文释义快查（供 study.rs 补全链）：ECDICT translation，无则 None。
/// 与 lookup_ecdict 的区别：仅取中文释义，失败/缺失静默。
pub fn zh_translation_of(word: &str) -> Option<String> {
    lookup_ecdict(word)
        .ok()
        .flatten()
        .map(|e| e.translation)
        .filter(|s| !s.trim().is_empty())
}

/// 把 ECDICT 词条格式化为词卡「词典全文」显示文本（英汉模式）：
/// 音标行 → 中文释义（原样多行）→ 英英释义（若有）→ 考纲标签。
pub fn format_full_zh(entry: &DictEntry) -> String {
    let mut out = String::with_capacity(256);
    if !entry.phonetic.is_empty() {
        out.push_str(&format!("/{}/\n", entry.phonetic));
    }
    if !entry.translation.is_empty() {
        out.push_str("【中文释义】\n");
        out.push_str(&entry.translation);
        out.push('\n');
    }
    let en: Vec<String> = entry
        .definitions
        .iter()
        .filter(|d| !d.sense.starts_with("〔考纲标签〕"))
        .map(|d| d.sense.clone())
        .collect();
    if !en.is_empty() {
        out.push_str("\n【英英】\n");
        for (i, s) in en.iter().enumerate() {
            out.push_str(&format!("{}. {s}\n", i + 1));
        }
    }
    if let Some(tag) = entry.definitions.iter().find(|d| d.sense.starts_with("〔考纲标签〕")) {
        out.push('\n');
        out.push_str(&tag.sense);
    }
    out.trim().to_string()
}

/// 从开发期资源目录同步词典数据到 %APPDATA%/WinOCR/dict/。
/// 首次启动调用；若已存在则跳过（避免重复复制 72MB）。
pub fn sync_from_assets() -> Result<()> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("dict");
    let dst = data_dir().join("dict");
    if dst.join("open-dictionary").join("api").exists() {
        // 词典已同步；检查音标/词源/学段词表单文件是否缺失
        for name in ["ipa_en.json", "etymology_en_subset.json"] {
            let s = src.join(name);
            let d = dst.join(name);
            if s.exists() && !d.exists() {
                std::fs::copy(&s, &d)?;
            }
        }
        // 学段词表（stage/*.txt，小文件，随程序分发）
        let stage_src = src.join("stage");
        if stage_src.is_dir() {
            let stage_dst = dst.join("stage");
            std::fs::create_dir_all(&stage_dst)?;
            for e in std::fs::read_dir(&stage_src)? {
                let e = e?;
                if e.path().extension().map(|x| x == "txt").unwrap_or(false) {
                    let d = stage_dst.join(e.file_name());
                    if !d.exists() {
                        std::fs::copy(e.path(), &d)?;
                    }
                }
            }
        }
        return Ok(());
    }
    if !src.join("open-dictionary").join("api").exists() {
        // 开发期资源不存在（如已清理）——静默跳过，运行时查询会自然返回 None
        return Ok(());
    }
    std::fs::create_dir_all(&dst)
        .context("创建词典数据目录失败")?;
    copy_dir(&src.join("open-dictionary"), &dst.join("open-dictionary"))?;
    // 学段词表目录（stage/*.txt）
    if src.join("stage").is_dir() {
        copy_dir(&src.join("stage"), &dst.join("stage"))?;
    }
    for name in ["ipa_en.json", "etymology_en_subset.json"] {
        let s = src.join(name);
        if s.exists() {
            std::fs::copy(&s, &dst.join(name))?;
        }
    }
    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir(&path, &target)?;
        } else {
            std::fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 开发机已同步词典数据（%APPDATA%/WinOCR/dict）时验证真实词条；
    /// 未同步则跳过（CI/无数据环境不失败）。
    #[test]
    fn format_full_real_entry() {
        let d = Dict::new();
        if !d.available() {
            return;
        }
        let Some(e) = d.lookup("nephritis").unwrap() else {
            return;
        };
        let s = format_full(&e);
        assert!(s.contains("名词 · Noun"), "词性分块缺失: {s}");
        assert!(s.contains("Inflammation of the kidney"), "义项缺失: {s}");
    }

    #[test]
    fn phonetic_common_word() {
        // ipa_en.json 16.8 万词，hello 必有；未同步环境跳过
        if !Dict::new().available() {
            return;
        }
        let p = phonetic_of("hello");
        assert!(p.is_some(), "hello 应有音标");
    }

    /// ECDICT sqlite 直读冒烟：数据已安装（%APPDATA%）时验证医学词/词组/中文释义；
    /// 未安装则跳过（不失败）。
    #[test]
    fn ecdict_lookup_smoke() {
        if !ecdict_available() {
            return;
        }
        // 医学词：nephritis → n. 肾炎
        let e = lookup_ecdict("nephritis").unwrap().expect("nephritis 应命中");
        assert!(e.translation.contains("肾炎"), "中文释义异常: {}", e.translation);
        assert!(!e.phonetic.is_empty(), "音标缺失");
        // 词组（COLLATE NOCASE + 空格词）
        let e2 = lookup_ecdict("Renal Failure").unwrap().expect("词组应命中");
        assert!(e2.translation.contains("肾"), "词组释义异常: {}", e2.translation);
        // 英汉格式化输出包含音标行与中文段
        let s = format_full_zh(&e);
        assert!(s.contains("【中文释义】"), "格式化缺中文段: {s}");
    }
}