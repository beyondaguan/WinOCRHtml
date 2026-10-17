//! WinOCR 离线翻译子进程（Argos / CTranslate2 旁路）
//!
//! 设计要点（详见 app/技术.md §4.6）：
//! - 本进程用 `+crt-static` 单独编译为 /MT 自洽二进制，与主工程 winocr（/MD）完全隔离，
//!   避免把主工程整个翻成静态 CRT（你标过风险的「全局 CRT 影响」）。
//! - 常驻：模型（82MB/方向）只加载一次；winocr 通过 stdin/stdout 行协议多次复用。
//! - 行协议：每行一个 JSON。
//!     请求: {"id":u64,"text":String,"from":String,"to":String}
//!     响应: {"id":u64,"ok":bool,"text":String,"score":f32,"error":String}
//!   - from/to 用语言码 "en" / "zh" / "auto"。
//!   - 不支持的方向（ja/ko 等）返回 ok=false，主工程据此走在线回退链。
//!   - 收到文本 "EXIT" 或 stdin EOF → 干净退出。
//!
//! 用法（由 winocr 自动拉起，一般无需手动）：
//!   winocr-argos.exe
//! 环境变量：
//!   WINOCR_ASSETS_DIR  指向含 argos_packages/ 的 assets 目录（缺省按 exe 同级/上级推断）

use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{anyhow, ensure, Result};
use ct2rs::tokenizers::sentencepiece::Tokenizer;
use ct2rs::{Config, TranslationOptions, Translator};
use serde::Deserialize;
use serde::Serialize;

#[derive(Deserialize)]
struct Req {
    id: u64,
    text: String,
    from: String,
    to: String,
}

#[derive(Serialize)]
struct Resp {
    id: u64,
    ok: bool,
    text: String,
    score: f32,
    error: String,
}

/// 按 (from, to) 选 Argos 包名后缀；不支持的方向返回 None（交给主工程回退链）。
fn pick_package(from: &str, to: &str) -> Option<&'static str> {
    match (from, to) {
        ("en", "zh") => Some("en_zh"),
        ("zh", "en") => Some("zh_en"),
        ("auto", "zh") => Some("en_zh"),
        ("auto", "en") => Some("zh_en"),
        _ => None,
    }
}

/// sentencepiece ▁ 后处理：▁ 还原为空格，折叠多余空格，去首尾空白。
fn detok(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c == '▁' || c == ' ' {
            if last_space {
                continue;
            }
            out.push(' ');
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    out.trim().to_string()
}

struct Engine {
    translator: Translator<Tokenizer>,
}

fn load_engine(assets_base: &std::path::Path, dir: &str) -> Result<Engine> {
    let pkg = assets_base
        .join("argos_packages")
        .join(format!("translate-{dir}-1_9"));
    let model_dir = pkg.join("model");
    let spm = pkg.join("sentencepiece.model");
    ensure!(model_dir.exists(), "Argos 模型目录不存在: {}", model_dir.display());
    let tokenizer = Tokenizer::from_file(&spm, &spm)?;
    let translator = Translator::with_tokenizer(&model_dir, tokenizer, &Config::default())?;
    Ok(Engine { translator })
}

fn translate_text(engine: &Engine, text: &str) -> Result<(String, f32)> {
    let out = engine.translator.translate_batch(
        &[text],
        &TranslationOptions::default(),
        None,
    )?;
    let (t, s) = &out[0];
    Ok((detok(t), s.unwrap_or(0.0)))
}

fn resolve_assets() -> PathBuf {
    if let Ok(p) = std::env::var("WINOCR_ASSETS_DIR") {
        return PathBuf::from(p);
    }
    // 向上最多 4 级查找 assets 目录：兼容「exe 与 assets 同级」（发布），
    // 也兼容开发期 exe 在 target/release/ 而 assets 在仓库根的情况。
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent();
        for _ in 0..4 {
            if let Some(d) = dir {
                let cand = d.join("assets");
                if cand.exists() {
                    return cand;
                }
                dir = d.parent();
            } else {
                break;
            }
        }
    }
    PathBuf::from("assets")
}

fn write_resp(w: &mut impl Write, resp: &Resp) -> std::io::Result<()> {
    let mut line = serde_json::to_string(resp)
        .unwrap_or_else(|_| "{\"id\":0,\"ok\":false,\"text\":\"\",\"score\":0,\"error\":\"serialize\"}".to_string());
    line.push('\n');
    w.write_all(line.as_bytes())?;
    w.flush()
}

fn main() -> Result<()> {
    let assets_base = resolve_assets();
    eprintln!("[winocr-argos] assets_base = {}", assets_base.display());
    let cache: Mutex<HashMap<String, Engine>> = Mutex::new(HashMap::new());

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());
    let mut writer = BufWriter::new(stdout.lock());
    let mut buf = String::new();

    loop {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break; // EOF（主进程关闭了 stdin）
        }
        let req: Req = match serde_json::from_str(buf.trim()) {
            Ok(r) => r,
            Err(e) => {
                let _ = write_resp(
                    &mut writer,
                    &Resp {
                        id: 0,
                        ok: false,
                        text: String::new(),
                        score: 0.0,
                        error: format!("bad-request: {e}"),
                    },
                );
                continue;
            }
        };

        if req.text == "EXIT" {
            let _ = write_resp(
                &mut writer,
                &Resp {
                    id: req.id,
                    ok: true,
                    text: "bye".to_string(),
                    score: 0.0,
                    error: String::new(),
                },
            );
            break;
        }

        let dir = match pick_package(&req.from, &req.to) {
            Some(d) => d,
            None => {
                let _ = write_resp(
                    &mut writer,
                    &Resp {
                        id: req.id,
                        ok: false,
                        text: String::new(),
                        score: 0.0,
                        error: format!("unsupported-direction: {}-{}", req.from, req.to),
                    },
                );
                continue;
            }
        };

        // 懒加载并缓存该方向引擎；模型仅首次加载（82MB/方向）。
        // 闭包内持 cache 锁做推理，单客户端（winocr）下可接受。
        let result: Result<(String, f32)> = (|| {
            let mut cache = cache.lock().unwrap();
            if !cache.contains_key(dir) {
                let e = load_engine(&assets_base, dir)
                    .map_err(|e| anyhow!("load-failed: {e:#}"))?;
                cache.insert(dir.to_string(), e);
            }
            let eng = cache.get(dir).unwrap();
            translate_text(eng, &req.text)
        })();

        match result {
            Ok((t, s)) => {
                let _ = write_resp(
                    &mut writer,
                    &Resp {
                        id: req.id,
                        ok: true,
                        text: t,
                        score: s,
                        error: String::new(),
                    },
                );
            }
            Err(e) => {
                let _ = write_resp(
                    &mut writer,
                    &Resp {
                        id: req.id,
                        ok: false,
                        text: String::new(),
                        score: 0.0,
                        error: format!("{e:#}"),
                    },
                );
            }
        }
    }
    Ok(())
}
