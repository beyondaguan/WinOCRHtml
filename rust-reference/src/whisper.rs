//! Whisper 本地语音识别模块
//! 支持 tiny/base 模型，完全离线

use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;

pub struct WhisperEngine {
    model_path: PathBuf,
    binary_path: PathBuf,
}

impl WhisperEngine {
    pub fn new(model_dir: &str) -> Self {
        let model_path = PathBuf::from(model_dir).join("ggml-tiny.bin");
        let binary_path = PathBuf::from("native_host/whisper_cli.exe");
        Self {
            model_path,
            binary_path,
        }
    }

    /// 检查模型是否存在
    pub fn model_exists(&self) -> bool {
        self.model_path.exists()
    }

    /// 获取模型路径
    pub fn model_path(&self) -> &str {
        self.model_path.to_str().unwrap_or("")
    }

    /// 识别音频文件
    pub fn transcribe(&self, audio_path: &str) -> Result<String> {
        if !self.model_exists() {
            return Err(anyhow::anyhow!(
                "Whisper 模型不存在: {:?}，请运行 download_whisper.bat 下载",
                self.model_path
            ));
        }

        let output = Command::new(&self.binary_path)
            .arg("-m")
            .arg(&self.model_path)
            .arg("-f")
            .arg(audio_path)
            .arg("-l")
            .arg("auto")
            .arg("--output-txt")
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Whisper 识别失败: {}", stderr));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
    }

    /// 列出可用模型
    pub fn list_models(model_dir: &str) -> Vec<(String, u64)> {
        let mut models = Vec::new();
        let dir = PathBuf::from(model_dir);
        
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_file() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("ggml-") && name.ends_with(".bin") {
                            models.push((name, metadata.len()));
                        }
                    }
                }
            }
        }
        models
    }
}
