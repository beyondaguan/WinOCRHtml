// ============================================================
// 术语库服务层
// 移植自 extension-0.8/js/modules/terms.js
//
// 职责：
//   1. 编译期嵌入内置医学词库（assets/terms/medical.json）
//   2. 运行时管理用户自定义术语（add/remove/query）
//   3. 翻译前：术语 → 占位符 ⟦T0⟧
//   4. 翻译后：占位符 → 中文术语
// ============================================================

use std::collections::HashMap;
use serde::Deserialize;

/// 单条内置术语（medical.json 反序列化目标）。
///
/// `zh` 中文释义；`cat` 学科分类（GB/T 13745 名称，如 "泌尿外科学"）；
/// `level` 难度档（医学专科词恒为 "专业"）。
#[derive(Debug, Clone, Deserialize)]
pub struct TermEntry {
    pub zh: String,
    pub cat: String,
    pub level: String,
}

/// 术语库数据库。
///
/// 内置术语通过 `include_str!` 在编译期嵌入二进制文件，结构为 `{ en: {zh, cat, level} }`；
/// 自定义术语在运行时通过 `add_custom` / `remove_custom` 管理（仅 en→zh）。
///
/// 设计为 `Clone` 友好，方便多线程共享（通过 `Arc<TermDb>` 包装）。
#[derive(Clone)]
pub struct TermDb {
    builtin: HashMap<String, TermEntry>,
    custom: HashMap<String, String>,
}

impl TermDb {
    /// 编译期嵌入内置医学词库并创建 TermDb 实例。
    ///
    /// JSON 文件路径相对于本文件位置：`../../assets/terms/medical.json`。
    /// 解析失败时回退为空词库（`unwrap_or_default`）。
    pub fn new() -> Self {
        let medical = include_str!("../../assets/terms/medical.json");
        let builtin: HashMap<String, TermEntry> = serde_json::from_str(medical).unwrap_or_default();
        Self {
            builtin,
            custom: HashMap::new(),
        }
    }

    /// 合并内置 + 自定义术语，返回完整的「英文 → 中文」映射表（供翻译占位逻辑使用）。
    pub fn full_map(&self) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for (k, v) in &self.builtin {
            map.insert(k.clone(), v.zh.clone());
        }
        map.extend(self.custom.clone());
        map
    }

    /// 精确查询内置术语条目（含学科 + 难度档）。大小写不敏感。
    pub fn entry(&self, word: &str) -> Option<&TermEntry> {
        self.builtin
            .get(word)
            .or_else(|| self.builtin.get(&word.to_lowercase()))
    }

    /// 查询学科分类（GB/T 13745 名称）。
    pub fn category_of(&self, word: &str) -> Option<&str> {
        self.entry(word).map(|e| e.cat.as_str())
    }

    /// 查询难度档（专业 / 基础 / 未分级）。
    pub fn level_of(&self, word: &str) -> Option<&str> {
        self.entry(word).map(|e| e.level.as_str())
    }

    /// 翻译前：将文本中的术语替换为占位符 ⟦T{i}⟧。
    ///
    /// 按术语长度降序排序后替换，避免短术语先替换导致长术语无法匹配。
    /// 跳过空目标术语的条目。
    ///
    /// 返回：替换后的文本 + 占位符→术语的还原映射。
    pub fn apply(&self, text: &str) -> (String, HashMap<String, String>) {
        if text.is_empty() {
            return (text.to_string(), HashMap::new());
        }

        let mut result = text.to_string();
        let mut restore_map = HashMap::new();
        let mut keys: Vec<_> = self.full_map().keys().cloned().collect();

        // 长的先替换，避免短术语覆盖长术语
        keys.sort_by(|a, b| b.len().cmp(&a.len()));

        for (i, src) in keys.iter().enumerate() {
            let tgt = &self.full_map()[src];
            if tgt.is_empty() {
                continue;
            }
            let placeholder = format!("⟦T{}⟧", i);
            if result.contains(src) {
                restore_map.insert(placeholder.clone(), tgt.clone());
                result = result.replace(src, &placeholder);
            }
        }

        (result, restore_map)
    }

    /// 翻译后：将占位符 ⟦T{i}⟧ 替换回原始术语。
    pub fn restore(&self, text: &str, map: &HashMap<String, String>) -> String {
        let mut result = text.to_string();
        for (placeholder, term) in map {
            result = result.replace(placeholder, term);
        }
        result
    }

    /// 添加或更新一条自定义术语。
    pub fn add_custom(&mut self, src: &str, tgt: &str) {
        self.custom.insert(src.to_string(), tgt.to_string());
    }

    /// 删除一条自定义术语。
    pub fn remove_custom(&mut self, src: &str) {
        self.custom.remove(src);
    }

    /// 获取当前自定义术语列表的不可变引用。
    #[allow(dead_code)] // v1.1: 术语库 UI 展示
    pub fn custom_terms(&self) -> &HashMap<String, String> {
        &self.custom
    }

    /// 获取内置术语数量（用于调试/UI 展示）。
    pub fn builtin_count(&self) -> usize {
        self.builtin.len()
    }

    /// 获取自定义术语数量（用于调试/UI 展示）。
    pub fn custom_count(&self) -> usize {
        self.custom.len()
    }

    /// 获取全部术语（内置 + 自定义）数量。
    #[allow(dead_code)] // v1.1: 术语库统计
    pub fn total_count(&self) -> usize {
        self.full_map().len()
    }

    /// 清空所有自定义术语。
    #[allow(dead_code)] // v1.1: 术语库清空
    pub fn clear_custom(&mut self) {
        self.custom.clear();
    }

    /// 批量导入自定义术语（覆盖式）。
    #[allow(dead_code)] // v1.1: 术语库导入
    pub fn import_custom(&mut self, terms: HashMap<String, String>) {
        self.custom = terms;
    }

    /// 批量导入自定义术语（合并式，不覆盖已有条目）。
    pub fn merge_custom(&mut self, terms: &HashMap<String, String>) {
        for (src, tgt) in terms {
            self.custom.entry(src.clone()).or_insert_with(|| tgt.clone());
        }
    }
}

impl Default for TermDb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_text() {
        let db = TermDb::new();
        let (result, map) = db.apply("");
        assert_eq!(result, "");
        assert!(map.is_empty());
    }

    #[test]
    fn test_apply_and_restore() {
        let mut db = TermDb::new();
        db.add_custom("hello world", "你好世界");

        let text = "This is hello world test";
        let (replaced, map) = db.apply(text);
        assert!(replaced.contains("⟦T"));
        assert_eq!(map.len(), 1);

        let restored = db.restore(&replaced, &map);
        assert_eq!(restored, text);
    }

    #[test]
    fn test_longer_term_first() {
        let mut db = TermDb::new();
        // "short" 和 "short term" 同时存在时，长术语优先替换
        db.add_custom("short", "短");
        db.add_custom("short term", "短期");

        let text = "This is a short term test";
        let (replaced, map) = db.apply(text);
        // 应该匹配到 "short term" 而不是 "short"
        assert!(map.values().any(|v| v == &"短期".to_string()));
    }

    #[test]
    fn test_custom_terms_crud() {
        let mut db = TermDb::new();
        assert_eq!(db.custom_count(), 0);

        db.add_custom("test", "测试");
        assert_eq!(db.custom_count(), 1);
        assert_eq!(db.custom_terms().get("test"), Some(&"测试".to_string()));

        db.remove_custom("test");
        assert_eq!(db.custom_count(), 0);
    }

    #[test]
    fn test_import_custom() {
        let mut db = TermDb::new();
        let mut terms = HashMap::new();
        terms.insert("foo".to_string(), "bar".to_string());
        terms.insert("baz".to_string(), "qux".to_string());

        db.import_custom(terms);
        assert_eq!(db.custom_count(), 2);
    }
}
