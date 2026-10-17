//! winocr-host.exe —— WinOCR 浏览器扩展的 Native Messaging Host。
//!
//! 协议（与 docs/WinOCR-开发指南.md §6.2 一致）：
//! ```text
//! 浏览器 → host：stdin 前 4 字节 = 小端 u32 长度，随后为 JSON 请求
//! host → 浏览器：stdout 前 4 字节 = 小端 u32 长度，随后为 JSON 响应
//! ```
//!
//! 请求格式：
//! ```json
//! {"id":"req-001","action":"translate","src":"prostate","srcLang":"en","tgtLang":"zh"}
//! {"id":"req-002","action":"collect","src":"prostate","tgt":"前列腺","tags":["泌尿"],"srcLang":"en","tgtLang":"zh"}
//! {"id":"req-003","action":"ping"}
//! ```
//!
//! 响应格式：
//! ```json
//! {"id":"req-001","ok":true,"tgt":"前列腺腺癌","engine":"argos"}
//! {"id":"req-002","ok":true}
//! {"id":"req-003","ok":true}
//! ```
//!
//! 依赖说明：
//! - 复用 `services::translate::Translate`（含 Argos 离线引擎 + 在线回退链）
//! - 复用 `services::collect::Collect`（SQLite 生词本）
//! - `Translate::translate` 为 async，故 host 内部维护一个单线程 tokio Runtime。

use std::io::{Read, Write};

use anyhow::{anyhow, Context, Result};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use serde::{Deserialize, Serialize};

use winocr::core::types::Lang;
use winocr::core::config::Config;
use winocr::services::collect::Collect;
use winocr::services::ocr;
use winocr::services::terms::TermDb;
use winocr::services::translate::Translate;

// ---------- 协议消息 ----------

#[derive(Debug, Deserialize)]
#[allow(non_snake_case, dead_code)] // 协议字段名与浏览器端约定一致（srcLang/tgtLang）；tgt/tags 保留兼容但未读取
struct Request {
    id: String,
    action: String,
    #[serde(default)]
    src: String,
    #[serde(default)]
    srcLang: String,
    #[serde(default)]
    tgtLang: String,
    #[serde(default)]
    tgt: String,
    #[serde(default)]
    tags: Vec<String>,
    /// list_collect 专用：返回条数上限（0 = 默认 500，最大 2000）
    #[serde(default)]
    limit: u32,
    // 说明：tgt / tags 目前 host 侧未读取 —— collect 走 ingest_text 自行从术语库取中文释义，
    // 不采用浏览器传来的译文（避免与术语库释义冲突）。字段保留以兼容协议，浏览器端仍在发送。
    /// ocr_translate 专用：base64 编码的 JPEG 截图
    #[serde(default)]
    img: String,
}

#[derive(Debug, Serialize)]
struct Response {
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tgt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    src_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl Response {
    fn ok(id: &str, tgt: Option<String>, engine: Option<String>) -> Self {
        Response { id: id.to_string(), ok: true, tgt, src_text: None, engine, error: None, data: None }
    }
    fn ok_data(id: &str, data: serde_json::Value) -> Self {
        Response { id: id.to_string(), ok: true, tgt: None, src_text: None, engine: None, error: None, data: Some(data) }
    }
    fn err(id: &str, e: &anyhow::Error) -> Self {
        Response {
            id: id.to_string(),
            ok: false,
            tgt: None,
            src_text: None,
            engine: None,
            error: Some(format!("{e:#}")),
            data: None,
        }
    }
}

// ---------- Native Messaging 帧读写 ----------

fn read_message() -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match std::io::stdin().read_exact(&mut len_buf) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e).context("读取长度前缀失败"),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > 16 * 1024 * 1024 {
        return Err(anyhow!("非法消息长度: {len}"));
    }
    let mut buf = vec![0u8; len];
    std::io::stdin()
        .read_exact(&mut buf)
        .with_context(|| format!("读取消息体失败（长度 {len}）"))?;
    Ok(Some(buf))
}

fn write_message(msg: &[u8]) -> Result<()> {
    let len = msg.len() as u32;
    let mut out = std::io::stdout().lock();
    out.write_all(&len.to_le_bytes())?;
    out.write_all(msg)?;
    out.flush()?;
    Ok(())
}

// ---------- 动作处理 ----------

fn handle_translate(rt: &tokio::runtime::Runtime, tr: &Translate, req: &Request) -> Response {
    let from = Lang::from_code(&req.srcLang);
    let to = Lang::from_code(&req.tgtLang);
    if to == Lang::Auto {
        return Response::err(&req.id, &anyhow!("目标语言不能为 auto"));
    }
    let text = req.src.trim();
    if text.is_empty() {
        return Response::err(&req.id, &anyhow!("翻译文本为空"));
    }
    match rt.block_on(tr.translate(text, from, to)) {
        Ok(r) => Response::ok(&req.id, Some(r.text), Some(r.engine)),
        Err(e) => Response::err(&req.id, &e),
    }
}

fn handle_collect(req: &Request, cfg: &Config, termdb: &TermDb) -> Response {
    let src = req.src.trim();
    let tgt = req.tgt.trim();
    if src.is_empty() && tgt.is_empty() {
        return Response::err(&req.id, &anyhow!("采集文本为空"));
    }
    // 采集池 = 原文 + 译文：无论原文/译文是哪种语言，只从中收英文词。
    // 典型场景：划选中文 → 译文为英文 → 译文中才是要学的英文词。
    // 切词只认 ASCII 字母，中文侧自动无产出，无需按语言分支。
    let mut pool = String::with_capacity(src.len() + tgt.len() + 1);
    pool.push_str(src);
    if !tgt.is_empty() {
        if !pool.is_empty() {
            pool.push('\n');
        }
        pool.push_str(tgt);
    }
    // context（学习时回忆出处）优先用原文；纯译文采集时退化为译文
    let context = if src.is_empty() { tgt } else { src };
    let c = Collect::new();
    let ignore = &cfg.ignore_words;
    let report = match c.ingest_text(
        &pool,
        &req.srcLang,
        &req.tgtLang,
        context,
        cfg.medical_only,
        ignore,
        termdb,
    ) {
        Ok(r) => r,
        Err(e) => return Response::err(&req.id, &e),
    };
    let items: Vec<serde_json::Value> = report
        .items
        .iter()
        .map(|i| {
            serde_json::json!({ "word": i.word, "project": i.project, "level": i.level })
        })
        .collect();
    let data = serde_json::json!({ "count": report.count, "items": items });
    Response::ok_data(&req.id, data)
}

/// 只读生词本：返回采集池现有条目（B 方案——桌面端为唯一真源，扩展侧只读镜像）
fn handle_list_collect(req: &Request) -> Response {
    let c = Collect::new();
    let limit = if req.limit == 0 { 500 } else { (req.limit as usize).min(2000) };
    let items: Vec<serde_json::Value> = c
        .list_all()
        .into_iter()
        .take(limit)
        .map(|i| {
            serde_json::json!({
                "id": i.id, "ts": i.ts, "word": i.word, "translation": i.translation,
                "project": i.project, "level": i.level, "context": i.context,
            })
        })
        .collect();
    let data = serde_json::json!({ "count": items.len(), "items": items });
    Response::ok_data(&req.id, data)
}

fn handle_ocr_translate(rt: &tokio::runtime::Runtime, tr: &Translate, req: &Request, cfg: &winocr::core::config::Config) -> Response {
    let img_b64 = req.img.trim();
    if img_b64.is_empty() {
        return Response::err(&req.id, &anyhow!("截图数据为空"));
    }
    // 解码 base64
    let img_data = match BASE64_STANDARD.decode(img_b64) {
        Ok(d) => d,
        Err(e) => return Response::err(&req.id, &anyhow!("base64 解码失败: {e}")),
    };
    // 解码图片
    let img = match image::load_from_memory(&img_data) {
        Ok(i) => i,
        Err(e) => return Response::err(&req.id, &anyhow!("图片解码失败: {e}")),
    };
    // OCR
    let text = match ocr::recognize_image(img) {
        Ok(t) => t,
        Err(e) => return Response::err(&req.id, &anyhow!("OCR 失败: {e}")),
    };
    let text = text.trim();
    if text.is_empty() {
        return Response::err(&req.id, &anyhow!("OCR 未识别出文本"));
    }
    // 翻译
    let from = Lang::from_code(&cfg.lang_from);
    let to = Lang::from_code(&cfg.lang_to);
    if to == Lang::Auto {
        return Response::err(&req.id, &anyhow!("目标语言不能为 auto"));
    }
    match rt.block_on(tr.translate(text, from, to)) {
        Ok(r) => Response { 
            id: req.id.clone(), 
            ok: true, 
            tgt: Some(r.text), 
            src_text: Some(text.to_string()),
            engine: Some(r.engine), 
            error: None,
            data: None,
        },
        Err(e) => Response::err(&req.id, &e),
    }
}

// ---------- 主循环 ----------

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // 复用桌面端配置（语言对、引擎等由 config.json 决定）
    let cfg = winocr::core::config::Config::load().unwrap_or_default();
    let default_tgt = cfg.lang_to.clone();
    log::info!(
        "[winocr-host] 启动：默认目标语言 {default_tgt}，翻译引擎 {}",
        cfg.translate_engine
    );

    let tr = Translate::new();
    // 验证生词本可初始化（真正采集时再建实例）
    let _c = Collect::new();
    // 内置医学术语库（学科分类 + 中文释义），采集批量分类时使用
    let termdb = TermDb::new();
    // 词典数据同步（best-effort；浏览器扩展经 host 查询英英释义）
    if let Err(e) = winocr::services::dict::sync_from_assets() {
        log::warn!("[winocr-host] 词典数据同步失败: {e:#}");
    }

    let rt = tokio::runtime::Runtime::new().context("创建 tokio runtime 失败")?;

    log::info!("[winocr-host] 就绪，等待浏览器消息…");

    loop {
        let raw = match read_message() {
            Ok(Some(v)) => v,
            Ok(None) => {
                log::info!("[winocr-host] stdin 关闭，退出");
                break;
            }
            Err(e) => {
                log::error!("[winocr-host] 读消息失败: {e:#}");
                // 尝试回写错误后继续；若写也失败则退出
                let resp = Response::err("req-0", &e);
                let _ = write_message(
                    &serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec()),
                );
                break;
            }
        };

        let req: Request = match serde_json::from_slice(&raw) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[winocr-host] JSON 解析失败: {e}");
                let resp = Response::err("req-0", &anyhow!("JSON 解析失败: {e}"));
                let _ = write_message(&serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec()));
                continue;
            }
        };

        log::info!("[winocr-host] 收到请求 id={} action={}", req.id, req.action);

        let resp = match req.action.as_str() {
            "ping" => Response::ok(&req.id, None, None),
            "translate" => handle_translate(&rt, &tr, &req),
            "collect" => handle_collect(&req, &cfg, &termdb),
            "list_collect" => handle_list_collect(&req),
            "ocr_translate" => handle_ocr_translate(&rt, &tr, &req, &cfg),
            other => Response::err(&req.id, &anyhow!("未知动作: {other}")),
        };

        match serde_json::to_vec(&resp) {
            Ok(bytes) => {
                if let Err(e) = write_message(&bytes) {
                    log::error!("[winocr-host] 写响应失败: {e:#}");
                    break;
                }
            }
            Err(e) => log::error!("[winocr-host] 序列化响应失败: {e}"),
        }
    }

    Ok(())
}