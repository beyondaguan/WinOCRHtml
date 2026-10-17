// ============================================================
// Services · TTS（Phase 2 新建）
// 回退链设计（开发指南 §3.3）：
//   edge-tts 在线 → OneCore 离线 → SAPI PowerShell 兜底
// 说明：
//   - edge-tts 需网络，走微软在线合成（预留，后续实现 WebSocket 协议）
//   - OneCore 需系统 Speech FoD（用户 Win10 Home 单语言版 19045 无 FoD，不可用）
//   - SAPI 兜底：PowerShell System.Speech，任何 Windows 均可用（当前默认）
// ============================================================
use anyhow::{Context, Result};
use log::{info, warn};
use std::process::Command;

/// TTS 引擎
#[allow(dead_code)] // 预留：设置页引擎选择
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsEngine {
    /// 在线合成（微软 edge-tts 服务）
    EdgeTts,
    /// Windows OneCore（WinRT SpeechSynthesis，需 FoD）
    OneCore,
    /// SAPI 5 兜底（PowerShell System.Speech）
    Sapi,
}

impl TtsEngine {
    #[allow(dead_code)] // 预留：设置页引擎选择显示
    pub fn label(&self) -> &'static str {
        match self {
            TtsEngine::EdgeTts => "edge-tts",
            TtsEngine::OneCore => "onecore",
            TtsEngine::Sapi => "sapi",
        }
    }
}

/// 语音合成服务
pub struct Tts;

impl Tts {
    pub fn new() -> Self {
        Self
    }

    /// 朗读文本（回退链：edge-tts → onecore → sapi）。
    /// 当前实际实现：仅 Sapi（稳定可用）；其余引擎返回 Unsupported 错误并记录。
    pub fn speak(&self, text: &str, lang: &str) -> Result<()> {
        self.speak_rate(text, lang, 0)
    }

    /// 带语速朗读（rate: SAPI Rate，-10 最慢 ~ 10 最快；0 = 正常）
    pub fn speak_rate(&self, text: &str, lang: &str, rate: i32) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let rate = rate.clamp(-10, 10);

        // 1) 尝试 edge-tts（预留：WebSocket 合成协议尚未实现）
        match self.try_edge_tts(text, lang) {
            Ok(true) => return Ok(()),
            Ok(false) => warn!("[tts] edge-tts 不可用，回退"),
            Err(e) => warn!("[tts] edge-tts 失败: {e}"),
        }

        // 2) OneCore（WinRT；无 FoD 时失败）
        match self.try_onecore(text, lang) {
            Ok(true) => return Ok(()),
            Ok(false) => warn!("[tts] onecore 不可用，回退"),
            Err(e) => warn!("[tts] onecore 失败: {e}"),
        }

        // 3) SAPI 兜底（PowerShell）
        self.speak_sapi(text, rate)
    }

    /// edge-tts（预留：返回「未实现」）
    fn try_edge_tts(&self, _text: &str, _lang: &str) -> Result<bool> {
        // TODO(P3): 实现 edge-tts WebSocket 合成协议（wss://speech.platform.bing.com/...）
        Ok(false)
    }

    /// OneCore（WinRT；无 Speech FoD 时返回 false）
    fn try_onecore(&self, _text: &str, _lang: &str) -> Result<bool> {
        // TODO(P3): 用 windows crate Media_SpeechSynthesis 实现。
        // 用户机器（Win10 Home 单语言版 19045）无 Speech FoD，此路径预期失败。
        Ok(false)
    }

    /// SAPI 兜底：PowerShell System.Speech（免依赖，任何 Windows 可用）
    fn speak_sapi(&self, text: &str, rate: i32) -> Result<()> {
        let ps_cmd = format!(
            "[System.Reflection.Assembly]::LoadWithPartialName('System.Speech') | Out-Null; \
             $synth = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
             $synth.Rate = {rate}; \
             $synth.Speak('{}')",
            text.replace('\'', "''").replace('\n', " ")
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-Command", &ps_cmd])
            .output()
            .context("启动 PowerShell 失败")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            warn!("[tts] SAPI 朗读失败: {err}");
            return Err(anyhow::anyhow!("SAPI 朗读失败: {err}"));
        }
        info!("[tts] SAPI 朗读完成 ({} chars)", text.chars().count());
        Ok(())
    }
}

/// 便捷函数：后台线程朗读（供 UI 直接调用，不阻塞主线程）
pub fn speak_async(text: String, lang: String) {
    speak_async_rate(text, lang, 0);
}

/// 带语速的后台朗读（rate: SAPI Rate，负值 = 慢速）
pub fn speak_async_rate(text: String, lang: String, rate: i32) {
    std::thread::spawn(move || {
        let tts = Tts::new();
        if let Err(e) = tts.speak_rate(&text, &lang, rate) {
            warn!("[tts] 朗读失败: {e:#}");
        }
    });
}