//! Native Messaging Communication Protocol
//! Communicates with browser extension via stdin/stdout (4-byte little-endian length prefix + UTF-8 JSON)

use crate::config::OcrEngine;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

/// Maximum message size (10MB)
pub const NM_MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;

/// Native Messaging protocol version
pub const NM_PROTOCOL_VERSION: &str = "1.0.0";

/// Message type tag for serialization
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NmMessage {
    /// Text translation request
    Translate {
        text: String,
        #[serde(default)]
        source_lang: Option<String>,
        #[serde(default)]
        target_lang: Option<String>,
        #[serde(default)]
        engine: Option<String>,
    },
    /// OCR recognition from image data URL
    /// 与扩展端 background.js `ocrViaHost` 对齐：发送 `{type:'ocr', id, dataUrl}`。
    Ocr {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "dataUrl")]
        data_url: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// Trigger screenshot capture
    Capture {
        #[serde(default)]
        region: Option<NmRegion>,
    },
    /// Get current settings
    /// `alias` 兼容扩展端实际使用的 `settings.get`（background.js 走 `type: 'settings.get'`）
    #[serde(alias = "settings.get")]
    GetSettings {
        /// 扩展端请求 id，用于回执关联（background.js 的 settingsWaiters）
        #[serde(default)]
        id: Option<String>,
    },
    /// Update settings
    UpdateSettings {
        settings: serde_json::Value,
    },
    /// Crop image by region
    RegionCrop {
        #[serde(default)]
        id: Option<String>,
        #[serde(rename = "dataUrl")]
        data_url: String,
        region: NmRegion,
    },
    /// Health check / ping
    Ping {
        #[serde(default)]
        timestamp: Option<u64>,
    },
    /// 自检 / 自愈诊断（§13.4.5 / §15.3）
    SelfCheck,
    /// Get version info
    GetVersion,
    /// 首次运行引导完成（§16.3）：扩展端 5 步向导结束、用户首次成功翻译后，
    /// 由扩展经 NM 通知壳侧清除 `first_run` 标志（落盘持久化）
    FirstRunComplete,
    /// 术语库查询（M11）：返回用户自定义术语列表
    #[serde(rename = "getTerms")]
    GetTerms {
        #[serde(default)]
        id: Option<String>,
    },
    /// 术语库更新（M11）：以传入列表整体替换用户自定义术语，落盘持久化
    #[serde(rename = "setTerms")]
    SetTerms {
        terms: Vec<crate::terminology::TermEntry>,
    },
    /// 视频字幕批量翻译（M9）：接收字幕段（文本+时间轴），返回双语段
    Subtitles {
        #[serde(default)]
        id: Option<String>,
        segments: Vec<crate::subtitles::SubtitleSegment>,
    },
    /// PDF 结构化块翻译（M10）：接收文本块（页码/坐标），返回保结构译文块
    #[serde(rename = "pdfTranslate")]
    PdfTranslate {
        #[serde(default)]
        id: Option<String>,
        blocks: Vec<crate::pdf_translate::PdfBlock>,
    },
    /// Initialize connection
    Init {
        #[serde(default)]
        extension_version: Option<String>,
    },
    /// Unknown message type
    #[serde(other)]
    Unknown,
}

/// Region for screenshot/crop
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct NmRegion {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Response type tag for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NmResponse {
    /// Translation result
    Translate {
        text: String,
        #[serde(default)]
        source_lang: Option<String>,
        #[serde(default)]
        target_lang: Option<String>,
    },
    /// OCR recognition result（对齐扩展端 background.js 的 `ocr.result` 契约：
    /// 成功时带 `text`，失败时带 `error`，均带 `id` 用于回执关联）
    #[serde(rename = "ocr.result")]
    OcrResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        confidence: Option<f32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Capture acknowledgment
    CaptureAck {
        #[serde(default)]
        data_url: Option<String>,
    },
    /// Current settings（无 id 时的通用回执）
    Settings {
        settings: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// 设置回执（扩展端 `settings.current` + id 关联；background.js 读 `msg.settings`）
    /// 注意：扩展端用的是带点的 `settings.current`，不能用 snake_case 推导出的 `settings_current`
    #[serde(rename = "settings.current")]
    SettingsCurrent {
        settings: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// Error response
    Error {
        code: String,
        message: String,
    },
    /// Pong response (keep-alive)
    Pong {
        timestamp: u64,
        #[serde(default)]
        uptime_seconds: Option<u64>,
    },
    /// Version info
    Version {
        protocol_version: String,
        app_version: String,
        #[serde(default)]
        ocr_engine: Option<String>,
        #[serde(default)]
        translate_engines: Option<Vec<String>>,
        /// 是否仍为首次运行（§16.3 引导未完成）
        #[serde(default)]
        first_run: bool,
    },
    /// Self-check diagnostic report
    SelfCheck {
        report: serde_json::Value,
    },
    /// Initialization complete
    InitAck {
        success: bool,
        #[serde(default)]
        capabilities: Option<Vec<String>>,
        /// 是否仍为首次运行（§16.3 引导未完成），供扩展决定是否需要打开向导
        #[serde(default)]
        first_run: bool,
    },
    /// 首次运行引导完成回执
    FirstRunCompleteAck {
        success: bool,
        #[serde(default)]
        first_run: bool,
    },
    /// 术语库列表（M11）
    Terms {
        terms: Vec<crate::terminology::TermEntry>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    /// 视频字幕批量翻译结果（M9）：对齐扩展端 `subtitles.result` 契约（带 id 关联）
    #[serde(rename = "subtitles.result")]
    SubtitlesResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        segments: Vec<crate::subtitles::SubtitleSegment>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// PDF 块翻译结果（M10）：对齐扩展端 `pdfTranslate.result` 契约（带 id 关联）
    #[serde(rename = "pdfTranslate.result")]
    PdfTranslateResult {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        blocks: Vec<crate::pdf_translate::PdfBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// Native Messaging connection manager
pub struct NmConnection {
    start_time: Instant,
    message_count: u64,
}

impl NmConnection {
    /// Create new NM connection
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            message_count: 0,
        }
    }

    /// Read a message from stdin (blocking)
    /// Returns None if connection is closed
    pub fn read_message(&mut self) -> anyhow::Result<Option<NmMessage>> {
        // Read 4-byte length prefix (little-endian)
        let mut len_buf = [0u8; 4];
        match io::stdin().read_exact(&mut len_buf) {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(anyhow::anyhow!("读取消息长度失败: {}", e)),
        }

        let len = u32::from_le_bytes(len_buf) as usize;

        // Validate message size
        if len == 0 {
            return Err(anyhow::anyhow!("收到空消息"));
        }
        if len > NM_MAX_MESSAGE_SIZE {
            return Err(anyhow::anyhow!("消息过大: {} 字节 (最大 {} 字节)", len, NM_MAX_MESSAGE_SIZE));
        }

        // Read message body
        let mut buf = vec![0u8; len];
        io::stdin().read_exact(&mut buf)?;

        // Parse JSON
        let msg: NmMessage = match serde_json::from_slice(&buf) {
            Ok(m) => m,
            Err(e) => {
                log::error!("JSON 解析失败: {}", e);
                return Err(anyhow::anyhow!("消息解析失败: {}", e));
            }
        };

        self.message_count += 1;
        log::debug!("收到消息 #{}: {:?}", self.message_count, msg);

        Ok(Some(msg))
    }

    /// Write a response to stdout
    pub fn write_message(&mut self, resp: &NmResponse) -> anyhow::Result<()> {
        // Serialize to JSON
        let data = serde_json::to_vec(resp)?;

        // Validate size
        if data.len() > NM_MAX_MESSAGE_SIZE {
            return Err(anyhow::anyhow!(
                "响应过大: {} 字节 (最大 {} 字节)",
                data.len(),
                NM_MAX_MESSAGE_SIZE
            ));
        }

        // Write 4-byte length prefix (little-endian)
        let len = (data.len() as u32).to_le_bytes();
        io::stdout().write_all(&len)?;

        // Write message body
        io::stdout().write_all(&data)?;
        io::stdout().flush()?;

        log::debug!("发送响应: {:?}", resp);

        Ok(())
    }

    /// Send error response
    pub fn send_error(&mut self, code: &str, message: &str) -> anyhow::Result<()> {
        self.write_message(&NmResponse::Error {
            code: code.to_string(),
            message: message.to_string(),
        })
    }

    /// Send pong response
    pub fn send_pong(&mut self, timestamp: u64) -> anyhow::Result<()> {
        self.write_message(&NmResponse::Pong {
            timestamp,
            uptime_seconds: Some(self.start_time.elapsed().as_secs()),
        })
    }

    /// Get uptime in seconds
    #[allow(dead_code)]
    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get total messages processed
    #[allow(dead_code)]
    pub fn message_count(&self) -> u64 {
        self.message_count
    }
}

impl Default for NmConnection {
    fn default() -> Self {
        Self::new()
    }
}

/// Message handler trait for processing NM messages
pub trait NmHandler: Send + Sync {
    /// Handle translate request
    fn handle_translate(
        &self,
        text: &str,
        source_lang: Option<&str>,
        target_lang: Option<&str>,
    ) -> anyhow::Result<String>;

    /// Handle OCR request
    fn handle_ocr(&self, data_url: &str) -> anyhow::Result<(String, Option<f32>)>;

    /// Handle capture request
    fn handle_capture(&self, region: Option<&NmRegion>) -> anyhow::Result<Option<String>>;

    /// Handle get settings
    fn handle_get_settings(&self) -> anyhow::Result<serde_json::Value>;

    /// Handle update settings
    fn handle_update_settings(&self, settings: &serde_json::Value) -> anyhow::Result<()>;

    /// Handle region crop
    fn handle_region_crop(&self, data_url: &str, region: &NmRegion) -> anyhow::Result<String>;

    /// Handle self-check / health diagnostic
    fn handle_self_check(&self) -> anyhow::Result<serde_json::Value>;

    /// 当前是否首次运行（§16.3 引导未完成）
    fn is_first_run(&self) -> bool;

    /// 首次运行引导完成：清除 `first_run` 标志并落盘持久化
    fn handle_first_run_complete(&self) -> anyhow::Result<bool>;

    /// 术语库查询（M11）：返回用户自定义术语列表
    fn handle_get_terms(&self) -> anyhow::Result<Vec<crate::terminology::TermEntry>>;

    /// 术语库更新（M11）：整体替换用户自定义术语并落盘
    fn handle_set_terms(&self, terms: &[crate::terminology::TermEntry]) -> anyhow::Result<()>;

    /// 视频字幕批量翻译（M9）：返回双语字幕段
    fn handle_subtitles(
        &self,
        segments: &[crate::subtitles::SubtitleSegment],
    ) -> anyhow::Result<Vec<crate::subtitles::SubtitleSegment>>;

    /// PDF 结构化块翻译（M10）：返回保结构译文块
    fn handle_pdf_translate(
        &self,
        blocks: &[crate::pdf_translate::PdfBlock],
    ) -> anyhow::Result<Vec<crate::pdf_translate::PdfBlock>>;
}

/// Process messages using handler
pub fn process_message(conn: &mut NmConnection, handler: &dyn NmHandler) -> anyhow::Result<bool> {
    let msg = match conn.read_message()? {
        Some(m) => m,
        None => return Ok(false), // Connection closed
    };

    match msg {
        NmMessage::Translate {
            text,
            source_lang,
            target_lang,
            ..
        } => {
            match handler.handle_translate(&text, source_lang.as_deref(), target_lang.as_deref()) {
                Ok(result) => {
                    conn.write_message(&NmResponse::Translate {
                        text: result,
                        source_lang,
                        target_lang,
                    })?;
                }
                Err(e) => {
                    conn.send_error("TRANSLATE_ERROR", &e.to_string())?;
                }
            }
        }
        NmMessage::Ocr { id, data_url, .. } => {
            match handler.handle_ocr(&data_url) {
                Ok((text, confidence)) => {
                    conn.write_message(&NmResponse::OcrResult {
                        id,
                        text: Some(text),
                        confidence,
                        error: None,
                    })?;
                }
                Err(e) => {
                    conn.write_message(&NmResponse::OcrResult {
                        id,
                        text: None,
                        confidence: None,
                        error: Some(e.to_string()),
                    })?;
                }
            }
        }
        NmMessage::Capture { region } => {
            match handler.handle_capture(region.as_ref()) {
                Ok(data_url) => {
                    conn.write_message(&NmResponse::CaptureAck { data_url })?;
                }
                Err(e) => {
                    conn.send_error("CAPTURE_ERROR", &e.to_string())?;
                }
            }
        }
        NmMessage::GetSettings { id } => {
            match handler.handle_get_settings() {
                Ok(settings) => {
                    // 带 id 时按扩展契约回 `settings.current`，否则回通用 `settings`
                    let resp = if id.is_some() {
                        NmResponse::SettingsCurrent { settings, id }
                    } else {
                        NmResponse::Settings { settings, id }
                    };
                    conn.write_message(&resp)?;
                }
                Err(e) => {
                    conn.send_error("SETTINGS_ERROR", &e.to_string())?;
                }
            }
        }
        NmMessage::UpdateSettings { settings } => {
            match handler.handle_update_settings(&settings) {
                Ok(_) => {
                    conn.write_message(&NmResponse::InitAck {
                        success: true,
                        capabilities: None,
                        first_run: handler.is_first_run(),
                    })?;
                }
                Err(e) => {
                    conn.send_error("UPDATE_ERROR", &e.to_string())?;
                }
            }
        }
        NmMessage::RegionCrop { id, data_url, region } => {
            match handler.handle_region_crop(&data_url, &region) {
                Ok(result) => {
                    conn.write_message(&NmResponse::OcrResult {
                        id,
                        text: Some(result),
                        confidence: None,
                        error: None,
                    })?;
                }
                Err(e) => {
                    conn.write_message(&NmResponse::OcrResult {
                        id,
                        text: None,
                        confidence: None,
                        error: Some(e.to_string()),
                    })?;
                }
            }
        }
        NmMessage::Ping { timestamp } => {
            let ts = timestamp.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            conn.send_pong(ts)?;
        }
        NmMessage::GetVersion => {
            let ocr_label = crate::globals::with_globals(|g| {
                match &g.config.lock().unwrap().ocr_engine {
                    OcrEngine::Local => "PP-OCRv6",
                    OcrEngine::Sf => "SiliconFlow",
                    OcrEngine::Windows => "Windows OCR",
                }
                .to_string()
            })
            .unwrap_or_else(|| "PP-OCRv6".to_string());
            conn.write_message(&NmResponse::Version {
                protocol_version: NM_PROTOCOL_VERSION.to_string(),
                app_version: crate::APP_VERSION.to_string(),
                ocr_engine: Some(ocr_label),
                translate_engines: Some(vec![
                    "siliconflow".to_string(),
                    "mymemory".to_string(),
                    "ollama".to_string(),
                ]),
                first_run: handler.is_first_run(),
            })?;
        }
        NmMessage::FirstRunComplete => {
            match handler.handle_first_run_complete() {
                Ok(first_run) => {
                    conn.write_message(&NmResponse::FirstRunCompleteAck {
                        success: true,
                        first_run,
                    })?;
                }
                Err(e) => {
                    conn.send_error("FIRST_RUN_ERROR", &e.to_string())?;
                }
            }
        }
        NmMessage::SelfCheck => {
            match handler.handle_self_check() {
                Ok(report) => {
                    conn.write_message(&NmResponse::SelfCheck { report })?;
                }
                Err(e) => {
                    conn.send_error("SELFCHECK_ERROR", &e.to_string())?;
                }
            }
        }
        // 术语库查询（M11）
        NmMessage::GetTerms { id } => {
            match handler.handle_get_terms() {
                Ok(terms) => {
                    conn.write_message(&NmResponse::Terms { terms, id })?;
                }
                Err(e) => {
                    conn.send_error("GET_TERMS_ERROR", &e.to_string())?;
                }
            }
        }
        // 术语库更新（M11）
        NmMessage::SetTerms { terms } => {
            match handler.handle_set_terms(&terms) {
                Ok(()) => {
                    conn.write_message(&NmResponse::Terms { terms, id: None })?;
                }
                Err(e) => {
                    conn.send_error("SET_TERMS_ERROR", &e.to_string())?;
                }
            }
        }
        // 视频字幕批量翻译（M9）
        NmMessage::Subtitles { id, segments } => {
            match handler.handle_subtitles(&segments) {
                Ok(out) => {
                    conn.write_message(&NmResponse::SubtitlesResult {
                        id,
                        segments: out,
                        error: None,
                    })?;
                }
                Err(e) => {
                    conn.write_message(&NmResponse::SubtitlesResult {
                        id,
                        segments: vec![],
                        error: Some(e.to_string()),
                    })?;
                }
            }
        }
        // PDF 结构化块翻译（M10）
        NmMessage::PdfTranslate { id, blocks } => {
            match handler.handle_pdf_translate(&blocks) {
                Ok(out) => {
                    conn.write_message(&NmResponse::PdfTranslateResult {
                        id,
                        blocks: out,
                        error: None,
                    })?;
                }
                Err(e) => {
                    conn.write_message(&NmResponse::PdfTranslateResult {
                        id,
                        blocks: vec![],
                        error: Some(e.to_string()),
                    })?;
                }
            }
        }
        NmMessage::Init { extension_version } => {
            log::info!("浏览器扩展连接: {:?}", extension_version);
            conn.write_message(&NmResponse::InitAck {
                success: true,
                capabilities: Some(vec![
                    "translate".to_string(),
                    "ocr".to_string(),
                    "capture".to_string(),
                    "settings".to_string(),
                ]),
                first_run: handler.is_first_run(),
            })?;
        }
        NmMessage::Unknown => {
            conn.send_error("UNKNOWN_TYPE", "未知消息类型")?;
        }
    }

    Ok(true) // Continue processing
}

/// Default NM loop
pub fn run_nm_loop(handler: &dyn NmHandler) -> anyhow::Result<()> {
    let mut conn = NmConnection::new();

    log::info!("Native Messaging 循环启动");

    loop {
        match process_message(&mut conn, handler) {
            Ok(true) => continue,
            Ok(false) => {
                log::info!("连接关闭，退出");
                break;
            }
            Err(e) => {
                log::error!("处理消息错误: {}", e);
                // Try to send error response
                let _ = conn.send_error("INTERNAL_ERROR", &e.to_string());
            }
        }
    }

    Ok(())
}

/// Helper to convert crate::hotkey::Region to NmRegion
impl From<crate::hotkey::Region> for NmRegion {
    fn from(r: crate::hotkey::Region) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.w,
            height: r.h,
        }
    }
}

/// Helper to convert NmRegion to crate::hotkey::Region
impl From<NmRegion> for crate::hotkey::Region {
    fn from(r: NmRegion) -> Self {
        Self {
            x: r.x,
            y: r.y,
            w: r.width,
            h: r.height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M4 契约：扩展端 background.js `ocrViaHost` 真实发送
    /// `{type:'ocr', id, dataUrl}`，必须能被解析为 `Ocr` 变体且 `data_url` 取到值。
    #[test]
    fn ocr_request_from_extension_deserializes() {
        let json = r#"{"type":"ocr","id":"o1","dataUrl":"data:image/png;base64,AAAA"}"#;
        let msg: NmMessage = serde_json::from_str(json).expect("扩展 OCR 请求应可解析");
        match msg {
            NmMessage::Ocr { id, data_url, .. } => {
                assert_eq!(id.as_deref(), Some("o1"));
                assert_eq!(data_url, "data:image/png;base64,AAAA");
            }
            other => panic!("期望 Ocr 变体，实际: {:?}", other),
        }
    }

    /// M4 契约：宿主回执必须序列化为 `type:"ocr.result"` 并带 `id` 与 `text`。
    #[test]
    fn ocr_result_serializes_to_extension_contract() {
        let resp = NmResponse::OcrResult {
            id: Some("o1".to_string()),
            text: Some("你好".to_string()),
            confidence: Some(0.98),
            error: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "ocr.result");
        assert_eq!(v["id"], "o1");
        assert_eq!(v["text"], "你好");
        assert!(v.get("error").is_none() || v["error"].is_null());
    }

    /// M4 契约：宿主错误回执也走 `ocr.result`，带 `error` 字段（扩展端据此 reject）。
    #[test]
    fn ocr_error_serializes_to_extension_contract() {
        let resp = NmResponse::OcrResult {
            id: Some("o2".to_string()),
            text: None,
            confidence: None,
            error: Some("模型未找到".to_string()),
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "ocr.result");
        assert_eq!(v["id"], "o2");
        assert_eq!(v["error"], "模型未找到");
    }

    // ===== M9 视频字幕契约 =====
    #[test]
    fn subtitles_message_parses_segments_and_timeline() {
        let json = r#"{"type":"subtitles","id":"s1","segments":[{"text":"Hello","start":0.0,"end":2.0},{"text":"World","start":2.5,"end":4.0}]}"#;
        let msg: NmMessage = serde_json::from_str(json).unwrap();
        match msg {
            NmMessage::Subtitles { id, segments } => {
                assert_eq!(id.as_deref(), Some("s1"));
                assert_eq!(segments.len(), 2);
                assert_eq!(segments[0].text, "Hello");
                assert_eq!(segments[0].start, 0.0);
                assert_eq!(segments[1].end, 4.0);
            }
            other => panic!("期望 Subtitles 变体，实际: {:?}", other),
        }
    }

    #[test]
    fn subtitles_result_serializes_to_extension_contract() {
        let resp = NmResponse::SubtitlesResult {
            id: Some("s1".to_string()),
            segments: vec![crate::subtitles::SubtitleSegment {
                text: "Hello".into(),
                start: 0.0,
                end: 2.0,
                translated: Some("你好".into()),
            }],
            error: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "subtitles.result");
        assert_eq!(v["id"], "s1");
        assert_eq!(v["segments"][0]["translated"], "你好");
    }

    // ===== M10 PDF 契约 =====
    #[test]
    fn pdf_translate_message_parses_blocks() {
        let json = r#"{"type":"pdfTranslate","id":"p1","blocks":[{"text":"Intro","page":1,"x":10,"y":20},{"text":"Body","page":2}]}"#;
        let msg: NmMessage = serde_json::from_str(json).unwrap();
        match msg {
            NmMessage::PdfTranslate { id, blocks } => {
                assert_eq!(id.as_deref(), Some("p1"));
                assert_eq!(blocks.len(), 2);
                assert_eq!(blocks[0].page, 1);
                assert_eq!(blocks[0].x, 10.0);
                assert_eq!(blocks[1].text, "Body");
            }
            other => panic!("期望 PdfTranslate 变体，实际: {:?}", other),
        }
    }

    #[test]
    fn pdf_translate_result_serializes_to_extension_contract() {
        let resp = NmResponse::PdfTranslateResult {
            id: Some("p1".to_string()),
            blocks: vec![crate::pdf_translate::PdfBlock {
                text: "Intro".into(),
                page: 1,
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 20.0,
                translated: Some("引言".into()),
            }],
            error: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "pdfTranslate.result");
        assert_eq!(v["id"], "p1");
        assert_eq!(v["blocks"][0]["translated"], "引言");
    }

    // ===== M11 术语库契约 =====
    #[test]
    fn get_terms_and_set_terms_parse() {
        let g = serde_json::from_str::<NmMessage>(r#"{"type":"getTerms","id":"t1"}"#).unwrap();
        match g {
            NmMessage::GetTerms { id } => assert_eq!(id.as_deref(), Some("t1")),
            other => panic!("期望 GetTerms，实际: {:?}", other),
        }
        let s = serde_json::from_str::<NmMessage>(
            r#"{"type":"setTerms","terms":[{"src":"OCR","dst":"光学字符识别","domain":"技术","regex":false,"case_sensitive":true}]}"#,
        )
        .unwrap();
        match s {
            NmMessage::SetTerms { terms } => {
                assert_eq!(terms.len(), 1);
                assert_eq!(terms[0].src, "OCR");
                assert_eq!(terms[0].dst, "光学字符识别");
                assert!(terms[0].case_sensitive);
                assert!(!terms[0].regex);
            }
            other => panic!("期望 SetTerms，实际: {:?}", other),
        }
    }

    #[test]
    fn terms_response_serializes_with_term_entries() {
        let resp = NmResponse::Terms {
            terms: vec![crate::terminology::TermEntry {
                src: "API".into(),
                dst: "API".into(),
                domain: "技术".into(),
                regex: false,
                case_sensitive: true,
            }],
            id: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["type"], "terms");
        assert_eq!(v["terms"][0]["src"], "API");
        assert_eq!(v["terms"][0]["dst"], "API");
    }
}
