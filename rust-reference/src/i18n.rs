//! i18n 模块（§17.8）：UI 字符串外置，支持简体中文与英文。
//!
//! 设计：编译期静态对照表，避免运行期解析文件。`locale()` 返回当前语言
//! （由配置 `ui_lang` 决定，默认 `zh`），`tr(key)` 按 locale 返回对应文案。
//! 所有用户可见字符串统一走 `tr!` / `tr()`，便于后续扩展更多语言。

use std::sync::{Mutex, OnceLock};

static LOCALE: OnceLock<Mutex<String>> = OnceLock::new();

/// 设置当前 UI 语言（"zh" / "en"），非法值回退 zh。
pub fn set_locale(lang: &str) {
    let l = if lang == "en" { "en" } else { "zh" }.to_string();
    let g = LOCALE.get_or_init(|| Mutex::new("zh".to_string()));
    *g.lock().unwrap() = l;
}

/// 读取当前 UI 语言。
pub fn locale() -> String {
    let g = LOCALE.get_or_init(|| Mutex::new("zh".to_string()));
    g.lock().unwrap().clone()
}

/// 取某 key 在当前 locale 下的文案。
pub fn tr<'a>(key: &'a str) -> &'a str {
    let zh = ZH.get_or_init(build_zh);
    let en = EN.get_or_init(build_en);
    if locale() == "en" {
        en.get(key)
            .copied()
            .unwrap_or_else(|| zh.get(key).copied().unwrap_or(key))
    } else {
        zh.get(key).copied().unwrap_or(key)
    }
}

/// 宏：tr!("copy") => tr("copy")
#[macro_export]
macro_rules! tr {
    ($k:expr) => {
        $crate::i18n::tr($k)
    };
}

use std::collections::HashMap;

fn build_zh() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    // 通用
    m.insert("app_name", "WinOCR 译文");
    m.insert("copy", "复制");
    m.insert("retranslate", "重译");
    m.insert("switch_engine", "换引擎");
    m.insert("favorite", "收藏");
    m.insert("pin", "固定");
    m.insert("pinned", "已固定");
    m.insert("close", "关闭");
    m.insert("edit_hint", "双击编辑原文");
    m.insert("original", "原文：");
    m.insert("translation", "译文：");
    m.insert("no_text", "没识别到文字");
    m.insert("retry", "重试");
    m.insert("open_settings", "打开设置");
    m.insert("copy_diagnostic", "复制诊断");
    m.insert("copied", "已复制到剪贴板");
    m.insert("favorited", "已加入生词本");
    m.insert("retranslating", "正在重新翻译…");
    m.insert("empty_region", "没识别到文字，请重试");
    m.insert("select_hint", "拖框选择要翻译的区域，Esc 取消");
    // 历史/生词本
    m.insert("history_title", "历史记录");
    m.insert("vocab_title", "生词本");
    m.insert("review_title", "复习");
    m.insert("search", "搜索：");
    m.insert("clear_all", "清空");
    m.insert("export", "导出");
    m.insert("export_anki", "导出 Anki");
    m.insert("review", "复习");
    m.insert("due", "待复习");
    m.insert("mastered", "已掌握");
    m.insert("today_new", "今日新增");
    m.insert("total", "总数");
    m.insert("flip_hint", "空格翻面 / 1-3 打分 / Esc 退出");
    m.insert("know", "认识");
    m.insert("fuzzy", "模糊");
    m.insert("forget", "忘记");
    m.insert("no_due", "暂无待复习词");
    m.insert("no_history", "暂无历史记录");
    m.insert("confirm_clear", "确认清空全部？此操作不可恢复");
    // 设置
    m.insert("ui_lang", "界面语言：");
    m.insert("magnifier", "选区放大镜：");
    m.insert("snap", "边缘吸附：");
    m.insert("on", "开");
    m.insert("off", "关");
    m.insert("appearance", "外观");
    m.insert("language", "语言");
    m.insert("selection", "选区");
    m.insert("history", "历史");
    m.insert("vocab", "生词本");
    m.insert("history_title", "历史记录");
    m.insert("vocab_title", "生词本");
    m.insert("review_title", "复习");
    m.insert("no_due", "暂无待复习词");
    m.insert("know", "认识");
    m.insert("fuzzy", "模糊");
    m.insert("forget", "忘记");
    m.insert("flip_hint", "空格翻面 / 1-3 打分 / Esc 退出");
    m.insert("clear_all", "清空");
    m.insert("review", "复习");
    m.insert("export", "导出");
    m.insert("export_anki", "导出 Anki");
    m.insert("search", "搜索：");
    m.insert("confirm_clear", "确认清空全部？此操作不可恢复");
    // Toast 段式
    m.insert("toast_failed", "操作失败");
    m.insert("toast_why", "原因见上方日志");
    m.insert("toast_what_now", "可重试或打开设置");
    m
}

fn build_en() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("app_name", "WinOCR Translation");
    m.insert("copy", "Copy");
    m.insert("retranslate", "Re-translate");
    m.insert("switch_engine", "Engine");
    m.insert("favorite", "Save");
    m.insert("pin", "Pin");
    m.insert("pinned", "Pinned");
    m.insert("close", "Close");
    m.insert("edit_hint", "Double-click to edit source");
    m.insert("original", "Source: ");
    m.insert("translation", "Translation: ");
    m.insert("no_text", "No text recognized");
    m.insert("retry", "Retry");
    m.insert("open_settings", "Settings");
    m.insert("copy_diagnostic", "Copy log");
    m.insert("copied", "Copied to clipboard");
    m.insert("favorited", "Added to vocabulary");
    m.insert("retranslating", "Re-translating…");
    m.insert("empty_region", "No text recognized, retry");
    m.insert("select_hint", "Drag to select a region, Esc to cancel");
    m.insert("history_title", "History");
    m.insert("vocab_title", "Vocabulary");
    m.insert("review_title", "Review");
    m.insert("search", "Search: ");
    m.insert("clear_all", "Clear");
    m.insert("export", "Export");
    m.insert("export_anki", "Export Anki");
    m.insert("review", "Review");
    m.insert("due", "Due");
    m.insert("mastered", "Mastered");
    m.insert("today_new", "New today");
    m.insert("total", "Total");
    m.insert("flip_hint", "Space flip / 1-3 grade / Esc exit");
    m.insert("know", "Know");
    m.insert("fuzzy", "Fuzzy");
    m.insert("forget", "Forget");
    m.insert("no_due", "Nothing due");
    m.insert("no_history", "No history yet");
    m.insert("confirm_clear", "Clear all? This cannot be undone");
    m.insert("ui_lang", "UI language: ");
    m.insert("magnifier", "Selection magnifier: ");
    m.insert("snap", "Edge snapping: ");
    m.insert("on", "On");
    m.insert("off", "Off");
    m.insert("appearance", "Appearance");
    m.insert("language", "Language");
    m.insert("selection", "Selection");
    m.insert("history", "History");
    m.insert("vocab", "Vocab");
    m.insert("history_title", "History");
    m.insert("vocab_title", "Vocabulary");
    m.insert("review_title", "Review");
    m.insert("no_due", "Nothing due");
    m.insert("know", "Know");
    m.insert("fuzzy", "Fuzzy");
    m.insert("forget", "Forget");
    m.insert("flip_hint", "Space flip / 1-3 grade / Esc exit");
    m.insert("clear_all", "Clear");
    m.insert("review", "Review");
    m.insert("export", "Export");
    m.insert("export_anki", "Export Anki");
    m.insert("search", "Search: ");
    m.insert("confirm_clear", "Clear all? This cannot be undone");
    m.insert("toast_failed", "Operation failed");
    m.insert("toast_why", "See log above for details");
    m.insert("toast_what_now", "Retry or open settings");
    m
}

static ZH: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
static EN: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

/// 在程序启动、加载配置后调用一次，初始化静态表与 locale。
pub fn init() {
    ZH.get_or_init(build_zh);
    EN.get_or_init(build_en);
}
