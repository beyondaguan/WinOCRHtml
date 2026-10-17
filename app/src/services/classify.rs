// ============================================================
// Services · 采集分类引擎
//
// 两个标准框架（用户拍板 2026-10-08）：
//   1. 学科分类 → GB/T 13745-2009《学科分类与代码》医学子树（标准代码 + 名称）
//   2. 难度分级 → 常用字典分级方法：CEFR A1–C2 + 专业（医学专科词）
//
// 设计原则（正向保守，见对话记录）：
//   - 术语库（medical.json）命中优先 → 精确学科 + 专业 + 中文释义
//   - 词根/后缀兜底 → 推断学科 + 专业（仅当术语库未命中）
//   - 其余通用词 → 通用医学 + CEFR 档；medical_only 开时跳过
//   - 分级是「标签」不是「排除」：默认全收，前端/桌面可按 level 筛选
// ============================================================

/// GB/T 13745-2009 医学子树（节选常用学科），供 UI 下拉与展示。
/// 第一项为代码，第二项为标准学科名称。
pub const DISCIPLINES: &[(&str, &str)] = &[
    ("310", "基础医学"),
    ("320.24", "内科学"),
    ("320.2410", "心血管病学"),
    ("320.2415", "呼吸病学"),
    ("320.2425", "消化病学"),
    ("320.2430", "血液病学"),
    ("320.2435", "肾脏病学"),
    ("320.2440", "内分泌学"),
    ("320.2445", "风湿免疫病学"),
    ("320.2455", "感染病学"),
    ("320.27", "外科学"),
    ("320.2740", "泌尿外科学"),
    ("320.34", "儿科学"),
    ("320.37", "眼科学"),
    ("320.47", "皮肤病学"),
    ("320.54", "神经病学"),
    ("320.57", "精神病学"),
    ("320.67", "肿瘤学"),
    ("320.71", "护理学"),
    ("350", "药学"),
    ("360", "中医学与中药学"),
];

/// 词根/后缀 → 学科名称（术语库命中优先；此为兜底归类，覆盖主要医学系统）。
pub fn affix_discipline(word: &str) -> Option<&'static str> {
    let w = word.to_lowercase();
    // 泌尿（肾/膀胱/尿道）
    if contains_any(&w, &["nephr", "reno", "urin", "cyst", "kidney", "vesic", "prostat", "testis", "orchid"]) {
        return Some("泌尿外科学");
    }
    // 肾脏病学（更细，置于泌尿之后）
    if contains_any(&w, &["renal", "nephro"]) {
        return Some("肾脏病学");
    }
    // 心血管
    if contains_any(&w, &["cardi", "aort", "vascular", "heart", "arter", "ven", "thromb", "embol", "ischem", "angin", "myocard"]) {
        return Some("心血管病学");
    }
    // 呼吸
    if contains_any(&w, &["pneum", "pulm", "respir", "lung", "thorac", "asthma", "bronch"]) {
        return Some("呼吸病学");
    }
    // 消化
    if contains_any(&w, &["gastr", "hepat", "liver", "intestin", "colo", "rect", "pancrea", "esophag", "bowel", "bil"]) {
        return Some("消化病学");
    }
    // 血液
    if contains_any(&w, &["hemat", "hemo", "leuk", "lymph", "anemi", "coagul", "platelet", "clott"]) {
        return Some("血液病学");
    }
    // 内分泌
    if contains_any(&w, &["endocrin", "thyroid", "diabet", "pituitar", "adrenal", "insulin", "hormon"]) {
        return Some("内分泌学");
    }
    // 风湿免疫
    if contains_any(&w, &["rheumat", "arthr", "autoimmun", "lupus", "scler", "vascul"]) {
        return Some("风湿免疫病学");
    }
    // 感染
    if contains_any(&w, &["infect", "virus", "bacter", "sepsis", "tubercul", "fung", "malaria", "pathogen", "antibiot"]) {
        return Some("感染病学");
    }
    // 肿瘤
    if contains_any(&w, &["onc", "tumor", "carcin", "sarcoma", "lymphoma", "metasta", "neoplas"]) || w.ends_with("oma") {
        return Some("肿瘤学");
    }
    // 神经
    if contains_any(&w, &["neur", "brain", "cerebr", "spinal", "encephal", "neuro", "seiz", "epilep", "stroke"]) {
        return Some("神经病学");
    }
    // 皮肤
    if contains_any(&w, &["derm", "skin", "cutan", "eczem", "psorias"]) {
        return Some("皮肤病学");
    }
    // 骨科
    if contains_any(&w, &["orthop", "osteo", "bone", "fractur", "spine", "vertebr", "joint"]) {
        return Some("骨科学");
    }
    // 精神
    if contains_any(&w, &["psych", "schizophren", "depress", "bipol", "mania"]) {
        return Some("精神病学");
    }
    // 儿科
    if contains_any(&w, &["pediatr", "neonat", "infant", "child"]) {
        return Some("儿科学");
    }
    // 眼科
    if contains_any(&w, &["ophthalm", "retin", "corne", "eye", "vision"]) {
        return Some("眼科学");
    }
    None
}

fn contains_any(w: &str, roots: &[&str]) -> bool {
    roots.iter().any(|r| w.contains(r))
}

// ---------------- CEFR 分级词表（完整 A1–C2 七档，嵌入式） ----------------
// 数据：app/assets/terms/cefr_full.txt（8648 词）
//   来源：CEFR-J v1.5（A1–B2，东京外国语大学，商用免费需引用）+ Octanove C1/C2（CC BY-SA 4.0）
//   格式：每行 "word\tlevel"；同一词多档取最高档（最难）。
// 分级是「标签」不是「排除」：默认全收，前端/桌面可按 level 筛选。

fn cefr_map() -> &'static std::collections::HashMap<String, &'static str> {
    use std::sync::OnceLock;
    static MAP: OnceLock<std::collections::HashMap<String, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        include_str!("../../assets/terms/cefr_full.txt")
            .lines()
            .filter_map(|l| {
                let mut it = l.split('\t');
                let word = it.next()?.trim().to_lowercase();
                let level = it.next()?.trim();
                if word.is_empty() || level.is_empty() {
                    None
                } else {
                    Some((word, level))
                }
            })
            .collect()
    })
}

/// 难度分级：返回 CEFR 档位（A1/A2/B1/B2/C1/C2）；未命中 → "未分级"。
/// 注意：医学专科词在 ingest_text 中优先走术语库/词根，恒为「专业」，不经过本函数。
/// 词表存原形（lemma），采集到的常是屈折形式（islands/boxes/patient's），
/// 因此精确未命中时按 后缀剥离 回退再查一轮。
pub fn cefr_tier(word: &str) -> &'static str {
    let w = word.to_lowercase();
    if let Some(t) = cefr_map().get(&w).copied() {
        return t;
    }
    // 词形归一：所有格 → 复数 s → 复数 es（顺序重要：先剥 's 再剥 s/es）
    let fallbacks = [
        w.strip_suffix("'s"),
        w.strip_suffix('s'),
        w.strip_suffix("es"),
    ];
    for base in fallbacks.into_iter().flatten() {
        if let Some(t) = cefr_map().get(base).copied() {
            return t;
        }
    }
    "未分级"
}

/// 单条分类结果。
pub struct Classified {
    pub project: String, // 学科名称（GB/T 13745）
    pub level: String,   // 难度档
    pub is_medical: bool,
}

/// 正向保守分类：忽略词 → None；词根命中 → 医学学科 + 专业；否则 → 通用医学 + CEFR 档。
/// 术语库命中由调用方（ingest_text）优先处理，不走本函数。
pub fn classify(word: &str, ignore: &[String]) -> Option<Classified> {
    let w = word.to_lowercase();
    if w.len() < 2 {
        return None;
    }
    if ignore.iter().any(|x| x.eq_ignore_ascii_case(&w)) {
        return None;
    }
    if let Some(disc) = affix_discipline(&w) {
        return Some(Classified {
            project: disc.to_string(),
            level: "专业".to_string(),
            is_medical: true,
        });
    }
    Some(Classified {
        project: "通用医学".to_string(),
        level: cefr_tier(&w).to_string(),
        is_medical: false,
    })
}
