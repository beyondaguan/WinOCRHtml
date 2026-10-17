// ============================================================
// Core · 共享类型（UI / Services 之间的契约）
// 严格不依赖任何具体实现。Services 引用本模块，UI 通过 Slint 属性桥接。
// 这是三层架构中唯一允许被 UI 与 Services 双向引用的词汇层。
// ============================================================
use serde::{Deserialize, Serialize};
use std::fmt;

/// 翻译语言。与 main_window.slint 顶栏语言对 ComboBox 的 model 顺序对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lang {
    Auto,
    En,
    Ja,
    Ko,
    Zh,
}

impl Lang {
    /// 引擎使用的语言码（PP-OCR / Argos / 在线 API 通用）。
    pub fn code(&self) -> &'static str {
        match self {
            Lang::Auto => "auto",
            Lang::En => "en",
            Lang::Ja => "ja",
            Lang::Ko => "ko",
            Lang::Zh => "zh",
        }
    }

    /// 由语言码解析（未知 / 空 → Auto）。
    #[allow(dead_code)] // 外部配置 → Lang 解析
    pub fn from_code(c: &str) -> Self {
        match c.to_ascii_lowercase().as_str() {
            "en" => Lang::En,
            "ja" | "ja-jp" => Lang::Ja,
            "ko" | "ko-kr" => Lang::Ko,
            "zh" | "zh-cn" | "zh_cn" | "zh-tw" => Lang::Zh,
            _ => Lang::Auto,
        }
    }

    /// 顶栏 ComboBox 的显示名（与 slint 的 model 文案一致）。
    pub fn label(&self) -> &'static str {
        match self {
            Lang::Auto => "自动检测",
            Lang::En => "English",
            Lang::Ja => "日本語",
            Lang::Ko => "한국어",
            Lang::Zh => "简体中文",
        }
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// 工作区模式：双栏（源/译）⇄ AI 对话。
#[allow(dead_code)] // v1.1: AI 对话模式切换
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkMode {
    Panes,
    Ai,
}

/// OCR 结果（含识别文本；后续可扩展文本框坐标供版面还原）。
#[allow(dead_code)] // v1.1: 版面还原
#[derive(Debug, Clone, Default)]
pub struct OcrResult {
    pub text: String,
}

impl OcrResult {
    #[allow(dead_code)] // v1.1: OCR 新 API 入口
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
    #[allow(dead_code)] // v1.1: 统计接口
    pub fn char_count(&self) -> usize {
        self.text.chars().count()
    }
}

/// 翻译结果（含译文与命中引擎，便于状态条展示回退链）。
#[derive(Debug, Clone, Default)]
pub struct TranslateResult {
    pub text: String,
    pub engine: String,
}

impl TranslateResult {
    pub fn new(text: impl Into<String>, engine: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            engine: engine.into(),
        }
    }
}

/// 历史记录条目（唯一真源，开发指南 §4.1）。
/// 13 字段规格：id/ts/src/tgt/src_lang/tgt_lang/engine/screenshot/project/
///             collected/note/tags/exported
/// 保留 favorited 字段以兼容存量 JSONL 数据反序列化（Phase 2 迁移后仍保留，
/// 但不再参与 UI 主流程，避免破坏旧数据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub ts: i64,
    pub src: String,
    pub tgt: String,
    pub src_lang: String,
    pub tgt_lang: String,
    pub engine: String,
    /// 截图路径（相对 data_dir 或绝对路径；空表示无截图）
    #[serde(default)]
    pub screenshot: String,
    /// 项目/来源名（默认「未分类」）
    #[serde(default)]
    pub project: String,
    /// 是否已采集（0/1）
    #[serde(default)]
    pub collected: bool,
    /// 自定义释义
    #[serde(default)]
    pub note: String,
    /// 逗号分隔的标签
    #[serde(default)]
    pub tags: String,
    /// 是否已导出（0/1）
    #[serde(default)]
    pub exported: bool,
    /// 兼容旧数据（v1.x 字段，迁移后不再参与 UI 主流程）
    #[serde(default)]
    pub favorited: bool,
}

/// 跨层事件（Core 定义词汇；Services 发布，UI / 其他 Services 订阅）。
/// Phase 2 起由 event_bus::EventBus 承载（tray / persist / ai 等常驻服务）。
#[allow(dead_code)] // v1.1: 事件总线接入
#[derive(Debug, Clone)]
pub enum AppEvent {
    CaptureRequested,
    Navigate(String),
    ModeChanged(WorkMode),
    OcrResult(OcrResult),
    TranslateResult(TranslateResult),
    Status(String),
}
