// ============================================================
// Core · 配置数据模型
// 纯数据 + JSON 读写。密钥 DPAPI 加密落盘（P2-2）。
//
// 落盘路径：%APPDATA%/WinOCR/config.json
//
// 加密字段（磁盘上 *_enc 替代明文）：
//   sf_key           → sf_key_enc           SiliconFlow 翻译 API Key
//   ai_sf_key        → ai_sf_key_enc        SiliconFlow AI API Key
//   youdao_app_key   → youdao_app_key_enc   有道 App Key
//   youdao_app_secret→ youdao_app_secret_enc 有道 App Secret
//
// 设计：
//   - 内存中始终持明文（供 services 层直接使用）
//   - 落盘前加密为 *_enc base64，明文字段置空
//   - 加载时解密 *_enc 到明文字段；旧版无 *_enc 则回退明文字段（自动迁移）
// ============================================================
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 源语言码（core::types::Lang::code，如 "auto" / "en" / "zh"）
    pub lang_from: String,
    /// 目标语言码
    pub lang_to: String,
    /// OCR 引擎标识（"ppocr" 主引擎 / "system" 兜底）
    pub ocr_engine: String,
    /// 翻译引擎标识（"argos" 离线主引擎，回退链见计划文档）
    pub translate_engine: String,
    /// 是否启用结果缓存（Phase 3 历史/生词）
    pub cache_enabled: bool,
    /// 是否启用桌面端划词气泡（浏览器扩展连接时建议关闭，避免双气泡冲突）
    #[serde(default = "d_selection_bubble_enabled")]
    pub selection_bubble_enabled: bool,
    /// 浏览器扩展是否已连接（运行时状态，不持久化）
    #[serde(skip)]
    pub browser_extension_connected: bool,

    // —— 以下为翻译引擎参数（移植自 rust-reference/config.rs）——
    /// Ollama 本地 LLM 地址（默认 11434）。本地运行，不属于"云端"。
    #[serde(default = "d_ollama_url")]
    pub ollama_url: String,
    /// Ollama 模型名（为空则 ollama 引擎不可用，报明确错误）
    #[serde(default)]
    pub ollama_model: String,
    /// SiliconFlow（OpenAI 兼容）地址
    #[serde(default = "d_sf_url")]
    pub sf_url: String,
    /// SiliconFlow API Key（内存明文；落盘时加密为 sf_key_enc）
    #[serde(default)]
    pub sf_key: String,
    /// DPAPI 加密后的 sf_key（base64）
    #[serde(default)]
    pub sf_key_enc: Option<String>,
    /// SiliconFlow 模型名
    #[serde(default = "d_sf_model")]
    pub sf_model: String,
    /// MyMemory 可选邮箱（填上可提升免费配额）
    #[serde(default)]
    pub mymemory_email: String,
    /// 有道智云 AppKey（内存明文；落盘时加密为 youdao_app_key_enc）
    #[serde(default)]
    pub youdao_app_key: String,
    /// DPAPI 加密后的 youdao_app_key
    #[serde(default)]
    pub youdao_app_key_enc: Option<String>,
    /// 有道智云 AppSecret（内存明文；落盘时加密为 youdao_app_secret_enc）
    #[serde(default)]
    pub youdao_app_secret: String,
    /// DPAPI 加密后的 youdao_app_secret
    #[serde(default)]
    pub youdao_app_secret_enc: Option<String>,

    // —— AI 对话 ——
    /// AI 后端："siliconflow" / "ollama"
    #[serde(default = "d_ai_backend")]
    pub ai_backend: String,
    /// SiliconFlow API Key（与 sf_key 独立，便于分别管理；落盘时加密为 ai_sf_key_enc）
    #[serde(default)]
    pub ai_sf_key: String,
    /// DPAPI 加密后的 ai_sf_key
    #[serde(default)]
    pub ai_sf_key_enc: Option<String>,
    /// SiliconFlow 模型名
    #[serde(default = "d_ai_sf_model")]
    pub ai_sf_model: String,

    // —— 术语库 ——
    /// 是否启用术语库替换（翻译前将专业术语替换为占位符，翻译后还原）
    #[serde(default)]
    pub term_enabled: bool,
    /// 用户自定义术语（src -> dst），设置页运行时增删，落盘持久化。
    #[serde(default)]
    pub custom_terms: HashMap<String, String>,

    // —— 划词翻译（P3-3）——
    /// P3-3 划词翻译：拖选文本后自动弹气泡翻译（默认开）
    #[serde(default = "d_true")]
    pub selection_enabled: bool,

    /// 划词气泡字号档位：0=紧凑(原文13/译文15px) 1=舒适(15/17px，默认) 2=大字(17/20px)
    #[serde(default = "d_bubble_font")]
    pub bubble_font_idx: i32,

    // —— 生词本 · 简单词过滤（忽略词表）——
    /// 忽略词（简单词/功能词），截图候选词提取时会被排除。
    /// 非密钥，明文落盘。默认由 BUILTIN_IGNORE 种子；用户可在生词页增删。
    #[serde(default = "default_ignore_words")]
    pub ignore_words: Vec<String>,

    /// 采集「仅医学词」开关：开 = 只收术语库/词根命中的医学词，减少基础词噪声；
    /// 关（默认）= 正向保守全收（含通用词，标注 CEFR 基础档）。
    #[serde(default)]
    pub medical_only: bool,

    /// 学习词典来源（2026-10-17 用户需求：词典切换入口）：
    ///   "open-dictionary"（默认）= 英英释义（Wiktionary，26 万词，含例句）
    ///   "ecdict"                = 英汉释义（ECDICT sqlite，340 万词，中文释义为主）
    /// 只影响词卡「释义卡」的内容来源；例句永远优先 open-dictionary（ECDICT 无例句），
    /// 音标永远优先 ipa_en.json（两源合并兜底）。切换后新查询立即生效，无需重启。
    #[serde(default = "d_study_dict")]
    pub study_dict: String,

    /// 学习组别（2026-10-17 用户需求：按学段词表学习）：显示名，与 services::stage::options() 对应
    ///   "全部单词"（默认，不过滤）/ "小学词汇" / "初中词汇" / "高中词汇" / "四级词汇"…
    ///   自定义组 = %APPDATA%/WinOCR/vocab/<组名>.txt（每行一词，文件名即组名）。
    /// 只影响学习窗口复习队列的过滤；入库/补全不受影响。
    #[serde(default = "d_study_stage")]
    pub study_stage: String,

    // —— 外观（多主题）——
    /// UI 主题："A" 清晰高效 / "B" 高饱和醒目 / "C" 舒适阅读
    #[serde(default = "d_theme")]
    pub theme: String,

    /// 对照区排列方向（Phase 3.4）：false=左右对照（默认）/ true=上下对照
    #[serde(default)]
    pub split_vertical: bool,

    // —— 窗口与字体（2026-10-06 用户需求：窗口可缩放 + 字体可调）——
    /// 上次关闭时的窗口宽度（逻辑像素）。0=未记录，UI 层回退到默认 1130。
    #[serde(default)]
    pub window_w: i32,
    /// 上次关闭时的窗口高度（逻辑像素）。
    #[serde(default)]
    pub window_h: i32,
    /// 全局缩放百分比（75/100/125/150/175/200）。作用于界面所有字号与控件尺寸。
    /// 存百分比整数而非浮点，避免不同 DPI 下累积误差。
    #[serde(default = "d_ui_scale")]
    pub ui_scale_pct: i32,
    /// 界面字体族名（空串=用 slint 内置默认）。常见可选：
    /// "Microsoft YaHei UI"（微软雅黑）/ "SimSun"（宋体）/ "SimHei"（黑体）/ ""（默认）。
    #[serde(default)]
    pub ui_font_family: String,
}

fn d_ollama_url() -> String {
    "http://localhost:11434".into()
}
fn d_bubble_font() -> i32 {
    1
}
/// 默认全局缩放 100%。取值限定 75~200，避免用户调到界面不可用。
fn d_ui_scale() -> i32 {
    100
}
fn d_sf_url() -> String {
    "https://api.siliconflow.cn/v1".into()
}
fn d_ai_backend() -> String {
    "siliconflow".into()
}
fn d_ai_sf_model() -> String {
    "Qwen/Qwen2.5-7B-Instruct".into()
}
fn d_sf_model() -> String {
    "Qwen/Qwen3-8B".into()
}
/// P3-3 划词翻译默认开启（新装 / 旧 config.json 缺字段时都取 true）
fn d_true() -> bool {
    true
}
/// selection_bubble_enabled 默认开启
fn d_selection_bubble_enabled() -> bool {
    true
}
/// 默认主题：C 舒适阅读
fn d_theme() -> String {
    "C".into()
}

/// 学习词典默认英英（open-dictionary）
fn d_study_dict() -> String {
    "open-dictionary".into()
}

/// 学习组别默认全部（不过滤）
fn d_study_stage() -> String {
    "全部单词".into()
}

/// 内置「简单词 / 功能词」表：截图候选词提取时会被排除，避免生词本被 to/where 等污染。
/// 这是忽略词表的初始种子；用户可在生词页增删（落盘后以其为准）。
pub const BUILTIN_IGNORE: &[&str] = &[
    // 冠词 / 限定词
    "a", "an", "the",
    // 代词
    "i", "me", "my", "mine", "we", "us", "our", "ours", "you", "your", "yours",
    "he", "him", "his", "she", "her", "hers", "it", "its", "they", "them", "their",
    "theirs", "this", "that", "these", "those", "who", "whom", "whose", "what",
    "which", "one", "ones", "all", "any", "both", "each", "either", "neither",
    "few", "more", "most", "other", "some", "such",
    // 系动词 / 助动词 / 情态动词
    "be", "am", "is", "are", "was", "were", "been", "being",
    "do", "does", "did", "done", "doing",
    "have", "has", "had", "having",
    "will", "would", "shall", "should", "can", "could", "may", "might",
    "must", "ought", "need", "dare", "used",
    // 介词
    "in", "on", "at", "by", "for", "with", "about", "against", "between",
    "into", "through", "during", "before", "after", "above", "below", "to",
    "from", "up", "down", "out", "off", "over", "under", "again", "further",
    "then", "once", "of", "as", "than",
    // 连词
    "and", "but", "or", "nor", "for", "yet", "so", "if", "unless", "because",
    "although", "though", "while", "whereas", "whether", "that",
    // 疑问/关系副词
    "where", "when", "why", "how",
    // 高频动词（非生词价值低）
    "go", "get", "make", "take", "come", "see", "know", "look", "use", "find",
    "give", "tell", "work", "call", "try", "ask", "need", "feel", "become",
    "leave", "put", "keep", "let", "begin", "seem", "help", "talk", "turn",
    "start", "show", "hear", "play", "run", "move", "live", "believe", "hold",
    "bring", "happen", "write", "provide", "sit", "stand", "lose", "pay", "meet",
    "include", "continue", "set", "learn", "change", "lead", "understand",
    // 高频副词 / 形容词
    "not", "no", "yes", "very", "too", "just", "now", "here", "there", "also",
    "only", "even", "still", "much", "many", "well", "back", "good", "new",
    "first", "last", "long", "great", "little", "own", "other", "old", "right",
    "big", "high", "different", "small", "large", "next", "early", "young",
    "important", "few", "public", "bad", "same", "able",
];

/// 忽略词表默认值（内置种子）
fn default_ignore_words() -> Vec<String> {
    BUILTIN_IGNORE.iter().map(|s| s.to_string()).collect()
}

/// 主题调色板（方案 A/B/C 的颜色令牌）
pub struct ThemePalette {
    pub text: String,
    pub text_2: String,
    pub text_3: String,
    pub bg: String,
    pub surface: String,
    pub surface_2: String,
    pub border: String,
    pub accent: String,
    pub accent_2: String,
    #[allow(dead_code)] // v1.1: 主题字体/间距精细化
    pub font_source: i32,
    #[allow(dead_code)] // v1.1: 主题字体/间距精细化
    pub font_target: i32,
    #[allow(dead_code)] // v1.1: 主题字体/间距精细化
    pub line_height_source: i32,
    #[allow(dead_code)] // v1.1: 主题字体/间距精细化
    pub line_height_target: i32,
    #[allow(dead_code)] // v1.1: 主题字体/间距精细化
    pub padding: i32,
    #[allow(dead_code)] // v1.1: 主题字体/间距精细化
    pub button_height: i32,
}

impl ThemePalette {
    /// 方案 A · 清晰高效（专业高对比，医学/技术长时间阅读）
    pub fn scheme_a() -> Self {
        Self {
            text: "#0F172A".into(),
            text_2: "#475569".into(),
            text_3: "#94A3B8".into(),
            bg: "#F1F5F9".into(),
            surface: "#FFFFFF".into(),
            surface_2: "#F8FAFC".into(),
            border: "#CBD5E1".into(),
            accent: "#4F46E5".into(),
            accent_2: "#A5B4FC".into(),
            font_source: 15,
            font_target: 16,
            line_height_source: 24,
            line_height_target: 26,
            padding: 16,
            button_height: 30,
        }
    }

    /// 方案 B · 高饱和醒目（品牌力量感，视觉冲击最强）
    pub fn scheme_b() -> Self {
        Self {
            text: "#111827".into(),
            text_2: "#4B5563".into(),
            text_3: "#6B7280".into(),
            bg: "#EEF2FF".into(),
            surface: "#FFFFFF".into(),
            surface_2: "#EEF2FF".into(),
            border: "#A5B4FC".into(),
            accent: "#4338CA".into(),
            accent_2: "#6366F0".into(),
            font_source: 15,
            font_target: 17,
            line_height_source: 24,
            line_height_target: 27,
            padding: 14,
            button_height: 32,
        }
    }

    /// 方案 C · 舒适阅读（最大字号 + 大留白，轻松柔和）
    pub fn scheme_c() -> Self {
        Self {
            text: "#334155".into(),
            text_2: "#64748B".into(),
            text_3: "#94A3B8".into(),
            bg: "#F8FAFC".into(),
            surface: "#FFFFFF".into(),
            surface_2: "#F8FAFC".into(),
            border: "#E2E8F0".into(),
            accent: "#0EA5E9".into(),
            accent_2: "#7DD3FC".into(),
            font_source: 16,
            font_target: 17,
            line_height_source: 26,
            line_height_target: 28,
            padding: 20,
            button_height: 32,
        }
    }

    /// 方案 E · 护眼墨绿（最低饱和，长时间阅读；WCAG AA 已验证 2026-10-14）
    pub fn scheme_e() -> Self {
        Self {
            text: "#22302A".into(),
            text_2: "#5C6B62".into(),
            text_3: "#8A9790".into(),
            bg: "#F2F5F0".into(),
            surface: "#FFFFFF".into(),
            surface_2: "#F7FAF7".into(),
            border: "#D4DFD8".into(),
            accent: "#1F6B4E".into(),
            accent_2: "#5DCAA5".into(),
            font_source: 16,
            font_target: 17,
            line_height_source: 26,
            line_height_target: 28,
            padding: 20,
            button_height: 32,
        }
    }

    /// 方案 F · 纸质暖调（米黄底 + 赭橙强调，纸质文献感；WCAG AA 已验证 2026-10-14）
    pub fn scheme_f() -> Self {
        Self {
            text: "#3B3226".into(),
            text_2: "#6E6252".into(),
            text_3: "#9C9081".into(),
            bg: "#FAF6EF".into(),
            surface: "#FFFDF8".into(),
            surface_2: "#F5EFE3".into(),
            border: "#E8E0D0".into(),
            accent: "#A14E22".into(),
            accent_2: "#EF9F27".into(),
            font_source: 16,
            font_target: 17,
            line_height_source: 26,
            line_height_target: 28,
            padding: 20,
            button_height: 32,
        }
    }

    /// 根据主题名返回对应调色板
    pub fn by_name(name: &str) -> Self {
        match name {
            "A" => Self::scheme_a(),
            "B" => Self::scheme_b(),
            "E" => Self::scheme_e(),
            "F" => Self::scheme_f(),
            _ => Self::scheme_c(), // 默认 C
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang_from: "auto".into(),
            lang_to: "zh".into(),
            ocr_engine: "ppocr".into(),
            translate_engine: "argos".into(),
            cache_enabled: true,
            ollama_url: d_ollama_url(),
            ollama_model: String::new(),
            sf_url: d_sf_url(),
            sf_key: String::new(),
            sf_key_enc: None,
            sf_model: d_sf_model(),
            mymemory_email: String::new(),
            youdao_app_key: String::new(),
            youdao_app_key_enc: None,
            youdao_app_secret: String::new(),
            youdao_app_secret_enc: None,
            ai_backend: d_ai_backend(),
            ai_sf_key: String::new(),
            ai_sf_key_enc: None,
            ai_sf_model: d_ai_sf_model(),
            selection_bubble_enabled: true,
            browser_extension_connected: false,
            term_enabled: false,
            custom_terms: HashMap::new(),
            selection_enabled: true,
            bubble_font_idx: 1,
            theme: "C".into(),
            split_vertical: false,
            window_w: 0,
            window_h: 0,
            ui_scale_pct: d_ui_scale(),
            ui_font_family: String::new(),
            ignore_words: default_ignore_words(),
            medical_only: false,
            study_dict: d_study_dict(),
            study_stage: d_study_stage(),
        }
    }
}

impl Config {
    /// 配置落盘路径：%APPDATA%/WinOCR/config.json
    pub fn path() -> std::path::PathBuf {
        let base = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        base.join("WinOCR").join("config.json")
    }

    /// 读取配置；文件不存在或解析失败则回退默认值（不报错，保证首启可用）。
    /// 加载时自动解密 *_enc 字段到明文（旧版明文兜底，自动迁移）。
    pub fn load() -> Result<Self> {
        use serde_json::Value;
        let p = Self::path();
        if p.exists() {
            let s = std::fs::read_to_string(&p)?;
            let v: Value = serde_json::from_str(&s)?;
            let mut config: Config = serde_json::from_value(v.clone())?;
            config.decrypt_secrets(&v);
            // 旧版明文迁移：若无加密字段但有明文，保存一次即自动加密
            if config.has_plain_secrets() && !config.has_encrypted_secrets() {
                log::info!("[core::config] 检测到明文密钥，自动加密迁移");
                let _ = config.save();
            }
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// 写入配置；自动创建父目录。
    /// 落盘前加密 *_enc 字段，明文置空，绝不写入明文密钥。
    pub fn save(&self) -> Result<()> {
        let mut cfg = self.clone();
        // 加密所有密钥字段
        cfg.encrypt_secrets();
        // 明文置空（绝不写入磁盘）
        cfg.sf_key = String::new();
        cfg.ai_sf_key = String::new();
        cfg.youdao_app_key = String::new();
        cfg.youdao_app_secret = String::new();
        let s = serde_json::to_string_pretty(&cfg)?;
        let p = Self::path();
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, s)?;
        Ok(())
    }

    // —— 内部辅助 ——

    /// 从 JSON 的 *_enc 字段解密到明文
    fn decrypt_secrets(&mut self, v: &serde_json::Value) {
        if let Some(obj) = v.as_object() {
            decrypt_field(obj, "sf_key_enc", &mut self.sf_key);
            decrypt_field(obj, "ai_sf_key_enc", &mut self.ai_sf_key);
            decrypt_field(obj, "youdao_app_key_enc", &mut self.youdao_app_key);
            decrypt_field(obj, "youdao_app_secret_enc", &mut self.youdao_app_secret);
        }
    }

    /// 加密明文字段到 *_enc
    fn encrypt_secrets(&mut self) {
        encrypt_field(&self.sf_key, &mut self.sf_key_enc, "sf_key");
        encrypt_field(&self.ai_sf_key, &mut self.ai_sf_key_enc, "ai_sf_key");
        encrypt_field(&self.youdao_app_key, &mut self.youdao_app_key_enc, "youdao_app_key");
        encrypt_field(&self.youdao_app_secret, &mut self.youdao_app_secret_enc, "youdao_app_secret");
    }

    /// 是否有未加密的明文密钥（旧版迁移检测）
    fn has_plain_secrets(&self) -> bool {
        !self.sf_key.is_empty()
            || !self.ai_sf_key.is_empty()
            || !self.youdao_app_key.is_empty()
            || !self.youdao_app_secret.is_empty()
    }

    /// 是否已存在加密字段
    fn has_encrypted_secrets(&self) -> bool {
        self.sf_key_enc.is_some()
            || self.ai_sf_key_enc.is_some()
            || self.youdao_app_key_enc.is_some()
            || self.youdao_app_secret_enc.is_some()
    }
}

/// 辅助：从 JSON 对象解密单个字段
fn decrypt_field(
    obj: &serde_json::Map<String, serde_json::Value>,
    enc_key: &str,
    target: &mut String,
) {
    if let Some(enc) = obj.get(enc_key).and_then(|x| x.as_str()) {
        if !enc.is_empty() {
            match crate::core::dpapi::decrypt_secret(enc) {
                Ok(p) => *target = p,
                Err(e) => log::warn!("[core::config] {} 解密失败，跳过: {}", enc_key, e),
            }
        }
    }
}

/// 辅助：加密单个字段
fn encrypt_field(plain: &str, target: &mut Option<String>, name: &str) {
    if plain.is_empty() {
        *target = None;
    } else {
        match crate::core::dpapi::encrypt_secret(plain) {
            Ok(enc) => *target = Some(enc),
            Err(e) => {
                log::warn!(
                    "[core::config] {} 加密失败，将跳过加密写入: {}",
                    name,
                    e
                );
                *target = None;
            }
        }
    }
}
