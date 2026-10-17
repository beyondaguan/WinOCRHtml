//! 翻译引擎模块
//! 支持 SiliconFlow / MyMemory / Ollama 本地模型

use crate::config::Config;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub struct TranslateEngine {
    config: Config,
    client: reqwest::blocking::Client,
}

impl TranslateEngine {
    pub fn new(config: &Config) -> Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self {
            config: config.clone(),
            client,
        })
    }

    pub fn translate(&self, text: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        match self.config.translate_engine.as_str() {
            "sf" | "siliconflow" => self.translate_sf(text),
            "mymemory" => self.translate_mymemory(text),
            "ollama" => self.translate_ollama(text),
            "youdao" => self.translate_youdao(text),
            "none" => Ok(String::new()),
            _ => self.translate_sf(text),
        }
    }

    /// 用指定引擎翻译（§17.5 换引擎重译）。engine 为空时用配置默认。
    pub fn translate_with(&self, text: &str, engine: &str) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        if engine.is_empty() {
            return self.translate(text);
        }
        let mut cfg = self.config.clone();
        cfg.translate_engine = engine.to_string();
        let tmp = TranslateEngine {
            config: cfg,
            client: self.client.clone(),
        };
        tmp.translate(text)
    }

    /// 用指定引擎 + 指定源语言翻译（§18 客户端语种检测）。
    /// `src_lang` 为 `Some` 时覆盖配置中的源语言（用于 `auto` 被本地检测后回填），
    /// 为 `None` 时沿用配置（保持原有 auto 交给服务端检测）。
    pub fn translate_with_src(
        &self,
        text: &str,
        engine: &str,
        src_lang: Option<&str>,
    ) -> Result<String> {
        if text.trim().is_empty() {
            return Ok(String::new());
        }
        let mut cfg = self.config.clone();
        if !engine.is_empty() {
            cfg.translate_engine = engine.to_string();
        }
        if let Some(l) = src_lang {
            if !l.is_empty() && l != "auto" {
                cfg.src_lang = l.to_string();
            }
        }
        let tmp = TranslateEngine {
            config: cfg,
            client: self.client.clone(),
        };
        // 术语库（M11）：翻译前用占位符保护术语 src，翻译后还原为术语 dst，
        // 确保术语译文优先于引擎默认表达（用户术语 > 内置术语 > 引擎默认）。
        let terms = crate::terminology::Terminology::with_user_terms(self.config.user_terms.clone());
        let (protected, gmap) = terms.apply(text);
        let translated = tmp.translate(&protected)?;
        Ok(crate::terminology::Terminology::restore(&translated, &gmap))
    }

    fn translate_sf(&self, text: &str) -> Result<String> {
        #[derive(Serialize)]
        struct SfRequest {
            model: String,
            messages: Vec<SfMessage>,
            temperature: f32,
            max_tokens: u32,
        }
        #[derive(Serialize, Deserialize)]
        struct SfMessage {
            role: String,
            content: String,
        }
        #[derive(Deserialize)]
        struct SfResponse {
            choices: Vec<SfChoice>,
        }
        #[derive(Deserialize)]
        struct SfChoice {
            message: SfMessageContent,
        }
        #[derive(Deserialize)]
        struct SfMessageContent {
            content: String,
        }

        let sys = format!(
            "You are a professional translator. Translate the user text into {}. Keep medical/technical terms accurate. Output ONLY the translation, no commentary, no quotes.",
            self.config.tgt_lang
        );
        let body = SfRequest {
            model: self.config.sf_model.clone(),
            messages: vec![
                SfMessage { role: "system".into(), content: sys },
                SfMessage { role: "user".into(), content: text.to_string() },
            ],
            temperature: 0.3,
            max_tokens: 4096,
        };
        let url = format!("{}/chat/completions", self.config.sf_url.trim_end_matches('/'));
        let resp: SfResponse = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.config.sf_key))
            .json(&body)
            .send()?
            .json()?;
        let out = resp.choices.first()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("SiliconFlow 返回空内容"))?;
        Ok(out)
    }

    fn translate_mymemory(&self, text: &str) -> Result<String> {
        // 简单 URL 编码
        let encoded_text = url_encode(text);
        let encoded_email = url_encode(&self.config.mymemory_email);
        let url = format!(
            "https://api.mymemory.translated.net/get?q={}&langpair={}|{}&de={}",
            encoded_text,
            self.config.src_lang,
            self.config.tgt_lang,
            encoded_email
        );
        #[derive(Deserialize)]
        struct MmResponse {
            #[serde(rename = "responseData")]
            response_data: MmData,
        }
        #[derive(Deserialize)]
        struct MmData {
            #[serde(rename = "translatedText")]
            translated_text: String,
        }
        let resp: MmResponse = self.client.get(&url).send()?.json()?;
        let out = resp.response_data.translated_text.trim().to_string();
        if out.is_empty() {
            return Err(anyhow::anyhow!("MyMemory 返回空内容"));
        }
        Ok(out)
    }

    fn translate_ollama(&self, text: &str) -> Result<String> {
        if self.config.ollama_model.is_empty() {
            return Err(anyhow::anyhow!("未选择 Ollama 模型"));
        }
        #[derive(Serialize)]
        struct OllamaRequest {
            model: String,
            prompt: String,
            stream: bool,
        }
        #[derive(Deserialize)]
        struct OllamaResponse {
            response: String,
        }
        let body = OllamaRequest {
            model: self.config.ollama_model.clone(),
            prompt: format!(
                "Translate the following text into {}. Keep medical/technical terms accurate. Output ONLY the translation:\n\n{}",
                self.config.tgt_lang, text
            ),
            stream: false,
        };
        let url = format!("{}/api/generate", self.config.ollama_url.trim_end_matches('/'));
        let resp: OllamaResponse = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()?
            .json()?;
        let out = resp.response.trim().to_string();
        if out.is_empty() {
            return Err(anyhow::anyhow!("Ollama 返回空内容"));
        }
        Ok(out)
    }

    /// 有道智云翻译（§18 / M7 第 4 引擎，signType=v3 MD5 签名）。
    /// 无 key 时返回明确错误（不再静默回退 SF）。
    fn translate_youdao(&self, text: &str) -> Result<String> {
        let app_key = self.config.youdao_app_key.trim();
        let app_secret = self.config.youdao_app_secret.trim();
        if app_key.is_empty() || app_secret.is_empty() {
            return Err(anyhow::anyhow!(
                "未配置有道翻译密钥（请在设置页填写 youdao_app_key / youdao_app_secret）"
            ));
        }
        let from = youdao_lang(&self.config.src_lang);
        let to = youdao_lang(&self.config.tgt_lang);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let salt = format!("{}", now.as_millis());
        let curtime = format!("{}", now.as_secs());
        // 签名输入：q 长度 > 20 时取「前10 + 长度 + 后10」
        let input = if text.chars().count() > 20 {
            let chars: Vec<char> = text.chars().collect();
            let len = chars.len();
            let first10: String = chars[..10].iter().collect();
            let last10: String = chars[len - 10..].iter().collect();
            format!("{}{}{}", first10, len, last10)
        } else {
            text.to_string()
        };
        let sign = md5_hex(
            format!("{}{}{}{}{}", app_key, input, salt, curtime, app_secret).as_bytes(),
        );

        #[derive(Deserialize)]
        struct YdResponse {
            #[serde(rename = "errorCode")]
            error_code: String,
            #[serde(rename = "translation")]
            translation: Option<Vec<String>>,
            #[serde(rename = "errorMsg")]
            error_msg: Option<String>,
        }

        let params: [(&str, &str); 8] = [
            ("q", text),
            ("from", &from),
            ("to", &to),
            ("appKey", app_key),
            ("salt", &salt),
            ("curtime", &curtime),
            ("sign", &sign),
            ("signType", "v3"),
        ];
        let resp: YdResponse = self
            .client
            .post("https://openapi.youdao.com/api")
            .form(&params)
            .send()?
            .json()?;
        if resp.error_code != "0" {
            return Err(anyhow::anyhow!(
                "有道翻译错误 {}: {}",
                resp.error_code,
                resp.error_msg.unwrap_or_default()
            ));
        }
        let out = resp
            .translation
            .and_then(|v| v.into_iter().next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("有道翻译返回空内容"))?;
        Ok(out)
    }
}

/// 项目简码 → 有道语言码
fn youdao_lang(code: &str) -> String {
    match code {
        "zh" | "zh-CN" | "zh-CHS" => "zh-CHS".to_string(),
        "en" => "en".to_string(),
        "ja" | "jp" => "ja".to_string(),
        "ko" => "ko".to_string(),
        "fr" => "fr".to_string(),
        "ru" => "ru".to_string(),
        "de" => "de".to_string(),
        "es" => "es".to_string(),
        "auto" => "auto".to_string(),
        other => other.to_string(),
    }
}

/// MD5 十六进制小写
fn md5_hex(input: &[u8]) -> String {
    let digest = md5::compute(input);
    let mut s = String::with_capacity(32);
    for b in digest.0 {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

impl Default for TranslateEngine {
    fn default() -> Self {
        Self::new(&Config::default()).unwrap_or_else(|_| Self {
            config: Config::default(),
            client: reqwest::blocking::Client::new(),
        })
    }
}

/// 简单 URL 编码（百分编码）
fn url_encode(input: &str) -> String {
    let mut result = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn youdao_lang_mapping() {
        assert_eq!(youdao_lang("zh"), "zh-CHS");
        assert_eq!(youdao_lang("zh-CN"), "zh-CHS");
        assert_eq!(youdao_lang("en"), "en");
        assert_eq!(youdao_lang("ja"), "ja");
        assert_eq!(youdao_lang("auto"), "auto");
        assert_eq!(youdao_lang("fr"), "fr");
        assert_eq!(youdao_lang("unknown_x"), "unknown_x");
    }

    #[test]
    fn md5_hex_known_vectors() {
        // 标准 MD5 测试向量
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn youdao_missing_key_errors_clearly() {
        let mut cfg = Config::default();
        cfg.translate_engine = "youdao".to_string();
        let eng = TranslateEngine::new(&cfg).unwrap();
        // 不应静默回退 SF，而应明确报错未配置密钥
        let r = eng.translate("hello world");
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("未配置有道"));
    }
}
