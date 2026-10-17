// ============================================================
// Services · OCR（真实实现）
// 主引擎：PP-OCRv6 ONNX 推理（移植自 rust-reference/ocr.rs，ort 2.0-rc.13，
//        det/rec 双模型，CTC 解码 + 连通域合并 + 透视裁剪 + 几何排序）。
// 兜底：Windows.Media.Ocr 系统引擎（零模型体积；中文质量弱，PP-OCR 失败时用）。
// 模型：assets/models/ocr/{det_tiny,rec_tiny}.onnx + ppocr_dict.txt（= 3.4 RapidOCR tiny 档）。
// 维护约定：算法逻辑与 rust-reference/ocr.rs 保持同步，改动需双向回补。
// ============================================================
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result};

/// ort 全局初始化（只需一次）
static ORT_INIT: OnceLock<()> = OnceLock::new();

/// 模型/运行库目录候选（cwd = 仓库根时 assets 就地可用；打包后用 exe 同级 assets）
const ASSET_DIR_CANDIDATES: [&str; 3] = ["assets", "../assets", "../../assets"];

fn resolve_asset_file(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        for c in ASSET_DIR_CANDIDATES {
            dirs.push(cwd.join(c).join(name));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            dirs.push(exe_dir.join(name));
            for c in ASSET_DIR_CANDIDATES {
                dirs.push(exe_dir.join(c).join(name));
            }
        }
    }
    dirs.into_iter().find(|p| p.exists())
}

fn resolve_model_dir() -> Result<PathBuf> {
    resolve_asset_file("models/ocr/ppocr_dict.txt")
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or_else(|| anyhow::anyhow!("找不到模型目录（ppocr_dict.txt）：cwd/assets/models/ocr 等"))
}

// ============================================================
// Phase 3.2 · OCR dhash 缓存
// 对输入图片计算感知哈希（dHash），命中 ocr_cache 表则直接返回上次结果，
// 避免相同/近似截图反复做 ONNX 推理（截图重识别/剪贴板粘贴等场景收益最大）。
// 开关来自 Config::cache_enabled（默认 true），设置页可运行时切换。
// ============================================================

/// OCR 缓存全局开关（启动时从 Config 读取，设置页可热更新）
static OCR_CACHE_ENABLED: OnceLock<AtomicBool> = OnceLock::new();

/// 读取 OCR 缓存开关（惰性初始化：首次调用从 Config 读取）
pub fn ocr_cache_enabled() -> bool {
    OCR_CACHE_ENABLED
        .get_or_init(|| {
            let v = crate::core::config::Config::load()
                .map(|c| c.cache_enabled)
                .unwrap_or(true);
            AtomicBool::new(v)
        })
        .load(Ordering::Relaxed)
}

/// 运行时更新 OCR 缓存开关（设置页「保存设置」调用，立即生效）
#[allow(dead_code)] // 设置页接线后启用
pub fn set_ocr_cache_enabled(v: bool) {
    OCR_CACHE_ENABLED
        .get_or_init(|| AtomicBool::new(v))
        .store(v, Ordering::Relaxed);
}

/// 计算图片 dHash（difference hash，8x8 灰度缩放 → 64 bit）。
/// 相邻像素亮度比较，对缩放/轻微色差鲁棒，适合「同一区域重复截图」命中。
fn dhash(img: &image::DynamicImage) -> u64 {
    // 9x8 灰度：水平相邻 8 对像素 × 8 行 = 64 bit
    let small = img
        .resize_exact(9, 8, image::imageops::FilterType::Triangle)
        .to_luma8();
    let mut hash: u64 = 0;
    for y in 0..8 {
        for x in 0..8 {
            let left = small.get_pixel(x, y)[0];
            let right = small.get_pixel(x + 1, y)[0];
            hash = (hash << 1) | u64::from(left > right);
        }
    }
    hash
}

fn ensure_ort_init() {
    ORT_INIT.get_or_init(|| {
        match resolve_asset_file("onnxruntime.dll") {
            Some(dll) => {
                // load-dynamic feature：ORT_DYLIB_PATH 指定运行库位置
                std::env::set_var("ORT_DYLIB_PATH", &dll);
                log::info!("[services::ocr] ORT_DYLIB_PATH = {dll:?}");
            }
            None => log::warn!("[services::ocr] 未找到 assets/onnxruntime.dll，尝试系统默认位置"),
        }
        ort::init().with_name("winocr-host").commit();
    });
}

// ============================================================
// 公共 API
// ============================================================

/// 识别图片文件，返回按行拼接的文本。
/// 主引擎 PP-OCR；失败/空结果时兜底 Windows.Media.Ocr。
/// 阻塞调用（ONNX 推理 / WinRT get()），**请在非 UI 线程调用**。
pub fn recognize_file(path: &Path) -> Result<String> {
    let img_data = std::fs::read(path).with_context(|| format!("读取图片失败: {path:?}"))?;
    let img = image::load_from_memory(&img_data)
        .map_err(|e| anyhow::anyhow!("图片加载失败: {e}"))?;
    recognize_image(img)
}

/// 下载 URL 到临时文件 → 识别。阻塞调用，请在非 UI 线程调用。
pub fn recognize_url(url: &str) -> Result<String> {
    let resp = reqwest::blocking::get(url)
        .map_err(|e| anyhow::anyhow!("下载失败: {e}"))?;
    let data = resp.bytes()
        .map_err(|e| anyhow::anyhow!("读取响应失败: {e}"))?;
    let img = image::load_from_memory(&data)
        .map_err(|e| anyhow::anyhow!("图片解码失败: {e}"))?;
    recognize_image(img)
}

/// 识别已解码的图片（内存直传，无 PNG 往返）。capture 路径与 recognize_file 共用此入口。
/// 阻塞调用（ONNX 推理 / WinRT get()），**请在非 UI 线程调用**。
///
/// Phase 3.2：入口先查 dhash 缓存，命中直接返回上次结果；未命中跑完整 OCR 后写缓存。
pub fn recognize_image(img: image::DynamicImage) -> Result<String> {
    // 缓存命中时连 ORT 都不需要初始化，放最前面
    let cache_key: Option<String> = if ocr_cache_enabled() {
        let key = format!("{:016x}", dhash(&img));
        match crate::services::persist::Persist::new().ocr_cache_get(&key) {
            Ok(Some(cached)) if !cached.trim().is_empty() => {
                log::info!("[services::ocr] dhash 缓存命中 {key}（{} 字符）", cached.chars().count());
                return Ok(cached);
            }
            Ok(_) => Some(key),
            Err(e) => {
                log::warn!("[services::ocr] 查询 dhash 缓存失败（忽略，继续 OCR）: {e:#}");
                Some(key)
            }
        }
    } else {
        None
    };

    let result = recognize_image_uncached(&img);

    // 成功且非空 → 写缓存（仅缓存非空结果，避免缓存「识别不出」的坏结果）
    if let (Some(key), Ok(text)) = (&cache_key, &result) {
        if !text.trim().is_empty() {
            if let Err(e) = crate::services::persist::Persist::new().ocr_cache_put(key, text) {
                log::warn!("[services::ocr] 写入 dhash 缓存失败（忽略）: {e:#}");
            }
        }
    }
    result
}

/// 识别已解码图片（不走缓存，内部实现）。
fn recognize_image_uncached(img: &image::DynamicImage) -> Result<String> {
    ensure_ort_init();

    // 主引擎：PP-OCR
    match recognize_ppocr(img) {
        Ok(text) if !text.trim().is_empty() => {
            log::info!("[services::ocr] PP-OCR 完成（{} 字符）", text.chars().count());
            return Ok(text);
        }
        Ok(_) => log::warn!("[services::ocr] PP-OCR 未识别出文本，转系统 OCR 兜底"),
        Err(e) => {
            // 用户取消：直接透传，绝不能落进系统 OCR 兜底（否则取消无效还继续烧 CPU）
            if crate::services::cancel::is_cancelled() {
                return Err(e);
            }
            log::warn!("[services::ocr] PP-OCR 失败（{e:#}），转系统 OCR 兜底")
        },
    }

    // 兜底：Windows.Media.Ocr
    recognize_windows_data(img)
}

/// 由 capture 直接传来的 24bpp RGB 内存缓冲识别（A 优化：跳过 PNG 编码/落盘/读盘/解码）。
pub fn recognize_capture(rgb: &[u8], w: u32, h: u32) -> Result<String> {
    let buf = image::ImageBuffer::<image::Rgb<u8>, Vec<u8>>::from_raw(w, h, rgb.to_vec())
        .ok_or_else(|| anyhow::anyhow!("capture rgb 尺寸不匹配 ({w}x{h}, {} bytes)", rgb.len()))?;
    recognize_image(image::DynamicImage::ImageRgb8(buf))
}

/// 启动时预热：提前把 det/rec 模型加载进 ORT 会话，
/// 避免首次框选 OCR 时因懒加载（create_session）产生明显卡顿。
/// 失败时仅告警，首次 recognize_file 会自动重试加载。
pub fn prewarm() {
    let t0 = std::time::Instant::now();
    // 必须在主线程触发一次 ort 全局环境初始化（详见 ui/mod.rs 的注释）：
    // 把 ORT_INIT 的 OnceLock 在主线程落定，后续后台线程的 recognize_file 只会命中
    // OnceLock 短路，绝不会再在后台线程调用 ort::init()，从而避免 off-main-thread panic。
    ensure_ort_init();
    let res = with_pp_engine(|engine| {
        let dir = resolve_model_dir()?;
        engine.init_sessions(&dir)
    });
    match res {
        Ok(_) => log::info!(
            "[services::ocr] 模型预热完成，耗时 {:.2?}",
            t0.elapsed()
        ),
        Err(e) => log::warn!(
            "[services::ocr] 模型预热失败（首次 OCR 时会重试）: {e:#}"
        ),
    }
}

// ============================================================
// 主引擎：PP-OCR（移植自 rust-reference/ocr.rs）
// ============================================================

pub struct PpOcrEngine {
    det_session: Option<ort::session::Session>,
    /// 识别并行池：每个 worker 线程持一个独立会话（ort::Session 不可 Clone —— 官方建议 "one session per thread"）。
    /// 每次识别从池中取走全部会话，并发推理后归还，避免每次 OCR 重新加载模型。
    rec_sessions: Vec<ort::session::Session>,
    dict: Arc<Vec<String>>,
    rec_input_height: u32,
    max_wh_ratio: f32,
}

static PP_ENGINE: Mutex<Option<PpOcrEngine>> = Mutex::new(None);

fn with_pp_engine<T>(f: impl FnOnce(&mut PpOcrEngine) -> Result<T>) -> Result<T> {
    // 容忍上一轮因 panic 导致的 poisoned 状态：直接接管内部数据，
    // 避免整个 OCR 服务因一次异常而永久瘫痪（之前的表现是每次 OCR 都 PoisonError）。
    let mut guard = PP_ENGINE
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    if guard.is_none() {
        *guard = Some(init_pp_engine()?);
    }

    // 推理（尤其 ort 内部）偶尔会 panic 而非返回 Err。用 catch_unwind 兜住，
    // 避免 Mutex 被永久 poison；一旦发生，丢弃损坏的引擎，下次调用自动重建。
    let engine = guard.as_mut().unwrap();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(engine)));

    match result {
        Ok(r) => r,
        Err(_) => {
            *guard = None; // 推理中途 panic：引擎可能已损坏，丢弃以便重建
            Err(anyhow::anyhow!(
                "OCR 引擎推理时发生内部异常，已重置引擎（下次调用将自动重建）"
            ))
        }
    }
}

fn init_pp_engine() -> Result<PpOcrEngine> {
    let model_dir = resolve_model_dir()?;
    log::info!("[services::ocr] model dir = {model_dir:?}");

    // 字典（tiny 档）
    let dict_path = model_dir.join("ppocr_dict.txt");
    let dict: Vec<String> = if dict_path.exists() {
        std::fs::read_to_string(&dict_path)
            .with_context(|| format!("读取字典失败: {dict_path:?}"))?
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        log::warn!("[services::ocr] 字典不存在: {dict_path:?}");
        vec![" ".to_string()]
    };

    Ok(PpOcrEngine {
        det_session: None,
        rec_sessions: Vec::new(),
        dict: Arc::new(dict),
        rec_input_height: 48, // 注意：rec_tiny.onnx 输入高度固定 48；改 32 会维度不匹配导致整条 PP-OCR 失效
        max_wh_ratio: 12.0, // tiny 档
    })
}

fn recognize_ppocr(img: &image::DynamicImage) -> Result<String> {
    // 取消检查：OCR 开始前检查，避免用户取消后仍做无用推理
    if crate::services::cancel::is_cancelled() {
        return Err(anyhow::anyhow!("OCR 已取消"));
    }

    with_pp_engine(|engine| {
        // 1. 预处理（小图放大 + 32 对齐）
        let t_pre = std::time::Instant::now();
        let processed = engine.preprocess(img)?;
        log::info!("[services::ocr][diag] preprocess 耗时 {:.1?}", t_pre.elapsed());

        // 3. 延迟初始化会话
        let model_dir = resolve_model_dir()?;
        engine.init_sessions(&model_dir)?;

        // 阶段间取消检查（大图 OCR 可达数十秒，取消要能及时生效）
        if crate::services::cancel::is_cancelled() {
            return Err(anyhow::anyhow!("OCR 已取消"));
        }

        // 4. 检测（DBNet）
        let t_det = std::time::Instant::now();
        let raw_boxes = engine.detect(&processed)?;
        log::info!("[services::ocr][diag] detect 耗时 {:.1?}", t_det.elapsed());
        let t_merge = std::time::Instant::now();

        // 5. 连通域合并 + 最小外接矩形
        let mut merged_boxes = merge_connected_regions(&raw_boxes);
        log::info!(
            "[services::ocr] det: {} 像素, {} 框 (预处理 {}x{} -> 原图 {}x{})",
            raw_boxes.len(),
            merged_boxes.len(),
            processed.width,
            processed.height,
            img.width(),
            img.height()
        );

        // 5.5 概率图坐标 → 原图坐标
        let scale_x = img.width() as f32 / processed.width as f32;
        let scale_y = img.height() as f32 / processed.height as f32;
        for b in &mut merged_boxes {
            for p in &mut b.points {
                p.0 *= scale_x;
                p.1 *= scale_y;
            }
            b.cx *= scale_x;
            b.cy *= scale_y;
        }

        // 5.6 框外扩（等效 unclip 膨胀）
        let img_w = img.width() as f32;
        let img_h = img.height() as f32;
        for b in &mut merged_boxes {
            let xs = [b.points[0].0, b.points[1].0, b.points[2].0, b.points[3].0];
            let ys = [b.points[0].1, b.points[1].1, b.points[2].1, b.points[3].1];
            let min_x = xs.iter().cloned().fold(f32::MAX, f32::min);
            let max_x = xs.iter().cloned().fold(f32::MIN, f32::max);
            let min_y = ys.iter().cloned().fold(f32::MAX, f32::min);
            let max_y = ys.iter().cloned().fold(f32::MIN, f32::max);
            let pad = (0.35 * (max_y - min_y)).max(2.0);
            let nx0 = (min_x - pad).max(0.0);
            let nx1 = (max_x + pad).min(img_w - 1.0);
            let ny0 = (min_y - pad).max(0.0);
            let ny1 = (max_y + pad).min(img_h - 1.0);
            if nx1 - nx0 < 4.0 || ny1 - ny0 < 4.0 {
                continue;
            }
            b.points = [(nx0, ny0), (nx1, ny0), (nx1, ny1), (nx0, ny1)];
            b.cx = (nx0 + nx1) / 2.0;
            b.cy = (ny0 + ny1) / 2.0;
        }

        // 6. 裁剪 + 识别
        if crate::services::cancel::is_cancelled() {
            return Err(anyhow::anyhow!("OCR 已取消"));
        }
        log::info!("[services::ocr][diag] merge+unclip 耗时 {:.1?}", t_merge.elapsed());
        // 性能关键：膨胀后框已是轴对齐矩形（line 190），直接用 image::imageops::crop_imm
        // 替代逐像素透视变换（快 50-100x），配合 Triangle 替代 Lanczos3（快 3-5x，OCR 质量无损）
        let t0 = std::time::Instant::now();
        let mut text_lines = engine.recognize_cropped_fast(&img, &merged_boxes)?;
        let t1 = t0.elapsed();
        log::info!("[services::ocr] 裁剪+识别 {} 框 完成，耗时 {:.1}s", merged_boxes.len(), t1.as_secs_f32());

        // 7. 几何排序

        // 7. 几何排序
        sort_text_lines(&mut text_lines);

        // 8. 拼接（score >= 0.5 过滤）
        Ok(text_lines
            .into_iter()
            .filter(|l| l.score >= 0.5)
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n"))
    })
}

impl PpOcrEngine {
    fn init_sessions(&mut self, model_dir: &Path) -> Result<()> {
        // det：单次推理，吃满 intra 并行（clamp 2..8）
        if self.det_session.is_none() {
            let names = ["det_tiny.onnx", "PP-OCRv6_det_tiny.onnx"];
            let path = names.iter().map(|n| model_dir.join(n)).find(|p| p.exists());
            match path {
                Some(p) => self.det_session = Some(create_session_with_intra(&p, det_intra())?),
                None => log::warn!("[services::ocr] 模型不存在: {names:?} @ {model_dir:?}"),
            }
        }
        // rec：并行池。ort::Session 不可 Clone，官方建议 "one session per thread"，
        // 故建 N 个独立会话（每个 intra_threads=1），并发交给 OS 线程池；
        // intra=1 避免 ORT 内部线程与 OS 线程池争抢，整体并行度交给分块并发。
        if self.rec_sessions.is_empty() {
            let names = ["rec_tiny.onnx", "PP-OCRv6_rec_tiny.onnx"];
            let path = names.iter().map(|n| model_dir.join(n)).find(|p| p.exists());
            match path {
                Some(p) => {
                    let pool = rec_pool_size();
                    let mut v = Vec::with_capacity(pool);
                    for _ in 0..pool {
                        v.push(create_session_with_intra(&p, 1)?);
                    }
                    log::info!("[services::ocr] rec 并行池大小 = {pool}（每项 intra_threads=1）");
                    self.rec_sessions = v;
                }
                None => log::warn!("[services::ocr] 模型不存在: {names:?} @ {model_dir:?}"),
            }
        }
        Ok(())
    }

    fn preprocess(&self, img: &image::DynamicImage) -> Result<ProcessedImage> {
        let w = img.width();
        let h = img.height();
        let max_side = std::env::var("WINOCR_MAX_SIDE")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&v| v >= 320)
            .unwrap_or(960);

        // 单次 resize 直达目标（2026-10-17 锁死修复）：
        // 旧实现「短边<256 先放大到短边 512」再「长边封顶 960」两连 resize，
        // 宽条形截图（如 1852×176）会先吹成 5371×512 的中间图（11 倍像素量），
        // debug 构建下仅预处理就 3.5s（实测日志）。合并为一个缩放系数一次到位：
        //   scale = min(小图放大系数, 长边封顶系数)，无中间大图。
        let short = w.min(h);
        let long = w.max(h);
        let up = if short < 256 { 512.0 / short as f32 } else { 1.0 };
        let cap = if long as f32 * up > max_side as f32 {
            max_side as f32 / (long as f32 * up)
        } else {
            1.0
        };
        let scale = up * cap;
        let mut img = if (scale - 1.0).abs() > 1e-3 {
            img.resize_exact(
                (w as f32 * scale) as u32,
                (h as f32 * scale) as u32,
                image::imageops::FilterType::Triangle,
            )
        } else {
            img.to_owned()
        };

        // det 模型要求输入宽高为 32 的倍数（就近取整，避免无谓放大）
        let align32 = |v: u32| -> u32 { ((v + 16) / 32) * 32 };
        let (tw, th) = (align32(img.width()), align32(img.height()));
        if tw != img.width() || th != img.height() {
            img = img.resize_exact(tw, th, image::imageops::FilterType::Triangle);
        }

        let rgb = img.to_rgb8();
        Ok(ProcessedImage {
            width: rgb.width() as i32,
            height: rgb.height() as i32,
            data: rgb.into_raw(),
        })
    }

    fn detect(&mut self, img: &ProcessedImage) -> Result<Vec<RawPixel>> {
        let session = match &mut self.det_session {
            Some(s) => s,
            None => {
                log::warn!("[services::ocr] 检测模型未加载，跳过检测");
                return Ok(Vec::new());
            }
        };

        let h = img.height as usize;
        let w = img.width as usize;

        // NCHW [1,3,H,W]，ImageNet 归一化
        let mut nchw = vec![0f32; 3 * h * w];
        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) * 3;
                let r = img.data[idx] as f32 / 255.0;
                let g = img.data[idx + 1] as f32 / 255.0;
                let b = img.data[idx + 2] as f32 / 255.0;
                nchw[y * w + x] = (r - 0.485) / 0.229;
                nchw[h * w + y * w + x] = (g - 0.456) / 0.224;
                nchw[2 * h * w + y * w + x] = (b - 0.406) / 0.225;
            }
        }

        let input_array = ndarray::Array4::<f32>::from_shape_vec((1, 3, h, w), nchw)?;
        let outputs =
            session.run(ort::inputs![ort::value::TensorRef::from_array_view(&input_array)?])?;

        // 解析输出 [1, 1, H, W] 概率图（阈值可用 WINOCR_DET_THRESH 覆盖，便于调参）
        for (_name, output) in &outputs {
            let (shape, data) = output
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("输出解析失败: {e}"))?;

            if shape.len() == 4 && shape[0] == 1 && shape[1] == 1 {
                let th = shape[2] as usize;
                let tw = shape[3] as usize;
                let threshold = std::env::var("WINOCR_DET_THRESH")
                    .ok()
                    .and_then(|s| s.parse::<f32>().ok())
                    .unwrap_or(0.3);
                let mut pixels = Vec::new();

                for y in 0..th {
                    for x in 0..tw {
                        let idx = y * tw + x;
                        if data[idx] > threshold {
                            pixels.push(RawPixel { x: x as i32, y: y as i32, score: data[idx] });
                        }
                    }
                }
                return Ok(pixels);
            }
        }
        Ok(Vec::new())
    }

    #[allow(dead_code)] // 备用：慢路径（透视变换），快路径不用时仍保留以防退化
    fn recognize_cropped(
        &mut self,
        img: &image::DynamicImage,
        boxes: &[DetectedBox],
    ) -> Result<Vec<TextLine>> {
        let session = match self.rec_sessions.first_mut() {
            Some(s) => s,
            None => {
                log::warn!("[services::ocr] 识别模型未加载，跳过识别");
                return Ok(Vec::new());
            }
        };

        let mut results = Vec::new();

        for box_item in boxes {
            let cropped = match crop_with_perspective(img, box_item) {
                Some(c) => c,
                None => continue,
            };

            let scale = self.rec_input_height as f32 / cropped.height() as f32;
            let mut new_w = (cropped.width() as f32 * scale) as u32;
            let max_w = (self.rec_input_height as f32 * self.max_wh_ratio) as u32;
            if new_w > max_w {
                new_w = max_w;
            }
            if new_w < self.rec_input_height {
                new_w = self.rec_input_height;
            }

            let resized = cropped.resize_exact(
                new_w,
                self.rec_input_height,
                image::imageops::FilterType::Lanczos3,
            );

            // rec 输入 [1,3,48,W]，CHW，(-0.5)/0.5 归一化
            let rgb = resized.to_rgb8();
            let (rw, rh) = (rgb.width() as usize, rgb.height() as usize);
            let mut chw: Vec<f32> = Vec::with_capacity(3 * rh * rw);
            for c in 0..3 {
                for y in 0..rh {
                    for x in 0..rw {
                        let p = rgb.get_pixel(x as u32, y as u32);
                        chw.push((p[c] as f32 / 255.0 - 0.5) / 0.5);
                    }
                }
            }

            let input = ndarray::Array4::<f32>::from_shape_vec((1, 3, rh, rw), chw)
                .map_err(|e| anyhow::anyhow!("创建输入 tensor 失败: {e}"))?;

            let outputs =
                session.run(ort::inputs![ort::value::TensorRef::from_array_view(&input)?])?;

            for (_name, output) in &outputs {
                let (shape, data) = output
                    .try_extract_tensor::<f32>()
                    .map_err(|e| anyhow::anyhow!("输出解析失败: {e}"))?;

                if shape.len() == 3 {
                    // [batch, T, C]：CTC 贪婪解码
                    let t = shape[1] as usize;
                    let c = shape[2] as usize;

                    let mut text = String::new();
                    let mut prev_idx: Option<usize> = None;
                    let mut total_score = 0.0f32;
                    let mut char_count = 0usize;

                    for step in 0..t {
                        let mut max_idx = 0usize;
                        let mut max_val = f32::MIN;
                        for j in 0..c {
                            let idx = step * c + j;
                            let val = data[idx];
                            if val > max_val {
                                max_val = val;
                                max_idx = j;
                            }
                        }

                        if max_idx != 0 && Some(max_idx) != prev_idx {
                            if max_idx <= self.dict.len() {
                                text.push_str(&self.dict[max_idx - 1]);
                                total_score += max_val;
                                char_count += 1;
                            } else if max_idx == self.dict.len() + 1 {
                                // 末位类别 = 空格（PP-OCR CTC: blank + dict + space）
                                text.push(' ');
                                total_score += max_val;
                                char_count += 1;
                            }
                        }
                        prev_idx = Some(max_idx);
                    }

                    if !text.is_empty() {
                        let avg_score = if char_count > 0 {
                            total_score / char_count as f32
                        } else {
                            0.0
                        };
                        results.push(TextLine {
                            text,
                            score: avg_score,
                            x: box_item.cx as i32,
                            y: box_item.cy as i32,
                        });
                    }
                }
            }
        }

        Ok(results)
    }

    /// 裁剪+识别（快路径：膨胀后框已轴对齐 → crop_imm 替代透视变换）。
    /// 性能关键：跨所有行收集全部切块，再按 rec 并行池 worker 数顺序分片，
    /// 每片交给一个独立 rec 会话（OS 线程）并发做批量 Session::run，
    /// 把"逐块串行"的上百次 run 变成 N 路并发，最大化多核吞吐。
    fn recognize_cropped_fast(
        &mut self,
        img: &image::DynamicImage,
        boxes: &[DetectedBox],
    ) -> Result<Vec<TextLine>> {
        if self.rec_sessions.is_empty() {
            log::warn!("[services::ocr] 识别模型未加载，跳过识别");
            return Ok(Vec::new());
        }

        let rgb = img.to_rgb8();

        // 收集所有切块（跨全部行），并记下每块是否行末块 / 该行的 overlap 去重字符数
        struct CropMeta {
            is_last: bool,
            overlap_chars: usize,
        }
        let mut crops: Vec<image::DynamicImage> = Vec::new();
        let mut meta: Vec<CropMeta> = Vec::new();
        // 每行：切块在 crops 中的 (起始下标, 个数, 框中心 x, 框中心 y)
        let mut line_spans: Vec<(usize, usize, f32, f32)> = Vec::new();

        for (bi, box_item) in boxes.iter().enumerate() {
            // 大图可能有几百个框，切块循环也要能响应取消（每 64 框查一次）
            if bi % 64 == 0 && crate::services::cancel::is_cancelled() {
                return Err(anyhow::anyhow!("OCR 已取消"));
            }
            let bx0 = box_item.points[0].0.max(0.0) as u32;
            let by0 = box_item.points[0].1.max(0.0) as u32;
            let bx1 = box_item.points[2].0.min(rgb.width() as f32) as u32;
            let by1 = box_item.points[2].1.min(rgb.height() as f32) as u32;
            if bx1 <= bx0 || by1 <= by0 || (bx1 - bx0) < 4 || (by1 - by0) < 4 {
                continue;
            }
            let bw = bx1 - bx0;
            let bh = by1 - by0;
            let box_ratio = bw as f32 / bh as f32;

            // 超长行（宽高比 > max_wh_ratio）横向分块：每块宽 <= bh*max_wh_ratio，
            // 单独识别后拼接，避免整行被压扁成乱码（rec 模型仅接受 <= max_wh_ratio:1 输入）。
            let chunk_w = ((bh as f32 * self.max_wh_ratio).max(1.0)) as u32;
            let n_chunks = if box_ratio > self.max_wh_ratio {
                (bw + chunk_w - 1) / chunk_w
            } else {
                1
            };
            let overlap = ((chunk_w as f32 * 0.15) as u32).max(1);
            // overlap 约对应 overlap/bh 个字符；非末块从末尾丢弃这些字符，避免与下块重复
            let overlap_chars = (((overlap as f32) / (bh as f32)).round() as usize).clamp(0, 6);

            let line_start = crops.len();
            for ci in 0..n_chunks {
                let cx0 = if ci == 0 { 0 } else { (ci * chunk_w).saturating_sub(overlap) };
                let cx1 = ((ci + 1) * chunk_w).min(bw);
                if cx1 <= cx0 {
                    continue;
                }
                let cropped =
                    image::imageops::crop_imm(&rgb, bx0 + cx0, by0, cx1 - cx0, bh).to_image();
                crops.push(image::DynamicImage::ImageRgb8(cropped));
                meta.push(CropMeta {
                    is_last: ci + 1 >= n_chunks,
                    overlap_chars,
                });
            }
            let count = crops.len() - line_start;
            if count > 0 {
                line_spans.push((line_start, count, box_item.cx, box_item.cy));
            }
        }

        if crops.is_empty() {
            return Ok(Vec::new());
        }

        // 并行池识别：取走整池 rec 会话与切块，分发给 OS 线程并发推理，
        // 结果按全局顺序拼接（与单线程逐块顺序一致，复用下方行拼接逻辑）。
        let pool = std::mem::take(&mut self.rec_sessions);
        let dict = self.dict.clone();
        let rec_h = self.rec_input_height;
        let ratio = self.max_wh_ratio;
        let (chunk_results, returned) = match parallel_recognize(crops, pool, rec_h, ratio, &dict) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("[services::ocr] 并行识别失败: {e:#}，降级为系统 OCR 兜底");
                return Ok(Vec::new());
            }
        };
        self.rec_sessions = returned; // 归还会话池以便下次复用

        let mut results = Vec::new();
        for (start, count, cx, cy) in line_spans {
            let mut full_text = String::new();
            let mut full_score = 0.0f32;
            let mut full_chars = 0usize;
            for k in 0..count {
                let m = &meta[start + k];
                let (mut txt, sc) = chunk_results[start + k].clone();
                // 非末块：丢弃与下块重叠的尾部字符，避免重复
                if !m.is_last && m.overlap_chars > 0 && txt.chars().count() > m.overlap_chars {
                    let keep = txt.chars().count() - m.overlap_chars;
                    txt = txt.chars().take(keep).collect();
                }
                if !txt.is_empty() {
                    full_text.push_str(&txt);
                    full_score += sc;
                    full_chars += 1;
                }
            }
            if !full_text.is_empty() {
                let avg_score = if full_chars > 0 { full_score / full_chars as f32 } else { 0.0 };
                results.push(TextLine {
                    text: full_text,
                    score: avg_score,
                    x: cx as i32,
                    y: cy as i32,
                });
            }
        }

        Ok(results)
    }


}

// ============================================================
// 并行识别自由函数（跨 OS 线程复用独立 Session）
// 设计：ort::Session 不可 Clone 且 run 需 &mut self，故每条 worker 线程持一个独立会话，
// 拥有其所有权、运行、再归还。会话池在 init_sessions 时一次性建好（intra_threads=1），
// 每次识别取走全部会话并发推理后归还，避免反复加载模型。
// ============================================================

/// rec 并行池大小：逻辑核心数，封顶 8（tiny 模型超过 8 线程收益递减）。
fn rec_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

/// det 单次推理的 intra 线程数：逻辑核心数，封顶 8。
fn det_intra() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

/// CTC 贪婪解码（输出 [batch, T, C]），dict 为 PP-OCR 字典。
fn decode_ctc_text(data: &[f32], t: usize, c: usize, dict: &[String]) -> (String, f32) {
    let mut text = String::new();
    let mut prev_idx: Option<usize> = None;
    let mut total_score = 0.0f32;
    let mut char_count = 0usize;
    for step in 0..t {
        let mut max_idx = 0usize;
        let mut max_val = f32::MIN;
        for j in 0..c {
            let val = data[step * c + j];
            if val > max_val {
                max_val = val;
                max_idx = j;
            }
        }
        if max_idx != 0 && Some(max_idx) != prev_idx {
            if max_idx <= dict.len() {
                text.push_str(&dict[max_idx - 1]);
                total_score += max_val;
                char_count += 1;
            } else if max_idx == dict.len() + 1 {
                text.push(' ');
                total_score += max_val;
                char_count += 1;
            }
        }
        prev_idx = Some(max_idx);
    }
    let avg = if char_count > 0 {
        total_score / char_count as f32
    } else {
        0.0
    };
    (text, avg)
}

/// 对一批裁剪图（同一分片）做一次批量 ORT 推理，再逐块 CTC 解码。
/// 归属单个会话（持有 &mut），可在独立 OS 线程中调用，实现并发识别。
/// 返回 (逐块结果(按输入顺序), 会话) —— 会话归还以便下一次识别复用。
fn recognize_line_batch_owned(
    crops: Vec<image::DynamicImage>,
    mut session: ort::session::Session,
    rec_h: u32,
    max_wh_ratio: f32,
    dict: Arc<Vec<String>>,
) -> (Vec<(String, f32)>, ort::session::Session) {
    if crops.is_empty() {
        return (Vec::new(), session);
    }
    let h = rec_h as usize;
    let n = crops.len();

    // 1. 逐块按真实宽高比 resize 到 (new_w, rec_h)，并记录宽度、取最大值作为统一宽度
    let mut resized_imgs: Vec<image::RgbImage> = Vec::with_capacity(n);
    let mut widths: Vec<usize> = Vec::with_capacity(n);
    let mut max_w = 0usize;
    for crop in &crops {
        let scale = rec_h as f32 / crop.height() as f32;
        let mut w = (crop.width() as f32 * scale) as u32;
        let max_w_limit = (rec_h as f32 * max_wh_ratio) as u32;
        if w > max_w_limit {
            w = max_w_limit;
        }
        if w < rec_h {
            w = rec_h;
        }
        let w = w as usize;
        let r = crop
            .resize_exact(w as u32, rec_h, image::imageops::FilterType::Triangle)
            .to_rgb8();
        widths.push(w);
        if w > max_w {
            max_w = w;
        }
        resized_imgs.push(r);
    }
    if max_w < 1 {
        max_w = 1;
    }

    // 2. 构造 [N,3,H,max_w]，每块右侧补白边（CHW=1.0=白底）对齐到统一宽度
    let mut chw: Vec<f32> = vec![1.0; n * 3 * h * max_w];
    for (i, r) in resized_imgs.iter().enumerate() {
        let w = widths[i];
        for cc in 0..3 {
            for y in 0..h {
                for x in 0..w {
                    let p = r.get_pixel(x as u32, y as u32);
                    chw[((i * 3 + cc) * h + y) * max_w + x] = (p[cc] as f32 / 255.0 - 0.5) / 0.5;
                }
            }
        }
    }
    let input = match ndarray::Array4::<f32>::from_shape_vec((n, 3, h, max_w), chw) {
        Ok(v) => v,
        Err(_) => return (vec![(String::new(), 0.0); n], session),
    };

    // 3. 单次批量推理（Session::run 需 &mut self，返回的 SessionOutputs 借用 session）
    let tensor = match ort::value::TensorRef::from_array_view(&input) {
        Ok(t) => t,
        Err(_) => return (vec![(String::new(), 0.0); n], session),
    };
    // 关键：把 run 的产出限制在块作用域内。SessionOutputs<'_> 持有对 session 的借用，
    // 必须先析构才能把 session move 出去（否则 E0505: cannot move out of `session`）。
    // 只把自有数据（data.to_vec()）带出块外。
    let found: Option<(Vec<f32>, usize, usize)> = {
        match session.run(ort::inputs![tensor]) {
            Ok(outputs) => {
                let mut f: Option<(Vec<f32>, usize, usize)> = None;
                for (_name, output) in &outputs {
                    let (shape, data) = match output.try_extract_tensor::<f32>() {
                        Ok(v) => v,
                        Err(_) => continue,
                    };
                    if shape.len() == 3 {
                        f = Some((data.to_vec(), shape[1] as usize, shape[2] as usize));
                        break;
                    }
                }
                f
            }
            Err(_) => None,
        }
    }; // outputs 在此析构，session 的借用随之结束
    let (data, t, c) = match found {
        Some((d, t, c)) => (d, t, c),
        None => return (vec![(String::new(), 0.0); n], session),
    };

    // 4. 逐块解码
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let start = i * t * c;
        out.push(decode_ctc_text(&data[start..start + t * c], t, c, &dict[..]));
    }
    (out, session)
}

/// 并行识别：把全部切块按 worker 数顺序切片，每片交给持独立 rec 会话的 OS 线程并发推理。
/// 返回 (按全局顺序拼接的结果, 归还的会话池)。会话池归还以便复用，避免每次 OCR 重新加载模型。
/// 注意：worker 数取 min(池大小, 切块数)，未配对的会话原样归还，避免池被逐步耗尽。
fn parallel_recognize(
    crops: Vec<image::DynamicImage>,
    pool: Vec<ort::session::Session>,
    rec_h: u32,
    max_wh_ratio: f32,
    dict: &Arc<Vec<String>>,
) -> Result<(Vec<(String, f32)>, Vec<ort::session::Session>)> {
    if crops.is_empty() {
        return Ok((Vec::new(), pool));
    }
    // 切块数可能少于池大小（小图），worker 数封顶到切块数，避免丢弃未使用的会话
    let n_workers = pool.len().min(crops.len());
    let chunk_size = (crops.len() + n_workers - 1) / n_workers;

    // 顺序切片（owned，避免克隆整图切块），保持全局顺序
    // 先记总数：into_iter() 会 move 掉 crops，之后不能再读 crops.len()（否则 E0382）
    let total_crops = crops.len();
    let mut chunks: Vec<Vec<image::DynamicImage>> = Vec::with_capacity(n_workers);
    let mut it = crops.into_iter();
    for _ in 0..n_workers {
        let mut c = Vec::with_capacity(chunk_size);
        for _ in 0..chunk_size {
            match it.next() {
                Some(x) => c.push(x),
                None => break,
            }
        }
        if c.is_empty() {
            break;
        }
        chunks.push(c);
    }
    let actual = chunks.len();

    // 前 actual 个会话交给线程；剩余会话原样归还（不被丢弃）
    let mut pool_iter = pool.into_iter();
    let used_sessions: Vec<ort::session::Session> = (0..actual)
        .map(|_| pool_iter.next().unwrap())
        .collect();
    let unused_sessions: Vec<ort::session::Session> = pool_iter.collect();

    let mut handles = Vec::with_capacity(actual);
    for (chunk, session) in chunks.into_iter().zip(used_sessions.into_iter()) {
        let dict = dict.clone();
        handles.push(std::thread::spawn(move || {
            recognize_line_batch_owned(chunk, session, rec_h, max_wh_ratio, dict)
        }));
    }

    let mut out = Vec::with_capacity(total_crops);
    let mut returned: Vec<ort::session::Session> =
        Vec::with_capacity(handles.len() + unused_sessions.len());
    let mut panicked = false;
    for h in handles {
        match h.join() {
            Ok((res, session)) => {
                out.extend(res);
                returned.push(session);
            }
            Err(_) => panicked = true,
        }
    }
    returned.extend(unused_sessions); // 归还未使用的会话，保持池规模
    if panicked {
        // 部分会话随 panic 线程丢失；返回 Err，由调用方降级为系统 OCR 兜底（下次重建池）
        return Err(anyhow::anyhow!("[services::ocr] 识别并行线程 panic"));
    }
    Ok((out, returned))
}

/// 创建 ONNX 会话（指定 intra 线程数）
fn create_session_with_intra(path: &Path, intra: usize) -> Result<ort::session::Session> {
    use ort::session::builder::GraphOptimizationLevel;

    let session = ort::session::Session::builder()
        .map_err(|e| anyhow::anyhow!("创建 SessionBuilder 失败: {e}"))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("设置优化级别失败: {e}"))?
        .with_intra_threads(intra)
        .map_err(|e| anyhow::anyhow!("设置 intra_threads 失败: {e}"))?
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("设置 inter_threads 失败: {e}"))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("加载模型失败: {path:?} - {e}"))?;

    log::info!(
        "[services::ocr] 模型加载成功: {:?} (输入 {}, 输出 {})",
        path.file_name().unwrap_or_default(),
        session.inputs().len(),
        session.outputs().len()
    );
    Ok(session)
}

// ============================================================
// 数据结构
// ============================================================

struct ProcessedImage {
    width: i32,
    height: i32,
    data: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct RawPixel {
    x: i32,
    y: i32,
    score: f32,
}

#[derive(Debug, Clone)]
struct DetectedBox {
    points: [(f32, f32); 4],
    #[allow(dead_code)] // v1.1: 检测框质量可视化
    score: f32,
    cx: f32,
    cy: f32,
}

#[derive(Debug, Clone)]
struct TextLine {
    text: String,
    score: f32,
    x: i32,
    y: i32,
}

// ============================================================
// 连通域合并（移植自 rust-reference/ocr.rs）
// ============================================================

fn merge_connected_regions(pixels: &[RawPixel]) -> Vec<DetectedBox> {
    if pixels.is_empty() {
        return Vec::new();
    }

    // O(n) 预建 HashMap，替代 BFS 内的 O(n) find — 总复杂度从 O(n²) 降到 O(n)
    let mut score_map: HashMap<(i32, i32), f32> = HashMap::with_capacity(pixels.len());
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for p in pixels {
        score_map.insert((p.x, p.y), p.score);
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
    }

    let w = (max_x - min_x + 1) as usize;
    let h = (max_y - min_y + 1) as usize;
    if w == 0 || h == 0 {
        return Vec::new();
    }

    let mut grid = vec![false; w * h];
    for p in pixels {
        let dx = (p.x - min_x) as usize;
        let dy = (p.y - min_y) as usize;
        grid[dy * w + dx] = true;
    }

    let mut visited = vec![false; w * h];
    let mut boxes = Vec::new();
    let mut diag: Vec<(usize, f32, f32)> = Vec::new();
    let directions: [(i32, i32); 8] = [
        (-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1),
    ];

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if grid[idx] && !visited[idx] {
                let mut queue = VecDeque::new();
                queue.push_back((x as i32, y as i32));
                visited[idx] = true;

                let mut region_pixels = Vec::new();
                let mut sum_score = 0.0f32;

                while let Some((cx, cy)) = queue.pop_front() {
                    let px = cx + min_x;
                    let py = cy + min_y;
                    let score = *score_map.get(&(px, py)).unwrap_or(&0.5);
                    sum_score += score;
                    region_pixels.push((cx, cy));

                    for &(dx, dy) in &directions {
                        let nx = cx + dx;
                        let ny = cy + dy;
                        if nx >= 0 && nx < w as i32 && ny >= 0 && ny < h as i32 {
                            let nidx = (ny as usize) * w + (nx as usize);
                            if grid[nidx] && !visited[nidx] {
                                visited[nidx] = true;
                                queue.push_back((nx, ny));
                            }
                        }
                    }
                }

                if region_pixels.len() < 10 {
                    continue;
                }

                let pmin_x = region_pixels.iter().map(|p| p.0).min().unwrap_or(0);
                let pmax_x = region_pixels.iter().map(|p| p.0).max().unwrap_or(0);
                let pmin_y = region_pixels.iter().map(|p| p.1).min().unwrap_or(0);
                let pmax_y = region_pixels.iter().map(|p| p.1).max().unwrap_or(0);

                let box_w = (pmax_x - pmin_x + 1) as f32;
                let box_h = (pmax_y - pmin_y + 1) as f32;
                diag.push((region_pixels.len(), box_w, box_h));
                if box_w < 4.0 || box_h < 4.0 {
                    continue;
                }
                let wh_ratio = box_w / box_h;
                // 注意：整行中文常是 100+:1 的超宽框（如 1017x8），
                // 上限若过低会把整行滤掉导致漏识别；这里只挡竖条状噪声（<0.05），
                // 真正的超长行在 recognize_cropped_fast 里按 max_wh_ratio 横向分块处理。
                if wh_ratio < 0.05 {
                    continue;
                }

                let cx = (pmin_x + pmax_x) as f32 / 2.0 + min_x as f32;
                let cy = (pmin_y + pmax_y) as f32 / 2.0 + min_y as f32;
                let avg_score = sum_score / region_pixels.len() as f32;

                boxes.push(DetectedBox {
                    points: [
                        ((pmin_x + min_x as i32) as f32, (pmin_y + min_y as i32) as f32),
                        ((pmax_x + min_x as i32) as f32, (pmin_y + min_y as i32) as f32),
                        ((pmax_x + min_x as i32) as f32, (pmax_y + min_y as i32) as f32),
                        ((pmin_x + min_x as i32) as f32, (pmax_y + min_y as i32) as f32),
                    ],
                    score: avg_score,
                    cx,
                    cy,
                });
            }
        }
    }

    // 诊断：连通域尺寸分布（前 8 大）
    diag.sort_by(|a, b| b.0.cmp(&a.0));
    let top: Vec<String> = diag
        .iter()
        .take(8)
        .map(|(n, bw, bh)| format!("{n}px {bw:.0}x{bh:.0}(r{:.1})", bw / bh))
        .collect();
    log::info!(
        "[services::ocr][diag] 连通域 {} 个，前8大: [{}]",
        diag.len(),
        top.join(", ")
    );

    boxes
}

// ============================================================
// 透视变换裁剪（移植自 rust-reference/ocr.rs）
// ============================================================

fn crop_with_perspective(
    img: &image::DynamicImage,
    box_item: &DetectedBox,
) -> Option<image::DynamicImage> {
    let pts = &box_item.points;

    let w1 = ((pts[1].0 - pts[0].0).powi(2) + (pts[1].1 - pts[0].1).powi(2)).sqrt();
    let w2 = ((pts[2].0 - pts[3].0).powi(2) + (pts[2].1 - pts[3].1).powi(2)).sqrt();
    let h1 = ((pts[3].0 - pts[0].0).powi(2) + (pts[3].1 - pts[0].1).powi(2)).sqrt();
    let h2 = ((pts[2].0 - pts[1].0).powi(2) + (pts[2].1 - pts[1].1).powi(2)).sqrt();

    let max_w = (w1.max(w2)).ceil() as u32;
    let max_h = (h1.max(h2)).ceil() as u32;
    if max_w < 4 || max_h < 4 {
        return None;
    }
    let max_w = max_w.min(2048);
    let max_h = max_h.min(256);

    let dst_pts: [(f64, f64); 4] = [
        (0.0, 0.0),
        (max_w as f64, 0.0),
        (max_w as f64, max_h as f64),
        (0.0, max_h as f64),
    ];
    let src_pts = [
        (pts[0].0 as f64, pts[0].1 as f64),
        (pts[1].0 as f64, pts[1].1 as f64),
        (pts[2].0 as f64, pts[2].1 as f64),
        (pts[3].0 as f64, pts[3].1 as f64),
    ];
    // dst→src 的透视变换矩阵（裁剪需从输出坐标映射回源图）
    let matrix = compute_perspective_transform(&dst_pts, &src_pts)?;

    let rgb = img.to_rgb8();
    let (img_w, img_h) = (rgb.width() as i32, rgb.height() as i32);
    let mut output = image::RgbImage::new(max_w, max_h);

    for y in 0..max_h {
        for x in 0..max_w {
            let denom = matrix[6] * x as f64 + matrix[7] * y as f64 + matrix[8];
            if denom.abs() < 1e-10 {
                continue;
            }
            let src_x = (matrix[0] * x as f64 + matrix[1] * y as f64 + matrix[2]) / denom;
            let src_y = (matrix[3] * x as f64 + matrix[4] * y as f64 + matrix[5]) / denom;

            if src_x >= 0.0 && src_x < img_w as f64 && src_y >= 0.0 && src_y < img_h as f64 {
                let px = src_x as u32;
                let py = src_y as u32;
                if px < rgb.width() && py < rgb.height() {
                    output.put_pixel(x, y, *rgb.get_pixel(px, py));
                }
            }
        }
    }

    Some(image::DynamicImage::ImageRgb8(output))
}

fn compute_perspective_transform(
    src: &[(f64, f64); 4],
    dst: &[(f64, f64); 4],
) -> Option<[f64; 9]> {
    let mut a = [[0.0f64; 8]; 8];

    for i in 0..4 {
        let (sx, sy) = src[i];
        let (dx, dy) = dst[i];

        a[i * 2][0] = sx;
        a[i * 2][1] = sy;
        a[i * 2][2] = 1.0;
        a[i * 2][3] = 0.0;
        a[i * 2][4] = 0.0;
        a[i * 2][5] = 0.0;
        a[i * 2][6] = -dx * sx;
        a[i * 2][7] = -dx * sy;

        a[i * 2 + 1][0] = 0.0;
        a[i * 2 + 1][1] = 0.0;
        a[i * 2 + 1][2] = 0.0;
        a[i * 2 + 1][3] = sx;
        a[i * 2 + 1][4] = sy;
        a[i * 2 + 1][5] = 1.0;
        a[i * 2 + 1][6] = -dy * sx;
        a[i * 2 + 1][7] = -dy * sy;
    }

    let b = [dst[0].0, dst[0].1, dst[1].0, dst[1].1, dst[2].0, dst[2].1, dst[3].0, dst[3].1];

    let mut aug: Vec<Vec<f64>> = (0..8)
        .map(|i| {
            let mut row = vec![0.0f64; 9];
            for j in 0..8 {
                row[j] = a[i][j];
            }
            row[8] = b[i];
            row
        })
        .collect();

    for col in 0..8 {
        let mut max_val = aug[col][col].abs();
        let mut max_row = col;
        for row in (col + 1)..8 {
            if aug[row][col].abs() > max_val {
                max_val = aug[row][col].abs();
                max_row = row;
            }
        }
        if max_val < 1e-10 {
            return None;
        }
        if max_row != col {
            aug.swap(col, max_row);
        }
        for row in 0..8 {
            if row != col {
                let factor = aug[row][col] / aug[col][col];
                for k in col..=8 {
                    aug[row][k] -= factor * aug[col][k];
                }
            }
        }
    }

    let mut h = [0.0f64; 8];
    for i in 0..8 {
        h[i] = aug[i][8] / aug[i][i];
    }

    Some([h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], 1.0])
}

// ============================================================
// 文本行排序
// ============================================================

fn sort_text_lines(lines: &mut [TextLine]) {
    lines.sort_by(|a, b| a.y.cmp(&b.y).then_with(|| a.x.cmp(&b.x)));
}

// ============================================================
// 兜底：Windows.Media.Ocr（移植自 rust-reference/ocr.rs::recognize_windows，
// 走内存像素路径：DataWriter + SoftwareBitmap::CreateCopyFromBuffer，不经文件）
// ============================================================

fn recognize_windows_data(img: &image::DynamicImage) -> Result<String> {
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine as WinOcrEngine;
    use windows::Storage::Streams::{DataWriter, IBuffer};
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    use windows::core::HSTRING;

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }

    // 转 BGRA（SoftwareBitmap Bgra8 需要 BGRA 顺序）
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
    if w == 0 || h == 0 {
        anyhow::bail!("图片尺寸为空，无法做 Windows OCR");
    }
    let mut bgra: Vec<u8> = Vec::with_capacity(rgba.len());
    for p in rgba.chunks_exact(4) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]); // RGBA -> BGRA
    }

    // 像素 -> IBuffer -> SoftwareBitmap
    let writer = DataWriter::new()?;
    writer.WriteBytes(&bgra)?;
    let buffer: IBuffer = writer.DetachBuffer()?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w, h)?;

    // 引擎：zh-Hans → 用户系统语言
    let engine = if let Ok(lang) = Language::CreateLanguage(&HSTRING::from("zh-Hans")) {
        WinOcrEngine::TryCreateFromLanguage(&lang)
    } else {
        WinOcrEngine::TryCreateFromUserProfileLanguages()
    }
    .or_else(|_| WinOcrEngine::TryCreateFromUserProfileLanguages())?;

    let result = engine.RecognizeAsync(&bitmap)?.get()?.Text()?.to_string_lossy();
    let text = result.trim().to_string();
    if text.is_empty() {
        return Err(anyhow::anyhow!("未识别出文本（选区可能没有清晰文字）"));
    }
    Ok(text)
}

// ============================================================
// 测试：端到端 OCR（供 CLI 冒烟验证，不依赖 GUI）
// 用法：设置环境变量 WINOCR_TEST_IMG 指向图片，再 cargo test -p winocr -- --nocapture ocr_e2e
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_e2e() {
        let Ok(path) = std::env::var("WINOCR_TEST_IMG") else {
            eprintln!("[skip] 未设置 WINOCR_TEST_IMG，跳过 OCR 端到端测试");
            return;
        };
        let path = PathBuf::from(path);
        assert!(path.exists(), "测试图片不存在: {path:?}");

        // 打开 info 日志，观察 det 像素/框数等中间量
        let _ = env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("info"),
        )
        .try_init();

        // 对齐 GUI 的 recognize_capture 路径：解码一次（不计时），后续走内存直传 recognize_image，
        // 排除 PNG 落盘/读盘/解码开销，得到 GUI 框选时的纯 OCR 计算耗时。
        let data = std::fs::read(&path).expect("读取测试图片失败");
        let img = image::load_from_memory(&data).expect("图片解码失败");

        // 预热：先跑一次以建立 ort 会话（GUI 中由 prewarm 完成，此处模拟稳态框选路径）
        let _ = recognize_image(img.clone());

        let t0 = std::time::Instant::now();
        let result = recognize_image(img.clone());
        let elapsed = t0.elapsed();

        match result {
            Ok(text) => {
                println!("\n===== OCR 耗时 {:.2}s，{} 字符 =====", elapsed.as_secs_f32(), text.chars().count());
                println!("{text}");
                println!("===== END =====\n");
            }
            Err(e) => {
                println!("\n===== OCR 失败（{:.2}s）: {e:#} =====\n", elapsed.as_secs_f32());
                panic!("OCR 失败: {e:#}");
            }
        }
    }
}
