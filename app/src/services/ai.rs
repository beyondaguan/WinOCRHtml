use anyhow::{Context, Result};
use futures::StreamExt;
use log::{info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;

/// AI 后端枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AiBackend {
    SiliconFlow,
    Ollama,
}

impl AiBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            AiBackend::SiliconFlow => "siliconflow",
            AiBackend::Ollama => "ollama",
        }
    }

    #[allow(dead_code)] // v1.1: 配置 → AiBackend 解析
    pub fn from_str(s: &str) -> Self {
        match s {
            "siliconflow" | "sf" => AiBackend::SiliconFlow,
            "ollama" => AiBackend::Ollama,
            _ => AiBackend::SiliconFlow,
        }
    }
}

/// 聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// SiliconFlow / OpenAI 兼容的请求体
#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
}

/// SSE 流式响应的 choices[0].delta.content
#[derive(Debug, Deserialize)]
struct StreamResponse {
    choices: Option<Vec<StreamChoice>>,
}

#[derive(Debug, Deserialize)]
struct StreamChoice {
    delta: Option<StreamDelta>,
}

#[derive(Debug, Deserialize)]
struct StreamDelta {
    content: Option<String>,
}

/// SiliconFlow / Ollama 双后端 AI 服务
pub struct Ai {
    client: Client,
    backend: Mutex<AiBackend>,
    silicon_url: Mutex<String>,
    silicon_key: Mutex<String>,
    silicon_model: Mutex<String>,
    ollama_url: Mutex<String>,
    ollama_model: Mutex<String>,
    context: Mutex<Vec<ChatMessage>>,
}

impl Ai {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            backend: Mutex::new(AiBackend::SiliconFlow),
            silicon_url: Mutex::new("https://api.siliconflow.cn/v1/chat/completions".into()),
            silicon_key: Mutex::new(String::new()),
            silicon_model: Mutex::new("Qwen/Qwen2.5-7B-Instruct".into()),
            ollama_url: Mutex::new("http://localhost:11434".into()),
            ollama_model: Mutex::new("qwen2.5:7b".into()),
            context: Mutex::new(Vec::new()),
        }
    }

    /// 应用配置（&self，便于 Arc 热切换）
    pub fn configure(&self, backend: AiBackend, sf_key: &str, sf_model: &str, ollama_url: &str, ollama_model: &str) {
        *self.backend.lock().unwrap() = backend;
        *self.silicon_key.lock().unwrap() = sf_key.into();
        if !sf_model.is_empty() {
            *self.silicon_model.lock().unwrap() = sf_model.into();
        }
        if !ollama_url.is_empty() {
            let base = ollama_url.trim_end_matches('/');
            *self.ollama_url.lock().unwrap() = format!("{}/api/chat", base);
        }
        if !ollama_model.is_empty() {
            *self.ollama_model.lock().unwrap() = ollama_model.into();
        }
        info!("[services::ai] 配置已更新: backend={}", backend.as_str());
    }

    /// 流式聊天：每收到一段 token 就回调
    #[allow(dead_code)] // 预留：无上下文版本（当前 UI 走 chat_stream_with_context）
    pub async fn chat_stream<F>(&self, user_msg: String, on_chunk: F) -> Result<()>
    where
        F: FnMut(String) + Send + 'static,
    {
        self.chat_stream_with_context(None, None, user_msg, on_chunk)
            .await
    }

    /// 流式聊天（带 OCR 上下文）：
    /// 将当前截图原文/译文作为 system prompt 注入，让 AI 结合语境回答。
    /// ocr_src / ocr_tgt 为 None 时退化为普通对话。
    pub async fn chat_stream_with_context<F>(
        &self,
        ocr_src: Option<String>,
        ocr_tgt: Option<String>,
        user_msg: String,
        on_chunk: F,
    ) -> Result<()>
    where
        F: FnMut(String) + Send + 'static,
    {
        let mut ctx = self.context.lock().unwrap();

        // 构造消息序列
        let mut messages: Vec<ChatMessage> = if ocr_src.is_some() || ocr_tgt.is_some() {
            // 带 OCR 上下文：重置会话，注入 system 上下文
            ctx.clear();
            let mut msgs = Vec::new();
            let mut sys = String::from("你是 WinOCR 的 AI 助手，帮助用户理解截图内容。\n");
            if let Some(src) = &ocr_src {
                if !src.trim().is_empty() {
                    sys.push_str(&format!("\n【当前截图原文】\n{src}\n"));
                }
            }
            if let Some(tgt) = &ocr_tgt {
                if !tgt.trim().is_empty() {
                    sys.push_str(&format!("【当前截图译文】\n{tgt}\n"));
                }
            }
            sys.push_str("\n请结合以上截图上下文回答用户问题。");
            msgs.push(ChatMessage {
                role: "system".into(),
                content: sys,
            });
            msgs
        } else {
            ctx.clone()
        };

        // 追加用户消息
        messages.push(ChatMessage {
            role: "user".into(),
            content: user_msg,
        });
        // 同步到会话上下文
        *ctx = messages.clone();
        drop(ctx);

        match *self.backend.lock().unwrap() {
            AiBackend::SiliconFlow => {
                self.chat_siliconflow(messages, on_chunk).await?;
            }
            AiBackend::Ollama => {
                self.chat_ollama(messages, on_chunk).await?;
            }
        }

        Ok(())
    }

    /// SiliconFlow SSE 流式（OpenAI 兼容）
    async fn chat_siliconflow<F>(&self, messages: Vec<ChatMessage>, mut on_chunk: F) -> Result<()>
    where
        F: FnMut(String) + Send + 'static,
    {
        let silicon_key = self.silicon_key.lock().unwrap().clone();
        let silicon_url = self.silicon_url.lock().unwrap().clone();
        let silicon_model = self.silicon_model.lock().unwrap().clone();
        if silicon_key.is_empty() {
            anyhow::bail!("SiliconFlow API Key 未配置，请在 Settings 中填写");
        }

        let req = ChatRequest {
            model: silicon_model,
            messages,
            stream: true,
            max_tokens: Some(2048),
            temperature: Some(0.7),
        };

        let resp = self
            .client
            .post(&silicon_url)
            .header("Authorization", format!("Bearer {}", silicon_key))
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .context("SiliconFlow 请求失败，检查网络或 API Key")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("SiliconFlow 返回 {}: {}", status, body);
        }

        let mut stream = resp.bytes_stream();
        let mut full_reply = String::new();
        let mut buf = Vec::new();

        // 每个 SSE chunk 等待上限：避免服务端半开连接/静默断流导致 UI 永久停在「回复一半」。
        // 每次收到新 chunk 都会重置计时，长回答流式输出不受影响。
        const CHUNK_TIMEOUT: Duration = Duration::from_secs(90);

        loop {
            let next = tokio::time::timeout(CHUNK_TIMEOUT, stream.next()).await;
            let chunk = match next {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => {
                    // 网络中途断开：保留已累积内容，让 UI 正常收尾（不要丢弃半截回复）
                    warn!("[services::ai] SiliconFlow 流读取错误：{e}");
                    break;
                }
                Ok(None) => break, // 服务端正常结束
                Err(_elapsed) => {
                    // 90 秒无新 chunk：判定为静默断流，保留已累积内容收尾
                    warn!("[services::ai] SiliconFlow 流超时（90s 无数据），提前结束");
                    break;
                }
            };
            buf.extend_from_slice(&chunk);

            // 按 SSE 行分割
            while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                let event_bytes: Vec<u8> = buf.drain(..pos + 2).collect();
                let event = String::from_utf8_lossy(&event_bytes);

                for line in event.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            // 标记正常完成，跳出外层循环
                            buf.clear();
                            let mut ctx = self.context.lock().unwrap();
                            ctx.push(ChatMessage {
                                role: "assistant".into(),
                                content: full_reply.clone(),
                            });
                            info!("[services::ai] siliconflow 完成 ({} 字符)", full_reply.chars().count());
                            return Ok(());
                        }
                        if let Ok(sr) = serde_json::from_str::<StreamResponse>(data) {
                            if let Some(choices) = &sr.choices {
                                if let Some(choice) = choices.first() {
                                    if let Some(delta) = &choice.delta {
                                        if let Some(content) = &delta.content {
                                            full_reply.push_str(content);
                                            on_chunk(content.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 循环结束（正常 EOF / 网络错误 / 超时）：若 buf 还有残留 data 行，尝试解析
        if !buf.is_empty() {
            let tail = String::from_utf8_lossy(&buf);
            for line in tail.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }
                    if let Ok(sr) = serde_json::from_str::<StreamResponse>(data) {
                        if let Some(choices) = &sr.choices {
                            if let Some(choice) = choices.first() {
                                if let Some(delta) = &choice.delta {
                                    if let Some(content) = &delta.content {
                                        full_reply.push_str(content);
                                        on_chunk(content.clone());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // 追加 assistant 回复到上下文
        let mut ctx = self.context.lock().unwrap();
        ctx.push(ChatMessage {
            role: "assistant".into(),
            content: full_reply.clone(),
        });
        info!("[services::ai] siliconflow 完成 ({} 字符)", full_reply.chars().count());
        Ok(())
    }

    /// Ollama 本地流式 (/api/chat)
    async fn chat_ollama<F>(&self, messages: Vec<ChatMessage>, mut on_chunk: F) -> Result<()>
    where
        F: FnMut(String) + Send + 'static,
    {
        #[derive(Serialize)]
        struct OllamaReq<'a> {
            model: &'a str,
            messages: &'a [ChatMessage],
            stream: bool,
        }

        #[derive(Deserialize)]
        struct OllamaResp {
            message: Option<OllamaMsg>,
            done: Option<bool>,
        }
        #[derive(Deserialize)]
        struct OllamaMsg {
            content: String,
        }

        let ollama_model = self.ollama_model.lock().unwrap().clone();
        let ollama_url = self.ollama_url.lock().unwrap().clone();

        let req = OllamaReq {
            model: &ollama_model,
            messages: &messages,
            stream: true,
        };

        let resp = self
            .client
            .post(&ollama_url)
            .json(&req)
            .send()
            .await
            .context("Ollama 请求失败，检查是否运行 (ollama run qwen2.5:7b)")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Ollama 返回 {}: {}", status, body);
        }

        let mut stream = resp.bytes_stream();
        let mut full_reply = String::new();
        let mut line_buf = Vec::new();

        const OLLAMA_CHUNK_TIMEOUT: Duration = Duration::from_secs(90);

        loop {
            let next = tokio::time::timeout(OLLAMA_CHUNK_TIMEOUT, stream.next()).await;
            let bytes = match next {
                Ok(Some(Ok(bytes))) => bytes,
                Ok(Some(Err(e))) => {
                    warn!("[services::ai] Ollama 流读取错误：{e}");
                    break;
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    warn!("[services::ai] Ollama 流超时（90s 无数据），提前结束");
                    break;
                }
            };
            line_buf.extend_from_slice(&bytes);

            while let Some(pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = line_buf.drain(..pos + 1).collect();
                let line_str = String::from_utf8_lossy(&line);

                if let Ok(resp) = serde_json::from_str::<OllamaResp>(line_str.trim()) {
                    if let Some(msg) = &resp.message {
                        full_reply.push_str(&msg.content);
                        on_chunk(msg.content.clone());
                    }
                    if resp.done == Some(true) {
                        // 标记正常完成，跳出外层循环
                        line_buf.clear();
                        let mut ctx = self.context.lock().unwrap();
                        ctx.push(ChatMessage {
                            role: "assistant".into(),
                            content: full_reply.clone(),
                        });
                        info!("[services::ai] ollama 完成 ({} 字符)", full_reply.chars().count());
                        return Ok(());
                    }
                }
            }
        }

        // 循环结束（正常 EOF / 错误 / 超时）：若 line_buf 还有残留，尝试解析
        if !line_buf.is_empty() {
            let tail = String::from_utf8_lossy(&line_buf);
            if let Ok(resp) = serde_json::from_str::<OllamaResp>(tail.trim()) {
                if let Some(msg) = &resp.message {
                    full_reply.push_str(&msg.content);
                    on_chunk(msg.content.clone());
                }
            }
        }

        let mut ctx = self.context.lock().unwrap();
        ctx.push(ChatMessage {
            role: "assistant".into(),
            content: full_reply.clone(),
        });
        info!("[services::ai] ollama 完成 ({} 字符)", full_reply.chars().count());
        Ok(())
    }

    /// 清空上下文
    pub fn clear_context(&self) {
        let mut ctx = self.context.lock().unwrap();
        ctx.clear();
    }

    /// 检查 AI 后端配置是否有效（UI 状态指示用，不发起网络请求）
    pub fn is_available(&self) -> bool {
        match *self.backend.lock().unwrap() {
            AiBackend::SiliconFlow => !self.silicon_key.lock().unwrap().is_empty(),
            AiBackend::Ollama => !self.ollama_url.lock().unwrap().is_empty(),
        }
    }
}
