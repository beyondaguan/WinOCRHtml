// ============================================================
// Services · 生词增强（enrich）
//
// 四卡片学习工作台的数据回填服务：
//   - 释义卡：open-dictionary（英英释义 + 例句）
//   - 发音卡：word2ipa + CMUdict（IPA 音标，168k 词）
//   - 词根卡：etymology-db 英语精选子集（词根拆解 + 词源链，13k 词）
//
// 数据文件（均独立于 exe，运行时从 %APPDATA%/WinOCR/dict/ 读取）：
//   - open-dictionary/  （目录，703 分片）
//   - ipa_en.json       （word  → ipa）
//   - etymology_en_subset.json（word → {roots_text, etymology_chain, relations}）
//
// 多词短语（如 "myocardial infarction"）会拆成各单词分别查词根，
// 合并展示，覆盖短语类医学词。
//
// 若数据未就绪（未下载/未同步），enrich 静默返回 None，不影响其他功能。
// ============================================================
use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;

use super::dict::Dict;
use super::persist::data_dir;

/// 增强结果（回填到 CollectItem）
#[derive(Debug, Clone, Default)]
pub struct EnrichedInfo {
    pub phonetic: String,
    pub definition_en: String,
    pub example: String,
    pub example_zh: String,
    pub roots: String,
    pub etymology: String,
}

/// 词源条目（etymology_en_subset.json）
#[derive(Debug, Deserialize)]
struct EtymologyEntry {
    #[serde(default)]
    roots_text: String,
    #[serde(default)]
    etymology_chain: String,
}

/// 增强服务
pub struct Enrich {
    dict: Dict,
}

// ── 进程级缓存（2026-10-17 批量补全引入）──
// 旧实现每次 enrich() 重新读 5.9MB ipa + 14MB etymology JSON；批量 250 词 = 5GB IO。
// 改 OnceLock 整表缓存（与 dict::phonetic_of 同模式），首次加载 ~200ms，之后内存直查。
// 返回 (数据是否就绪, 表)；未就绪时表为空 map，查询恒 None。
fn ipa_map() -> &'static (bool, HashMap<String, String>) {
    static IPA: std::sync::OnceLock<(bool, HashMap<String, String>)> = std::sync::OnceLock::new();
    IPA.get_or_init(|| {
        let path = data_dir().join("dict").join("ipa_en.json");
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, String>>(&bytes) {
                Ok(m) => (true, m),
                Err(e) => {
                    log::warn!("[services::enrich] 解析音标数据失败（忽略）: {e}");
                    (false, HashMap::new())
                }
            },
            Err(_) => (false, HashMap::new()),
        }
    })
}

fn etymology_map() -> &'static (bool, HashMap<String, EtymologyEntry>) {
    static ETY: std::sync::OnceLock<(bool, HashMap<String, EtymologyEntry>)> =
        std::sync::OnceLock::new();
    ETY.get_or_init(|| {
        let path = data_dir().join("dict").join("etymology_en_subset.json");
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<HashMap<String, EtymologyEntry>>(&bytes) {
                Ok(m) => (true, m),
                Err(e) => {
                    log::warn!("[services::enrich] 解析词源数据失败（忽略）: {e}");
                    (false, HashMap::new())
                }
            },
            Err(_) => (false, HashMap::new()),
        }
    })
}

impl Enrich {
    pub fn new() -> Self {
        Self { dict: Dict::new() }
    }

    /// 查询单词的词根拆解 + 词源链。
    /// 多词短语拆成各单词分别查，合并展示。
    /// 词源链截断到 MAX_CHAIN_CHARS，避免 UI 展示过长。
    const MAX_CHAIN_CHARS: usize = 300;

    fn lookup_etymology(
        &self,
        data: &HashMap<String, EtymologyEntry>,
        word: &str,
    ) -> (String, String) {
        let mut roots_parts: Vec<String> = Vec::new();
        let mut chain_parts: Vec<String> = Vec::new();
        let mut seen_roots = std::collections::HashSet::new();
        let mut seen_chain = std::collections::HashSet::new();

        // 先查整词，再拆词查
        let mut candidates: Vec<String> = Vec::new();
        candidates.push(word.to_string());
        for part in word.split_whitespace() {
            let p = part.trim().to_lowercase();
            if !p.is_empty() && p != word {
                candidates.push(p);
            }
        }

        for cand in candidates {
            if let Some(e) = data.get(&cand) {
                let r = e.roots_text.trim();
                if !r.is_empty() && seen_roots.insert(r.to_string()) {
                    roots_parts.push(r.to_string());
                }
                let c = e.etymology_chain.trim();
                if !c.is_empty() && seen_chain.insert(c.to_string()) {
                    chain_parts.push(c.to_string());
                }
            }
        }

        let roots = roots_parts.join("\n");
        let chain_full = chain_parts.join("\n");
        let chain = if chain_full.chars().count() > Self::MAX_CHAIN_CHARS {
            let mut truncated: String = chain_full.chars().take(Self::MAX_CHAIN_CHARS).collect();
            truncated.push('…');
            truncated
        } else {
            chain_full
        };

        (roots, chain)
    }

    /// 对单词做全量增强。数据未就绪时返回 None。
    pub fn enrich(&self, word: &str) -> Result<Option<EnrichedInfo>> {
        let w = word.trim().to_lowercase();
        if w.is_empty() {
            return Ok(None);
        }
        let mut info = EnrichedInfo::default();

        // 1. 释义来源按 config.study_dict 切换（2026-10-17 用户需求：词典切换入口）
        //    例句永远来自 open-dictionary（ECDICT 无例句），英英模式下释义也由它提供。
        let dict_mode = super::dict::study_dict_name();
        if let Some(entry) = self.dict.lookup(&w)? {
            info.example = entry.examples.join("\n");
            if dict_mode != "ecdict" {
                // 英英模式（默认）：完整词典全文格式（词性分块+义项编号+例句+词源）
                info.definition_en = super::dict::format_full(&entry);
            }
        }
        if dict_mode == "ecdict" && super::dict::ecdict_available() {
            // 英汉模式：ECDICT 中文释义为主（音标作 ipa_en.json 缺失时的兜底）
            if let Some(e) = super::dict::lookup_ecdict(&w)? {
                info.definition_en = super::dict::format_full_zh(&e);
                if info.phonetic.is_empty() {
                    info.phonetic = e.phonetic;
                }
            }
        }

        // 2. 音标（word2ipa + CMUdict 合并；进程级缓存）
        let (ipa_ok, ipa_data) = ipa_map();
        if *ipa_ok {
            if let Some(ipa) = ipa_data.get(&w) {
                info.phonetic = ipa.clone();
            }
        }

        // 3. 词根 + 词源（etymology-db 精选子集，多词拆分；进程级缓存）
        let (ety_ok, ety_data) = etymology_map();
        if *ety_ok {
            let (roots, chain) = self.lookup_etymology(ety_data, &w);
            info.roots = roots;
            info.etymology = chain;
        }

        // 全部为空 → 视为未命中（避免无意义回填）
        if info.phonetic.is_empty()
            && info.definition_en.is_empty()
            && info.example.is_empty()
            && info.roots.is_empty()
            && info.etymology.is_empty()
        {
            return Ok(None);
        }

        Ok(Some(info))
    }
}

impl Default for Enrich {
    fn default() -> Self {
        Self::new()
    }
}