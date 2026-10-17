// ============================================================
// Services · 文本翻译（Argos 离线主引擎 + 在线引擎回退链，移植自 rust-reference/translate.rs）
//
// 引擎：argos（本地离线主引擎，旁路子进程）/ ollama（本地 LLM）/ mymemory /
//      sf（SiliconFlow）/ youdao
// 回退链（argos 不可用时）：ollama → mymemory → sf
//
// ⚠️ 关于 Argos 离线主引擎（已实现，避免后来者误解）：
//    rust-reference/translate.rs **从未实现** Argos/CTranslate2 —— 它只有上述 4 个 HTTP 引擎。
//    本项目用独立子进程 `winocr-argos.exe`（ct2rs/CTranslate2，/MT 自洽，crt-static 旁路）
//    接入 Argos 离线翻译，避免把主工程翻成静态 CRT。模型在 `assets/argos_packages/`
//    （en_zh / zh_en 两方向包，各 82MB）。`argos` 为默认最高优先级引擎；子进程不可用
//    时自动降级到回退链 ollama → mymemory → sf（见 `ArgosBridge` 与 §4.6）。
//    详见 docs/Rust技术路线-Argos.md、app/技术.md §4.6。
//
// 与参考实现的差异：
//    - 参考用 reqwest::blocking；本工程已有 tokio 且对外是 async，故改用 reqwest 异步客户端，
//      避免在 async 上下文里阻塞线程。
//    - 未移植参考的「术语库占位保护」（本工程暂无 terminology 模块）。
//    - 语言用 core::types::Lang（Auto/En/Ja/Ko/Zh），而非裸字符串。
// ============================================================
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use crate::core::config::Config;
use crate::core::types::{Lang, TranslateResult};
use crate::services::terms::TermDb;

/// 回退链顺序（首项优先级最高）。
/// Argos 接入后在此数组最前面插入 "argos" 即可，无需改其它代码。
const CHAIN: [&str; 3] = ["ollama", "mymemory", "sf"];

// ============================================================
// Argos 离线主引擎 · 旁路子进程桥接
//
// 为什么是子进程：ct2rs/CTranslate2 必须统一为 /MT（crt-static），否则 LNK2038。
// 直接塞进 winocr 会把整个二进制翻 /MT（你标过风险的全局 CRT 影响）。故把 Argos
// 单独编成 /MT 自洽的 `winocr-argos.exe`，由本桥接层常驻拉起，stdin/stdout 行协议通信。
// 模型（82MB/方向）只加载一次，多请求复用。子进程不可用（exe 缺失/崩溃）时自动降级
// 到在线回退链，不影响主流程。
// ============================================================

#[derive(serde::Serialize)]
struct ArgosReq {
    id: u64,
    text: String,
    from: String,
    to: String,
}

#[derive(serde::Deserialize)]
struct ArgosResp {
    #[allow(dead_code)] // 请求 ID 保留用于调试
    id: u64,
    ok: bool,
    text: String,
    #[serde(default)]
    #[allow(dead_code)] // 置信度保留用于调试
    score: f32,
    #[serde(default)]
    error: String,
}

struct BridgeInner {
    child: Child,
    writer: BufWriter<std::process::ChildStdin>,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

pub struct ArgosBridge {
    inner: Arc<StdMutex<Option<BridgeInner>>>,
}

impl ArgosBridge {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(StdMutex::new(None)),
        }
    }

    /// 解析子进程 exe：环境变量 → exe 同级 → PATH。
    fn exe_path() -> PathBuf {
        if let Ok(p) = std::env::var("WINOCR_ARGOS_BIN") {
            return PathBuf::from(p);
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let c = dir.join("winocr-argos.exe");
                if c.exists() {
                    return c;
                }
            }
        }
        PathBuf::from("winocr-argos.exe")
    }

    /// 解析 assets 目录（含 argos_packages/），与子进程约定一致。
    ///
    /// 候选顺序与 `ocr::resolve_asset_file` 对齐（否则 debug 布局会解析失败）：
    ///   1) 环境变量 WINOCR_ASSETS_DIR
    ///   2) exe 同级 assets/（发布布局：<exe>/assets）
    ///   3) exe 上溯 1~3 级 assets/（debug 布局：target/debug → 仓库根，需上溯两级）
    ///   4) cwd 上溯 0~3 级 assets/
    /// 校验依据是候选目录下确实存在 argos_packages/，命中才返回。
    fn assets_dir() -> PathBuf {
        if let Ok(p) = std::env::var("WINOCR_ASSETS_DIR") {
            let p = PathBuf::from(p);
            if p.join("argos_packages").exists() {
                return p;
            }
            log::warn!(
                "[services::translate] WINOCR_ARGOS_ASSETS_DIR={} 下无 argos_packages/，改用自动探测",
                p.display()
            );
        }
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join("assets"));
                // debug：target/debug/winocr.exe → 上溯两级才是仓库根
                for up in ["..", "../..", "../../.."] {
                    candidates.push(dir.join(up).join("assets"));
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            for up in [".", "..", "../..", "../../.."] {
                candidates.push(cwd.join(up).join("assets"));
            }
        }
        if let Some(hit) = candidates
            .into_iter()
            .find(|d| d.join("argos_packages").exists())
        {
            log::info!("[services::translate] Argos assets 目录 = {}", hit.display());
            return hit;
        }
        log::warn!(
            "[services::translate] 未找到含 argos_packages/ 的 assets 目录，离线翻译将降级到在线回退链"
        );
        PathBuf::from("assets")
    }

    fn spawn() -> anyhow::Result<BridgeInner> {
        let exe = Self::exe_path();
        let assets = Self::assets_dir();
        // CREATE_NO_WINDOW：主程序为 GUI 子系统（无控制台），桥接子进程是 console
        // 子系统，不加此标志每次翻译都会闪黑控制台窗（v1.1.0 控制台隐藏配套）
        use std::os::windows::process::CommandExt;
        let mut child = Command::new(&exe)
            .env("WINOCR_ASSETS_DIR", &assets)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                anyhow!(
                    "启动离线翻译子进程失败（{exe:?}）：{e}。请先构建 argos-bridge 并把 winocr-argos.exe 放到 exe 同级（或设 WINOCR_ARGOS_BIN）"
                )
            })?;
        let writer = BufWriter::new(child.stdin.take().expect("piped stdin"));
        let reader = BufReader::new(child.stdout.take().expect("piped stdout"));
        Ok(BridgeInner {
            child,
            writer,
            reader,
            next_id: 1,
        })
    }

    /// 同步请求/响应（在 spawn_blocking 内调用，不阻塞 async 运行时）。
    fn request(
        inner: &Arc<StdMutex<Option<BridgeInner>>>,
        text: &str,
        from: &str,
        to: &str,
    ) -> Result<String> {
        let mut g = inner.lock().unwrap();
        // 子进程已退出或尚未启动 → 重新拉起
        let need_spawn = match g.as_mut() {
            Some(b) => b.child.try_wait()?.is_some(),
            None => true,
        };
        if need_spawn {
            log::info!("[services::translate] Argos 子进程未就绪，启动中…");
            *g = Some(Self::spawn()?);
        }
        let b = g.as_mut().unwrap();
        let id = b.next_id;
        b.next_id += 1;

        let req = ArgosReq {
            id,
            text: text.to_string(),
            from: from.to_string(),
            to: to.to_string(),
        };
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        b.writer.write_all(line.as_bytes())?;
        b.writer.flush()?;

        let mut resp_line = String::new();
        let n = b.reader.read_line(&mut resp_line)?;
        if n == 0 {
            return Err(anyhow!("Argos 子进程意外关闭管道"));
        }
        let resp: ArgosResp = serde_json::from_str(resp_line.trim())
            .map_err(|e| anyhow!("Argos 响应解析失败: {e}"))?;
        if resp.ok {
            Ok(resp.text)
        } else {
            Err(anyhow!("Argos 翻译失败: {}", resp.error))
        }
    }

    /// 异步翻译入口：失败返回 Err（调用方据此走回退链）。
    pub async fn translate(&self, text: &str, from: Lang, to: Lang) -> Result<String> {
        let inner = self.inner.clone();
        let text = text.to_string();
        let from = from.code().to_string();
        let to = to.code().to_string();
        tokio::task::spawn_blocking(move || Self::request(&inner, &text, &from, &to))
            .await
            .map_err(|e| anyhow!("Argos 任务调度失败: {e}"))?
    }
}

impl Drop for ArgosBridge {
    fn drop(&mut self) {
        if let Ok(mut g) = self.inner.lock() {
            if let Some(mut b) = g.take() {
                let _ = b.child.kill();
            }
        }
    }
}

pub struct Translate {
    cfg: std::sync::Mutex<Config>,
    client: reqwest::Client,
    argos: ArgosBridge,
    term_db: Arc<StdMutex<TermDb>>,
    term_enabled: Arc<StdMutex<bool>>,
}

impl Translate {
    pub fn new() -> Self {
        let cfg = Config::load().unwrap_or_else(|e| {
            log::warn!("[services::translate] 配置读取失败，用默认值: {e:#}");
            Config::default()
        });
        let client = reqwest::Client::builder()
            // 总超时 120s（大段文本 + 慢速引擎需要）
            .timeout(Duration::from_secs(120))
            // 连接超时 10s：网络不可达时快速失败，让回退链尽快走到下一个引擎，
            // 避免长时间挂起（配合多线程 runtime 的 block_on 调用点，缩短占用窗口）。
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|e| {
                log::warn!("[services::translate] reqwest 客户端构建失败，用默认: {e:#}");
                reqwest::Client::new()
            });
        let mut term_db = TermDb::new();
        term_db.merge_custom(&cfg.custom_terms);
        let builtin_n = term_db.builtin_count();
        let custom_n = term_db.custom_count();
        log::info!(
            "[services::translate] 术语库已加载：内置 {} 条，自定义 {} 条",
            builtin_n, custom_n
        );
        let term_db = Arc::new(StdMutex::new(term_db));
        let term_enabled = Arc::new(StdMutex::new(cfg.term_enabled));
        Self {
            cfg: std::sync::Mutex::new(cfg),
            client,
            argos: ArgosBridge::new(),
            term_db,
            term_enabled,
        }
    }

    fn cfg_lock(&self) -> std::sync::MutexGuard<'_, Config> {
        self.cfg.lock().unwrap()
    }

    /// 运行时新增/更新自定义术语（设置页调用），立即对后续翻译生效。
    pub fn add_custom_term(&self, src: &str, tgt: &str) {
        self.term_db.lock().unwrap().add_custom(src, tgt);
    }

    /// 运行时删除自定义术语（设置页调用）。
    pub fn remove_custom_term(&self, src: &str) {
        self.term_db.lock().unwrap().remove_custom(src);
    }

    /// 运行时切换术语库启用状态（设置页调用），立即对后续翻译生效。
    pub fn set_term_enabled(&self, v: bool) {
        *self.term_enabled.lock().unwrap() = v;
    }

    /// 运行时重配引擎与密钥（设置页「保存设置」调用，立即对后续翻译生效）。
    /// 只更新需要热更新的字段，不重建 reqwest 客户端与 ArgosBridge。
    pub fn configure(
        &self,
        engine: &str,
        sf_key: &str,
        ollama_url: &str,
        ollama_model: &str,
    ) {
        let mut c = self.cfg_lock();
        c.translate_engine = engine.to_string();
        c.ollama_url = ollama_url.to_string();
        c.ollama_model = ollama_model.to_string();
        if !sf_key.trim().is_empty() {
            c.sf_key = sf_key.to_string();
        }
        log::info!("[services::translate] runtime reconfigure -> engine={engine}");
    }

    /// 启动预热（Phase 3.1）：在后台拉起 Argos 子进程并加载 en→zh 模型，
    /// 避免首次真实翻译时冷启动（模型 82MB/方向，懒加载）。
    /// 失败仅告警——首次真实翻译会自动重试，并可能走在线回退链。
    pub async fn prewarm(&self) {
        log::info!("[services::translate] 后台预热 Argos（en→zh）…");
        // 加 60s 上限：模型加载偶发卡死时不至于让预热线程永久挂着
        let fut = self.argos.translate("prewarm", Lang::En, Lang::Zh);
        match tokio::time::timeout(Duration::from_secs(60), fut).await {
            Ok(Ok(t)) => log::info!("[services::translate] Argos 预热完成: {t:?}"),
            Ok(Err(e)) => {
                log::warn!(
                    "[services::translate] Argos 预热失败（首次真实翻译会重试/降级）: {e:#}"
                );
            }
            Err(_) => log::warn!("[services::translate] Argos 预热超时（60s），已跳过"),
        }
    }

    /// 翻译文本。
    /// - `from` / `to`：语言对；`to` 不能为 Auto（无意义）。
    /// - 返回译文与**实际命中**的引擎名（便于状态条展示回退链）。
    /// - 术语库启用时：翻译前将术语替换为占位符（避免翻译引擎修改专业术语），
    ///   翻译后将占位符还原为中文术语。
    pub async fn translate(&self, text: &str, from: Lang, to: Lang) -> Result<TranslateResult> {
        if text.trim().is_empty() {
            return Ok(TranslateResult::new(String::new(), "none"));
        }
        if to == Lang::Auto {
            return Err(anyhow!("目标语言不能为「自动检测」，请先指定目标语言"));
        }

        // 术语库替换：翻译前将专业术语替换为占位符，翻译后还原
        let (masked_text, restore_map) = self.term_db.lock().unwrap().apply(text);
        let text_to_translate = if *self.term_enabled.lock().unwrap() {
            &masked_text
        } else {
            text
        };

        let c311 = self.cfg_lock();
        let engine = c311.translate_engine.as_str();
        let (result, engine_name): (String, String) = match engine {
            "none" => (String::new(), "none".to_string()),
            "argos" => {
                match self.argos.translate(text_to_translate, from, to).await {
                    Ok(t) => (t, "argos".to_string()),
                    Err(e) => {
                        log::warn!(
                            "[services::translate] Argos 离线引擎不可用，降级回退链: {e:#}"
                        );
                        let t = self.chain(text_to_translate, from, to).await?.text;
                        (t, "chain".to_string())
                    }
                }
            }
            "sf" | "siliconflow" | "mymemory" | "ollama" | "youdao" => {
                let t = self.by_name(text_to_translate, from, to, engine).await?;
                let name = if engine == "siliconflow" { "sf".to_string() } else { engine.to_string() };
                (t, name)
            }
            _ => {
                let t = self.chain(text_to_translate, from, to).await?.text;
                (t, "chain".to_string())
            }
        };

        // 术语库还原：翻译后将占位符还原为中文术语
        let final_text = if *self.term_enabled.lock().unwrap() && !restore_map.is_empty() {
            self.term_db.lock().unwrap().restore(&result, &restore_map)
        } else {
            result.to_string()
        };

        Ok(TranslateResult::new(final_text, engine_name))
    }

    /// 按引擎名调用单个引擎。
    async fn by_name(&self, text: &str, from: Lang, to: Lang, name: &str) -> Result<String> {
        match name {
            "ollama" => self.ollama(text, to).await,
            "mymemory" => self.mymemory(text, from, to).await,
            "sf" | "siliconflow" => self.sf(text, to).await,
            "youdao" => self.youdao(text, from, to).await,
            other => Err(anyhow!("未知翻译引擎: {other}")),
        }
    }

    /// 回退链：依次尝试 CHAIN，第一个产出非空译文的引擎胜出。
    async fn chain(&self, text: &str, from: Lang, to: Lang) -> Result<TranslateResult> {
        let mut last: Option<anyhow::Error> = None;
        for name in CHAIN {
            match self.by_name(text, from, to, name).await {
                Ok(t) if !t.trim().is_empty() => {
                    log::info!("[services::translate] 回退链命中引擎: {name}");
                    return Ok(TranslateResult::new(t, name));
                }
                Ok(_) => last = Some(anyhow!("{name} 返回空内容")),
                Err(e) => {
                    log::warn!("[services::translate] 引擎 {name} 失败: {e:#}");
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("全部翻译引擎均失败")))
    }

    // ——— 引擎 1：Ollama（本地 LLM，非云端）———
    async fn ollama(&self, text: &str, to: Lang) -> Result<String> {
        if self.cfg_lock().ollama_model.trim().is_empty() {
            return Err(anyhow!("未配置 Ollama 模型（config.ollama_model 为空）"));
        }
        #[derive(Serialize)]
        struct Req {
            model: String,
            prompt: String,
            stream: bool,
        }
        #[derive(Deserialize)]
        struct Resp {
            response: String,
        }
        let body = Req {
            model: self.cfg_lock().ollama_model.clone(),
            prompt: format!(
                "Translate the following text into {}. Keep medical/technical terms accurate. \
                 Output ONLY the translation:\n\n{}",
                to.code(),
                text
            ),
            stream: false,
        };
        let url = format!("{}/api/generate", self.cfg_lock().ollama_url.trim_end_matches('/'));
        let resp: Resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        let out = resp.response.trim().to_string();
        if out.is_empty() {
            return Err(anyhow!("Ollama 返回空内容"));
        }
        Ok(out)
    }

    // ——— 引擎 2：MyMemory（免费，无需密钥）———
    async fn mymemory(&self, text: &str, from: Lang, to: Lang) -> Result<String> {
        // 注意：de=（邮箱）**仅在配置了邮箱时才带上**。实测空 de= 会被 MyMemory 判为
        // "INVALID EMAIL PROVIDED"（参考实现无条件拼接，会直接踩这个坑）。
        let base = format!(
            "https://api.mymemory.translated.net/get?q={}&langpair={}|{}",
            url_encode(text),
            from.code(),
            to.code()
        );
        let c429 = self.cfg_lock();
        let email = c429.mymemory_email.trim();
        let url = if email.is_empty() {
            base
        } else {
            format!("{base}&de={}", url_encode(email))
        };

        #[derive(Deserialize)]
        struct Resp {
            #[serde(rename = "responseData")]
            response_data: Data,
            #[serde(rename = "responseStatus")]
            response_status: Option<i64>,
            #[serde(rename = "responseDetails")]
            response_details: Option<String>,
        }
        #[derive(Deserialize)]
        struct Data {
            #[serde(rename = "translatedText")]
            translated_text: String,
        }
        let resp: Resp = self.client.get(&url).send().await?.json().await?;

        // 出错时 MyMemory 会把 "INVALID EMAIL PROVIDED" 之类塞进 translatedText，
        // 必须先看 responseStatus，否则会把错误文案当译文返回给用户。
        if let Some(st) = resp.response_status {
            if st != 200 {
                let detail = resp
                    .response_details
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| resp.response_data.translated_text.clone());
                return Err(anyhow!("MyMemory 返回错误状态 {st}: {detail}"));
            }
        }
        let out = resp.response_data.translated_text.trim().to_string();
        if out.is_empty() {
            return Err(anyhow!("MyMemory 返回空内容"));
        }
        Ok(out)
    }

    // ——— 引擎 3：SiliconFlow（OpenAI 兼容）———
    async fn sf(&self, text: &str, to: Lang) -> Result<String> {
        if self.cfg_lock().sf_key.trim().is_empty() {
            return Err(anyhow!("未配置 SiliconFlow API Key（config.sf_key 为空）"));
        }
        #[derive(Serialize)]
        struct Req {
            model: String,
            messages: Vec<Msg>,
            temperature: f32,
            max_tokens: u32,
        }
        #[derive(Serialize, Deserialize)]
        struct Msg {
            role: String,
            content: String,
        }
        #[derive(Deserialize)]
        struct Resp {
            choices: Vec<Choice>,
        }
        #[derive(Deserialize)]
        struct Choice {
            message: Msg,
        }
        let body = Req {
            model: self.cfg_lock().sf_model.clone(),
            messages: vec![
                Msg {
                    role: "system".into(),
                    content: format!(
                        "You are a professional translator. Translate the user text into {}. \
                         Keep medical/technical terms accurate. Output ONLY the translation, \
                         no commentary, no quotes.",
                        to.code()
                    ),
                },
                Msg {
                    role: "user".into(),
                    content: text.to_string(),
                },
            ],
            temperature: 0.3,
            max_tokens: 4096,
        };
        let url = format!("{}/chat/completions", self.cfg_lock().sf_url.trim_end_matches('/'));
        let resp: Resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.cfg_lock().sf_key))
            .json(&body)
            .send()
            .await?
            .json()
            .await?;
        resp.choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("SiliconFlow 返回空内容"))
    }

    // ——— 引擎 4：有道智云（signType=v3，MD5 签名）———
    async fn youdao(&self, text: &str, from: Lang, to: Lang) -> Result<String> {
        let c = self.cfg_lock();
        let app_key = c.youdao_app_key.trim();
        let app_secret = c.youdao_app_secret.trim();
        if app_key.is_empty() || app_secret.is_empty() {
            return Err(anyhow!(
                "未配置有道翻译密钥（请填写 config.youdao_app_key / youdao_app_secret）"
            ));
        }
        let from = youdao_lang(from.code());
        let to = youdao_lang(to.code());

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
        struct Resp {
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
        let resp: Resp = self
            .client
            .post("https://openapi.youdao.com/api")
            .form(&params)
            .send()
            .await?
            .json()
            .await?;
        if resp.error_code != "0" {
            return Err(anyhow!(
                "有道翻译错误 {}: {}",
                resp.error_code,
                resp.error_msg.unwrap_or_default()
            ));
        }
        resp.translation
            .and_then(|v| v.into_iter().next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("有道翻译返回空内容"))
    }
}

impl Default for Translate {
    fn default() -> Self {
        Self::new()
    }
}

/// 项目语言码 → 有道语言码
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

/// 百分编码（仅用于 MyMemory 的 query 拼接）
fn url_encode(input: &str) -> String {
    let mut result = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youdao_lang_mapping() {
        assert_eq!(youdao_lang("zh"), "zh-CHS");
        assert_eq!(youdao_lang("zh-CN"), "zh-CHS");
        assert_eq!(youdao_lang("en"), "en");
        assert_eq!(youdao_lang("ja"), "ja");
        assert_eq!(youdao_lang("ko"), "ko");
        assert_eq!(youdao_lang("auto"), "auto");
        assert_eq!(youdao_lang("fr"), "fr");
        assert_eq!(youdao_lang("unknown_x"), "unknown_x");
    }

    #[test]
    fn md5_hex_known_vectors() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn url_encode_basic() {
        assert_eq!(url_encode("hello"), "hello");
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("中文"), "%E4%B8%AD%E6%96%87");
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
    }

    /// 未配置密钥时必须明确报错，而不是静默回退其它引擎（对齐参考实现的行为）。
    #[tokio::test]
    async fn youdao_missing_key_errors_clearly() {
        let t = Translate::new();
        let r = t.youdao("hello world", Lang::En, Lang::Zh).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("未配置有道"));
    }

    /// 目标语言为 Auto 时必须拒绝（auto 只能作源语言）。
    #[tokio::test]
    async fn target_lang_cannot_be_auto() {
        let t = Translate::new();
        let r = t.translate("hello", Lang::En, Lang::Auto).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("自动检测"));
    }

    /// 空文本直接返回空结果，且不发起任何网络请求。
    #[tokio::test]
    async fn empty_text_short_circuit() {
        let t = Translate::new();
        let r = t.translate("   ", Lang::En, Lang::Zh).await.unwrap();
        assert_eq!(r.text, "");
        assert_eq!(r.engine, "none");
    }

    /// 真实网络冒烟：MyMemory 免费、无需密钥，用来验证整条 HTTP 链路真的通。
    /// 默认 ignore（离线/受限网络会失败，不能进常规回归）：
    ///   cargo test --release -p winocr translate -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn smoke_mymemory_real_network() {
        let t = Translate::new();
        // 直接调私有引擎方法，绕开配置（避免依赖/污染用户 config.json）
        match t.mymemory("Hello world", Lang::En, Lang::Zh).await {
            Ok(s) => {
                println!("[smoke] mymemory -> {s}");
                assert!(!s.trim().is_empty());
            }
            Err(e) => panic!("MyMemory 冒烟失败（可能网络不可达）: {e:#}"),
        }
    }
}
