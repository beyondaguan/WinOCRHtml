//! 配置管理：加载、保存、默认值、版本迁移、密钥加密落盘
//!
//! 设计要点（对应计划文档 §13.5 / §15.9）：
//! - `config_version`：配置 schema 版本，启动时做迁移，保证旧文件可升级。
//! - `sf_key`：内存明文，通过 NM 暴露给本扩展设置页（同一用户会话内，可接受）；
//!   但**落盘时明文被清空、改为 `sf_key_enc` 经 DPAPI 加密**，避免密钥明文出现在磁盘（§15.9 红线）。
//! - `save`：先写临时文件再 rename（原子替换），覆盖前保留 `.json.bak` 备份。
//! - 两条序列化路径：
//!   - `to_json()`：设置页用，含 `sf_key` 明文、不含 `sf_key_enc` blob；
//!   - `save()`：磁盘用，含 `sf_key_enc`、不含 `sf_key` 明文。

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// 当前配置 schema 版本（变更配置结构时自增，并在 `migrate` 中处理旧版本）
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    /// API Key（内存明文；设置页经 NM 暴露；落盘前清空并改用 `sf_key_enc`）
    pub sf_key: String,
    /// DPAPI 加密后的 sf_key（base64）。落盘写入；设置页 JSON 中不外发。
    #[serde(default)]
    pub sf_key_enc: Option<String>,
    pub sf_url: String,
    pub sf_model: String,
    pub ocr_engine: OcrEngine,
    pub local_ocr_tier: OcrTier,
    pub src_lang: String,
    pub tgt_lang: String,
    pub translate_engine: String,
    pub hotkey: String,
    pub quit_hotkey: String,
    pub settings_hotkey: String,
    /// 暂停/恢复热键（三态模型，§15.4 / §16.6）
    #[serde(default = "default_pause_hotkey")]
    pub pause_hotkey: String,
    /// 静默截图 OCR 热键：识别后仅入历史、不弹浮窗（界面地图 S12，默认 alt+q）
    #[serde(default = "default_silent_hotkey")]
    pub silent_hotkey: String,
    /// 唤起主窗口热键（界面地图 S12，默认 alt+w）
    #[serde(default = "default_main_hotkey")]
    pub main_hotkey: String,
    /// 输入框转译 / 粘贴翻译热键（界面地图 S12，默认 alt+t）
    #[serde(default = "default_input_hotkey")]
    pub input_hotkey: String,
    /// 朗读热键（界面地图 S12，默认 alt+r）
    #[serde(default = "default_read_hotkey")]
    pub read_hotkey: String,
    /// 复制译文热键（界面地图 S12，默认 alt+c）
    #[serde(default = "default_copy_hotkey")]
    pub copy_hotkey: String,
    /// 替换原文热键（桌面端降级为复制，界面地图 S12，默认 alt+v）
    #[serde(default = "default_replace_hotkey")]
    pub replace_hotkey: String,
    /// 静默模式（主窗口「截图翻译」是否默认静默入历史，对应界面地图 S1 底部 ☑静默OCR）
    #[serde(default)]
    pub silent_mode: bool,
    pub mymemory_email: String,
    pub ollama_url: String,
    pub ollama_model: String,
    /// 有道智云翻译 App Key（§18 / M7 第 4 引擎）。空表示未配置。
    #[serde(default)]
    pub youdao_app_key: String,
    /// 有道智云翻译 App Secret（同上）
    #[serde(default)]
    pub youdao_app_secret: String,
    pub native_host: bool,
    pub model_dir: String,
    pub proxy_url: String,
    pub models: ModelsConfig,
    /// 配置 schema 版本
    #[serde(default = "default_config_version")]
    pub config_version: u32,
    /// 是否首次运行（用于 5 步引导，§16.3）
    #[serde(default)]
    pub first_run: bool,
    /// 遥测开关（默认 off，§15.6）。仅聚合匿名错误计数，绝不携带文本/密钥；默认关闭。
    #[serde(default)]
    pub telemetry: bool,
    /// 浮窗主题（light / dark / auto），对应 §17.3 外观分区
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 浮窗字号档位（small / standard / large），对应 §17.3 外观分区
    #[serde(default = "default_font_scale")]
    pub font_scale: String,
    /// 浮窗不透明度 0-255（对应 §17.3 外观分区）
    #[serde(default = "default_float_opacity")]
    pub float_opacity: u8,
    /// 是否记忆浮窗上次位置（对应 §17.5 位置记忆）
    #[serde(default = "default_allow_position_memory")]
    pub allow_position_memory: bool,
    /// 界面语言（zh / en），对应 §17.8 i18n
    #[serde(default = "default_ui_lang")]
    pub ui_lang: String,
    /// 选区放大镜（§17.4），默认开
    #[serde(default = "default_true")]
    pub magnifier: bool,
    /// 选区边缘吸附（§17.4），默认开
    #[serde(default = "default_true")]
    pub snap: bool,
    /// 是否启用 DXGI 桌面复制截图（§17.4）。开启时优先用 DXGI 抓屏（质量高、无闪烁），
    /// 本机显卡/驱动不支持时（报 0x80070057 等）可关闭，改用 GDI 兼容模式。
    #[serde(default = "default_true")]
    pub use_dxgi: bool,
    /// OCR 结果缓存开关（§14.5 / P2-5），默认开
    #[serde(default = "default_true")]
    pub ocr_cache_enabled: bool,
    /// OCR 缓存最大条目数（§14.5 / P2-5），默认 500
    #[serde(default = "default_ocr_cache_max")]
    pub ocr_cache_max: usize,
    /// 月度字符配额上限（§15.8 / P2-6），0 表示不限
    #[serde(default)]
    pub monthly_char_limit: u64,
    /// 用户自定义术语库（M11）：关键词 → 翻译映射，持久化后优先级高于内置术语
    #[serde(default)]
    pub user_terms: Vec<crate::terminology::TermEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ModelsConfig {
    pub base_url: String,
    pub proxy_enabled: bool,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            base_url: "https://hf-mirror.com".to_string(),
            proxy_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OcrEngine {
    Local,
    Sf,
    Windows,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OcrTier {
    Tiny,
    Small,
    Medium,
}

impl OcrTier {
    /// 根据模型类型返回 ONNX 文件名
    pub fn model_name(&self, kind: &str) -> String {
        match self {
            OcrTier::Tiny => format!("{}_tiny.onnx", kind),
            OcrTier::Small => format!("{}_small.onnx", kind),
            OcrTier::Medium => format!("{}_medium.onnx", kind),
        }
    }
}

fn default_config_version() -> u32 {
    CONFIG_VERSION
}

fn default_pause_hotkey() -> String {
    "ctrl+alt+p".to_string()
}

fn default_silent_hotkey() -> String {
    "alt+q".to_string()
}

fn default_main_hotkey() -> String {
    "alt+w".to_string()
}

fn default_input_hotkey() -> String {
    "alt+t".to_string()
}

fn default_read_hotkey() -> String {
    "alt+r".to_string()
}

fn default_copy_hotkey() -> String {
    "alt+c".to_string()
}

fn default_replace_hotkey() -> String {
    "alt+v".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sf_key: String::new(),
            sf_key_enc: None,
            sf_url: "https://api.siliconflow.cn/v1".to_string(),
            sf_model: "Qwen/Qwen3-8B".to_string(),
            ocr_engine: OcrEngine::Local,
            local_ocr_tier: OcrTier::Tiny,
            src_lang: "en".to_string(),
            tgt_lang: "zh".to_string(),
            // 默认走 MyMemory（免费 · 免 key · 国内可直连），保证开机即译、开箱即用；
            // 用户可在设置里切到 SiliconFlow / Ollama 以获得更高质量（需自备 key 或本地服务）。
            translate_engine: "mymemory".to_string(),
            hotkey: "alt+s".to_string(),
            silent_hotkey: default_silent_hotkey(),
            main_hotkey: default_main_hotkey(),
            input_hotkey: default_input_hotkey(),
            read_hotkey: default_read_hotkey(),
            copy_hotkey: default_copy_hotkey(),
            replace_hotkey: default_replace_hotkey(),
            silent_mode: false,
            quit_hotkey: "ctrl+alt+q".to_string(),
            settings_hotkey: "ctrl+alt+s".to_string(),
            pause_hotkey: default_pause_hotkey(),
            mymemory_email: String::new(),
            ollama_url: "http://localhost:11434".to_string(),
            ollama_model: String::new(),
            youdao_app_key: String::new(),
            youdao_app_secret: String::new(),
            native_host: false,
            model_dir: "models/ocr".to_string(),
            proxy_url: "http://127.0.0.1:7890".to_string(),
            models: ModelsConfig::default(),
            config_version: CONFIG_VERSION,
            first_run: true,
            telemetry: false,
            theme: default_theme(),
            font_scale: default_font_scale(),
            float_opacity: default_float_opacity(),
            allow_position_memory: default_allow_position_memory(),
            ui_lang: default_ui_lang(),
            magnifier: default_true(),
            snap: default_true(),
            use_dxgi: default_true(),
            ocr_cache_enabled: default_true(),
            ocr_cache_max: default_ocr_cache_max(),
            monthly_char_limit: 0,
            user_terms: Vec::new(),
        }
    }
}

fn default_theme() -> String {
    "light".to_string()
}

fn default_font_scale() -> String {
    "standard".to_string()
}

fn default_float_opacity() -> u8 {
    235
}

fn default_allow_position_memory() -> bool {
    true
}

fn default_ui_lang() -> String {
    "zh".to_string()
}

fn default_true() -> bool {
    true
}

fn default_ocr_cache_max() -> usize {
    500
}

/// 默认配置文件路径：exe 同目录下的 winocr_config.json
pub fn default_config_path() -> anyhow::Result<String> {
    let exe = std::env::current_exe().context("获取 exe 路径失败")?;
    let dir = exe.parent().context("获取 exe目录失败")?;
    Ok(dir
        .join("winocr_config.json")
        .to_string_lossy()
        .into_owned())
}

impl Config {
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        let owned;
        let path: &str = match path {
            Some(p) => p,
            // 默认配置文件锚定 exe 所在目录（NM 模式下 cwd 不可控）
            None => {
                owned = default_config_path()?;
                &owned
            }
        };
        if Path::new(path).exists() {
            let content = std::fs::read_to_string(path)?;
            let v: Value = serde_json::from_str(&content)?;
            let mut config: Config = serde_json::from_value(v.clone())?;

            // 解析 sf_key：加密字段优先，旧版明文兜底（首次加载后下次保存即自动加密）
            let plain = if let Some(enc) = v.get("sf_key_enc").and_then(|x| x.as_str()) {
                if enc.is_empty() {
                    String::new()
                } else {
                    match crate::dpapi::decrypt_secret(enc) {
                        Ok(p) => p,
                        Err(e) => {
                            log::warn!("sf_key 解密失败，使用空密钥: {}", e);
                            String::new()
                        }
                    }
                }
            } else if let Some(p) = v.get("sf_key").and_then(|x| x.as_str()) {
                p.to_string()
            } else {
                String::new()
            };
            config.sf_key = plain;

            config.migrate();

            // 迁移：若仍是旧版明文 sf_key 且无加密字段，立即落盘加密（§15.9）
            if config.sf_key_enc.is_none() && !config.sf_key.is_empty() {
                if let Err(e) = config.save(path) {
                    log::warn!("明文 sf_key 加密迁移失败（将保留明文）: {}", e);
                }
            }

            Ok(config)
        } else {
            let config = Config::default();
            config.save(path)?;
            Ok(config)
        }
    }

    /// 配置结构迁移：旧版本字段在此补齐/转换，并提升到当前版本
    fn migrate(&mut self) {
        if self.config_version < CONFIG_VERSION {
            log::info!(
                "配置从 v{} 迁移到 v{}",
                self.config_version,
                CONFIG_VERSION
            );
            self.config_version = CONFIG_VERSION;
        }
    }

    /// 设置页用 JSON：暴露 `sf_key` 明文供 UI 回填，剔除加密 blob `sf_key_enc`。
    pub fn to_json(&self) -> String {
        let mut v = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Some(obj) = v.as_object_mut() {
            obj.remove("sf_key_enc");
        }
        serde_json::to_string_pretty(&v).unwrap_or_default()
    }

    /// 原子保存：先写 `.json.tmp` 再 rename；覆盖前保留 `.json.bak`。
    /// `sf_key` 在落盘前经 DPAPI 加密为 `sf_key_enc`，明文清空，绝不写入文件。
    pub fn save(&self, path: &str) -> anyhow::Result<()> {
        let enc = if self.sf_key.is_empty() {
            None
        } else {
            match crate::dpapi::encrypt_secret(&self.sf_key) {
                Ok(s) => Some(s),
                Err(e) => {
                    log::warn!("sf_key 加密失败，回退为明文写入: {}", e);
                    None
                }
            }
        };

        let mut v = serde_json::to_value(self)?;
        if let Some(obj) = v.as_object_mut() {
            // 磁盘安全：明文替换为空，写入加密 blob
            obj.insert("sf_key".to_string(), Value::String(String::new()));
            obj.insert("sf_key_enc".to_string(), serde_json::to_value(enc)?);
        }
        let content = serde_json::to_string_pretty(&v)?;

        let path_p = Path::new(path);
        if path_p.exists() {
            if let Some(parent) = path_p.parent() {
                let bak_name = path_p
                    .file_name()
                    .map(|n| format!("{}.bak", n.to_string_lossy()))
                    .unwrap_or_else(|| "winocr_config.json.bak".to_string());
                let _ = std::fs::copy(path_p, parent.join(bak_name));
            }
        }
        let tmp = path_p
            .parent()
            .map(|p| {
                p.join(format!(
                    "{}.tmp",
                    path_p.file_name().unwrap().to_string_lossy()
                ))
            })
            .unwrap_or_else(|| path_p.with_extension("tmp"));
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, path_p)?;
        Ok(())
    }

    /// 遥测是否启用（默认 off，§15.6）。仅聚合匿名错误计数，绝不携带文本/密钥。
    pub fn is_telemetry_enabled(&self) -> bool {
        self.telemetry
    }
}
