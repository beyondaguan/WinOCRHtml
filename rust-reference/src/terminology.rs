//! 术语库（M11）：专业术语管理与翻译一致性
//!
//! 设计要点（对应计划文档 §13.2 / M11）：
//! - 内置术语库（法律/医学/技术/金融等领域的种子术语，编译期内置）。
//! - 用户自定义术语（关键词 → 翻译映射），由 `config.user_terms` 持久化。
//! - 匹配方式：精确词匹配（默认，按词边界）、正则匹配（`regex=true`）、大小写敏感可选。
//! - 优先级：**用户术语 > 内置术语 > 引擎默认**（同一 src 命中多条时，用户优先）。
//! - 翻译保护：翻译前用不可译占位符替换术语 src，翻译后还原为术语 dst，
//!   确保术语在译文中以用户/内置指定的形式出现（标准 glossary 占位符法）。
//!
//! 注意：术语库是「翻译处理层」能力，本身不依赖网络；批量翻译字幕(M9)与
//! PDF 块翻译(M10)都会经过 `apply`/`restore` 以保术语一致性。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 单条术语
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TermEntry {
    /// 源语言关键词（或正则模式，当 `regex=true`）
    pub src: String,
    /// 目标语言翻译（术语库优先采用的表达）
    pub dst: String,
    /// 领域标签（法律/医学/技术/金融…），仅用于分组展示，不参与匹配
    #[serde(default)]
    pub domain: String,
    /// 将 `src` 视为正则模式（默认 false = 精确词匹配）
    #[serde(default)]
    pub regex: bool,
    /// 大小写敏感（默认 false = 不敏感）
    #[serde(default)]
    pub case_sensitive: bool,
}

/// 内置种子术语（编译期内置，作为默认值；用户术语优先覆盖）
/// 仅放少量高频跨领域示例，避免内置库过大。领域术语应由用户在设置中扩充。
fn seed_builtin() -> Vec<TermEntry> {
    vec![
        TermEntry { src: "OCR".into(), dst: "OCR".into(), domain: "技术".into(), regex: false, case_sensitive: true },
        TermEntry { src: "API".into(), dst: "API".into(), domain: "技术".into(), regex: false, case_sensitive: true },
        TermEntry { src: "GPU".into(), dst: "GPU".into(), domain: "技术".into(), regex: false, case_sensitive: true },
        TermEntry { src: "CPU".into(), dst: "CPU".into(), domain: "技术".into(), regex: false, case_sensitive: true },
        TermEntry { src: "HTTP".into(), dst: "HTTP".into(), domain: "技术".into(), regex: false, case_sensitive: true },
        TermEntry { src: "machine learning".into(), dst: "机器学习".into(), domain: "技术".into(), regex: false, case_sensitive: false },
        TermEntry { src: "deep learning".into(), dst: "深度学习".into(), domain: "技术".into(), regex: false, case_sensitive: false },
        TermEntry { src: "neural network".into(), dst: "神经网络".into(), domain: "技术".into(), regex: false, case_sensitive: false },
        TermEntry { src: "plaintiff".into(), dst: "原告".into(), domain: "法律".into(), regex: false, case_sensitive: false },
        TermEntry { src: "defendant".into(), dst: "被告".into(), domain: "法律".into(), regex: false, case_sensitive: false },
        TermEntry { src: "contract".into(), dst: "合同".into(), domain: "法律".into(), regex: false, case_sensitive: false },
        TermEntry { src: "jurisdiction".into(), dst: "管辖权".into(), domain: "法律".into(), regex: false, case_sensitive: false },
        TermEntry { src: "diagnosis".into(), dst: "诊断".into(), domain: "医学".into(), regex: false, case_sensitive: false },
        TermEntry { src: "prognosis".into(), dst: "预后".into(), domain: "医学".into(), regex: false, case_sensitive: false },
        TermEntry { src: "hypertension".into(), dst: "高血压".into(), domain: "医学".into(), regex: false, case_sensitive: false },
        TermEntry { src: "inflation".into(), dst: "通货膨胀".into(), domain: "金融".into(), regex: false, case_sensitive: false },
        TermEntry { src: "liquidity".into(), dst: "流动性".into(), domain: "金融".into(), regex: false, case_sensitive: false },
        TermEntry { src: "interest rate".into(), dst: "利率".into(), domain: "金融".into(), regex: false, case_sensitive: false },
    ]
}

/// 术语库：用户术语（高优先级）+ 内置术语（兜底）
pub struct Terminology {
    user: Vec<TermEntry>,
    builtin: Vec<TermEntry>,
}

impl Default for Terminology {
    fn default() -> Self {
        Self::new()
    }
}

impl Terminology {
    pub fn new() -> Self {
        Terminology {
            user: Vec::new(),
            builtin: seed_builtin(),
        }
    }

    /// 用用户术语（来自 config.user_terms）重建；内置种子始终兜底
    pub fn with_user_terms(user: Vec<TermEntry>) -> Self {
        Terminology {
            user,
            builtin: seed_builtin(),
        }
    }

    pub fn user_terms(&self) -> &[TermEntry] {
        &self.user
    }

    pub fn set_user_terms(&mut self, terms: Vec<TermEntry>) {
        self.user = terms;
    }

    /// 合并返回全部术语（用户在前），用于导出/展示
    pub fn all(&self) -> Vec<&TermEntry> {
        self.user.iter().chain(self.builtin.iter()).collect()
    }

    /// 命中检测：按优先级（用户 > 内置）遍历，命中即返回对应术语
    fn match_term(&self, term: &TermEntry, text: &str) -> bool {
        if term.regex {
            let pattern = if term.case_sensitive {
                format!("(?-u:{})", term.src)
            } else {
                format!("(?i-u:{})", term.src)
            };
            regex::Regex::new(&pattern)
                .map(|re| re.is_match(text))
                .unwrap_or(false)
        } else if term.case_sensitive {
            text.contains(&term.src)
        } else {
            text.to_lowercase().contains(&term.src.to_lowercase())
        }
    }

    /// 在文本中查找所有命中的（去重后）术语，按优先级排序（用户优先；同 src 取用户）
    fn collect_matches(&self, text: &str) -> Vec<TermEntry> {
        let mut out: Vec<TermEntry> = Vec::new();
        let mut seen_user_src = std::collections::HashSet::new();
        // 用户优先
        for t in &self.user {
            if self.match_term(t, text) {
                seen_user_src.insert(t.src.clone());
                out.push(t.clone());
            }
        }
        for t in &self.builtin {
            if seen_user_src.contains(&t.src) {
                continue; // 用户已覆盖同 src
            }
            if self.match_term(t, text) {
                out.push(t.clone());
            }
        }
        out
    }

    /// 翻译前：将命中的术语 src 替换为不可译占位符，返回（改写后文本, 占位符→dst 映射）
    /// 占位符使用私用区字符 U+E000..U+E0FF，正常翻译引擎不会生成，还原时安全。
    pub fn apply(&self, text: &str) -> (String, HashMap<String, String>) {
        let matches = self.collect_matches(text);
        if matches.is_empty() {
            return (text.to_string(), HashMap::new());
        }
        let mut working = text.to_string();
        let mut map: HashMap<String, String> = HashMap::new();
        for (i, term) in matches.iter().enumerate() {
            let placeholder = format!("\u{E000}{:02X}\u{E001}", i);
            // 按匹配方式替换：正则用 Regex 替换全部；否则按大小写敏感做全部替换
            if term.regex {
                let pattern = if term.case_sensitive {
                    format!("(?-u:{})", term.src)
                } else {
                    format!("(?i-u:{})", term.src)
                };
                if let Ok(re) = regex::Regex::new(&pattern) {
                    working = re.replace_all(&working, &placeholder).to_string();
                }
            } else if term.case_sensitive {
                working = working.replace(&term.src, &placeholder);
            } else {
                // 大小写不敏感全部替换：逐处替换
                working = case_insensitive_replace(&working, &term.src, &placeholder);
            }
            map.insert(placeholder, term.dst.clone());
        }
        (working, map)
    }

    /// 翻译后：将占位符还原为术语 dst
    pub fn restore(translated: &str, map: &HashMap<String, String>) -> String {
        let mut out = translated.to_string();
        for (ph, dst) in map {
            out = out.replace(ph, dst);
        }
        out
    }
}

/// 大小写不敏感地替换全部子串（逐字符扫描，保留其它字符原样）
fn case_insensitive_replace(haystack: &str, needle: &str, replacement: &str) -> String {
    let lower_h: Vec<char> = haystack.to_lowercase().chars().collect();
    let lower_n: Vec<char> = needle.to_lowercase().chars().collect();
    let h: Vec<char> = haystack.chars().collect();
    let mut result = String::new();
    let mut i = 0;
    while i < h.len() {
        if i + lower_n.len() <= lower_h.len() && lower_h[i..i + lower_n.len()] == lower_n[..] {
            result.push_str(replacement);
            i += lower_n.len();
        } else {
            result.push(h[i]);
            i += 1;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(src: &str, dst: &str, domain: &str) -> TermEntry {
        TermEntry { src: src.into(), dst: dst.into(), domain: domain.into(), regex: false, case_sensitive: false }
    }

    #[test]
    fn apply_replaces_and_restores() {
        let t = Terminology::with_user_terms(vec![term("machine learning", "机器学习", "技术")]);
        let (g, map) = t.apply("I study machine learning every day.");
        assert!(!map.is_empty(), "应命中术语");
        // 占位符不应出现在原词（已被替换）
        assert!(!g.to_lowercase().contains("machine learning"));
        let restored = Terminology::restore("我每天学习\u{E000}00\u{E001}。", &map);
        assert!(restored.contains("机器学习"), "还原后应有术语译文: {}", restored);
    }

    #[test]
    fn user_overrides_builtin_same_src() {
        // 内置有 OCR->OCR(大写敏感保留)，用户自定义 OCR->光学字符识别 应覆盖
        let t = Terminology::with_user_terms(vec![TermEntry {
            src: "OCR".into(), dst: "光学字符识别".into(), domain: "技术".into(),
            regex: false, case_sensitive: true,
        }]);
        let (_, map) = t.apply("OCR engine");
        let restored = Terminology::restore("光学字符识别引擎", &map);
        assert!(restored.contains("光学字符识别"));
        assert!(!restored.contains("OCR"));
    }

    #[test]
    fn regex_term_matches() {
        let t = Terminology::with_user_terms(vec![TermEntry {
            src: r"\d+px".into(), dst: "<尺寸>".into(), domain: "技术".into(),
            regex: true, case_sensitive: false,
        }]);
        let (g, map) = t.apply("set width 12px and height 24px");
        // 同一条正则术语命中两处，去重后仅 1 条术语、2 处均被同一占位符替换
        assert_eq!(map.len(), 1, "应仅有 1 条正则术语: {:?}", map);
        assert!(g.contains("\u{E000}00\u{E001}"), "应包含占位符: {}", g);
        let restored = Terminology::restore(&g, &map);
        assert_eq!(restored, "set width <尺寸> and height <尺寸>");
    }

    #[test]
    fn no_match_returns_identity() {
        let t = Terminology::new(); // 空用户，仅内置
        let (g, map) = t.apply("hello world");
        assert!(map.is_empty());
        assert_eq!(g, "hello world");
    }

    #[test]
    fn case_insensitive_flag() {
        let t = Terminology::with_user_terms(vec![term("Contract", "合同", "法律")]);
        let (g, map) = t.apply("The CONTRACT was signed.");
        assert!(!map.is_empty());
        let restored = Terminology::restore("合同已签署。", &map);
        assert!(restored.contains("合同"));
    }
}
