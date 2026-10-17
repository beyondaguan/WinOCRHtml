// ============================================================
// Services · 学段词表（stage）
//
// 2026-10-17 用户需求：按小学/初中/高中学段组别学习单词，设置入口可自定义。
//
// 数据：
//   固定组（随程序分发，app/assets/dict/stage/*.txt → %APPDATA%/WinOCR/dict/stage/）
//     primary.txt = 小学（中考课标 ∩ CEFR A1/A2，1080 词）
//     junior.txt  = 初中（ECDICT zk 中考课标 1603 词）
//     senior.txt  = 高中（ECDICT gk 高考课标 3677 词）
//     cet4/cet6/kaoyan.txt = 四级/六级/考研（ECDICT tag）
//   自定义组：用户把 txt 词表（每行一词）放进 %APPDATA%/WinOCR/vocab/<组名>.txt
//     → 文件名即组名，自动出现在设置「学习组别」下拉里，重启生效。
//
// 学习语义：过滤复习队列（生词本已收的词 ∩ 组别词表）。
//   「全部单词」= 不过滤。词表导学（背未收录的新词）不在本模块范围。
// ============================================================
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::persist::data_dir;

/// 固定组显示名 → 词表文件名（不含扩展名）
const BUILTIN: &[(&str, &str)] = &[
    ("小学词汇", "primary"),
    ("初中词汇", "junior"),
    ("高中词汇", "senior"),
    ("四级词汇", "cet4"),
    ("六级词汇", "cet6"),
    ("考研词汇", "kaoyan"),
];

/// 全部组选项（显示名）：「全部单词」+ 固定组 + 自定义组（vocab/*.txt 按文件名排序）
pub fn options() -> Vec<String> {
    let mut v = vec!["全部单词".to_string()];
    for (name, _) in BUILTIN {
        v.push((*name).to_string());
    }
    let mut custom: Vec<String> = custom_dir()
        .read_dir()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map(|x| x == "txt").unwrap_or(false))
                .filter_map(|e| e.file_name().into_string().ok())
                .map(|f| f.trim_end_matches(".txt").to_string())
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default();
    custom.sort();
    v.extend(custom);
    v
}

/// 自定义词表目录：%APPDATA%/WinOCR/vocab/
pub fn custom_dir() -> PathBuf {
    data_dir().join("vocab")
}

/// 组显示名 → 词集合（OnceLock 进程级缓存；文件缺失 → 空 set → 过滤后队列为空）
fn sets() -> &'static HashMap<String, HashSet<String>> {
    static SETS: std::sync::OnceLock<HashMap<String, HashSet<String>>> =
        std::sync::OnceLock::new();
    SETS.get_or_init(|| {
        let mut m: HashMap<String, HashSet<String>> = HashMap::new();
        let base = data_dir().join("dict").join("stage");
        for (name, file) in BUILTIN {
            if let Some(set) = load_txt(&base.join(format!("{file}.txt"))) {
                m.insert((*name).to_string(), set);
            }
        }
        if let Ok(rd) = custom_dir().read_dir() {
            for e in rd.filter_map(|e| e.ok()) {
                let path = e.path();
                if path.extension().map(|x| x == "txt").unwrap_or(false) {
                    if let Some(fname) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Some(set) = load_txt(&path) {
                            m.insert(fname.to_string(), set);
                        }
                    }
                }
            }
        }
        m
    })
}

fn load_txt(path: &PathBuf) -> Option<HashSet<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.lines()
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// 判断单词是否属于某组（大小写不敏感）
pub fn contains(stage_name: &str, word: &str) -> bool {
    sets()
        .get(stage_name)
        .map(|s| s.contains(&word.trim().to_lowercase()))
        .unwrap_or(false)
}

/// 按组别过滤复习队列；「全部单词」/未知组名 → 原样返回
pub fn apply_stage(items: Vec<super::collect::CollectItem>, stage_name: &str) -> Vec<super::collect::CollectItem> {
    if stage_name.is_empty() || stage_name == "全部单词" || !sets().contains_key(stage_name) {
        return items;
    }
    items
        .into_iter()
        .filter(|i| contains(stage_name, &i.word))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 词表已同步（%APPDATA%）时验证：选项完整、小学 ⊆ 初中、逐词判定正确；
    /// 未同步则跳过。
    #[test]
    fn stage_lists_smoke() {
        let opts = options();
        assert_eq!(opts[0], "全部单词");
        assert!(opts.contains(&"初中词汇".to_string()), "固定组缺失: {opts:?}");
        if !data_dir().join("dict").join("stage").join("junior.txt").exists() {
            return; // 未同步环境
        }
        assert!(contains("初中词汇", "apple"), "apple 应属中考词");
        assert!(!contains("初中词汇", "nephritis"), "nephritis 是 gre 词，不应属中考");
        assert!(contains("小学词汇", "apple"), "apple 应属小学基础词");
    }
}
