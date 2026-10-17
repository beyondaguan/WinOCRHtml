//! PP-OCRv6 ONNX 推理模块
//! 基于 ort crate v2.0.0-rc.13，支持 det/rec/cls 三模型推理
//! 包含：连通域合并、最小外接矩形、透视变换裁剪、CTC 解码、几何排序

use crate::config::{Config, OcrEngine as OcrBackend, OcrTier};
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;

/// ort 全局初始化（只需一次）
static ORT_INIT: OnceLock<()> = OnceLock::new();

fn ensure_ort_init() {
    ORT_INIT.get_or_init(|| {
        ort::init()
            .with_name("winocr-host")
            .commit();
    });
}

/// exe 所在目录：模型等资源的锚定点
/// （NM 模式下浏览器启动宿主时 cwd 不可控，不能用 cwd 相对路径）
fn exe_base_dir() -> Result<PathBuf> {
    Ok(std::env::current_exe()
        .context("获取 exe 路径失败")?
        .parent()
        .context("获取 exe 目录失败")?
        .to_path_buf())
}

pub struct OcrEngine {
    engine: OcrBackend,
    tier: OcrTier,
    src_lang: String,
    model_dir: PathBuf,
    det_session: Option<ort::session::Session>,
    rec_session: Option<ort::session::Session>,
    dict: Vec<String>,
    // 识别输入高度（标准 32px）
    rec_input_height: u32,
    // 最大宽高比（防止过长条）
    max_wh_ratio: f32,
}

impl OcrEngine {
    pub fn new(config: &Config) -> Result<Self> {
        ensure_ort_init();
        let engine = config.ocr_engine.clone();
        let tier = config.local_ocr_tier.clone();
        let src_lang = config.src_lang.clone();
        // 模型目录：优先 exe 所在目录，其次进程工作目录（兼容直接从仓库根运行/调试）
        let rel = &config.model_dir;
        let exe_dir = exe_base_dir()?;
        let model_dir = if exe_dir.join(rel).join("ppocr_dict.txt").exists() {
            exe_dir.join(rel)
        } else if let Ok(cwd) = std::env::current_dir() {
            if cwd.join(rel).join("ppocr_dict.txt").exists() {
                cwd.join(rel)
            } else {
                exe_dir.join(rel)
            }
        } else {
            exe_dir.join(rel)
        };

        // 加载字典（PP-OCRv6 rec 字典按 tier 选择：medium 字典与 tiny 不同）
        let dict_name = match tier {
            OcrTier::Medium => "ppocr_dict_medium.txt",
            _ => "ppocr_dict.txt",
        };
        let dict_path = model_dir.join(dict_name);
        let dict = if dict_path.exists() {
            let content = std::fs::read_to_string(&dict_path)
                .with_context(|| format!("读取字典失败: {:?}", dict_path))?;
            content.lines().map(|s| s.to_string()).collect()
        } else {
            log::warn!("字典不存在: {:?}，识别将输出乱码", dict_path);
            vec![" ".to_string()]
        };

        let rec_input_height = 48;
        let max_wh_ratio = match tier {
            OcrTier::Tiny => 12.0,
            OcrTier::Small => 14.0,
            OcrTier::Medium => 16.0,
        };

        Ok(Self {
            engine,
            tier,
            src_lang,
            model_dir,
            det_session: None,
            rec_session: None,
            dict,
            rec_input_height,
            max_wh_ratio,
        })
    }

    /// 识别图片中的文本
    pub fn recognize(&mut self, img_data: &[u8]) -> Result<String> {
        // 0. 结果缓存（§14.5 / P2-5）：同内容图片直接复用上次识别结果
        let ck = crate::ocr_cache::key_for(img_data);
        if let Some(cached) = crate::ocr_cache::get(&ck) {
            log::info!("OCR 缓存命中 {}（{} 字符）", ck, cached.chars().count());
            return Ok(cached);
        }

        // 0.5 Windows 原生 OCR 后端（零模型体积兜底，仅 Windows）
        #[cfg(windows)]
        if self.engine == OcrBackend::Windows {
            let result = self.recognize_windows(img_data)?;
            crate::ocr_cache::put(&ck, &result);
            return Ok(result);
        }
        #[cfg(not(windows))]
        if self.engine == OcrBackend::Windows {
            anyhow::bail!("Windows 原生 OCR 后端仅支持 Windows 平台");
        }

        // 1. 加载图片
        let img = image::load_from_memory(img_data)
            .map_err(|e| anyhow::anyhow!("图片加载失败: {}", e))?;

        // 2. 预处理（小图放大）
        let processed = self.preprocess(&img)?;

        // 3. 延迟初始化会话
        self.init_sessions()?;

        // 4. 检测（DBNet）
        let raw_boxes = self.detect(&processed)?;

        // 5. 检测后处理：连通域合并 + 最小外接矩形
        let mut merged_boxes = merge_connected_regions(&raw_boxes);
        log::info!(
            "det: {} 像素, {} 框 (预处理 {}x{} -> 原图 {}x{})",
            raw_boxes.len(),
            merged_boxes.len(),
            processed.width,
            processed.height,
            img.width(),
            img.height()
        );

        // 5.5 概率图坐标 → 原图坐标（预处理含缩放：小图放大 + 32 对齐）
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

        // 5.6 框外扩（DBNet 概率图小于实际文字区域，等效 unclip 膨胀）
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

        // 6. 透视变换裁剪 + 识别
        let mut text_lines = self.recognize_cropped(&img, &merged_boxes)?;

        // 7. 几何排序（从上到下，从左到右）
        sort_text_lines(&mut text_lines);

        // 8. 拼接结果
        let result = text_lines
            .into_iter()
            .filter(|l| l.score >= 0.5)
            .map(|l| l.text)
            .collect::<Vec<_>>()
            .join("\n");

        // 9. 写回缓存（仅非空结果）
        crate::ocr_cache::put(&ck, &result);

        Ok(result)
    }

    /// Windows 原生 OCR 后端（§18 方案 B）：零模型体积、系统级兜底，仅 Windows 可用。
    /// 通过 `windows::Media::Ocr` 识别，仅返回纯文本（无逐行坐标框）。
    /// 参考 pot-desktop `system_ocr.rs`（仅思路，未照搬代码）。
    #[cfg(windows)]
    fn recognize_windows(&self, img_data: &[u8]) -> Result<String> {
        use windows::core::HSTRING;
        use windows::Globalization::Language;
        use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
        use windows::Media::Ocr::OcrEngine as WinOcrEngine;
        use windows::Storage::Streams::{DataWriter, IBuffer};

        // 1. 解码图片，转 BGRA 像素（SoftwareBitmap Bgra8 需要 BGRA 顺序）
        let img = image::load_from_memory(img_data)
            .map_err(|e| anyhow::anyhow!("图片加载失败: {}", e))?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width() as i32, rgba.height() as i32);
        if w == 0 || h == 0 {
            anyhow::bail!("图片尺寸为空，无法做 Windows OCR");
        }
        let mut bgra: Vec<u8> = Vec::with_capacity(rgba.len());
        for p in rgba.chunks_exact(4) {
            bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]); // RGBA -> BGRA
        }

        // 2. 像素 -> IBuffer -> SoftwareBitmap（不走临时文件 / 文件 API）
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let buffer: IBuffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w, h)?;

        // 3. 创建 OCR 引擎：auto 用用户配置文件语言，否则按 src_lang 选语言包
        let engine = if self.src_lang == "auto" || self.src_lang.is_empty() {
            WinOcrEngine::TryCreateFromUserProfileLanguages()
        } else {
            let bcp47 = match self.src_lang.as_str() {
                "zh" => "zh-Hans",
                "ja" => "ja",
                "ko" => "ko",
                "ru" => "ru",
                "fr" => "fr",
                "de" => "de",
                "es" => "es",
                _ => "en",
            };
            match Language::CreateLanguage(&HSTRING::from(bcp47)) {
                Ok(lang) => WinOcrEngine::TryCreateFromLanguage(&lang),
                Err(_) => WinOcrEngine::TryCreateFromUserProfileLanguages(),
            }
        };

        let engine = engine.map_err(|e| {
            let msg = e.to_string();
            if msg.contains("0x00000000") {
                anyhow::anyhow!(
                    "Windows OCR 失败：未安装对应语言包（在「设置 → 时间和语言 → 语言」中安装语言包后重试）"
                )
            } else {
                anyhow::anyhow!("Windows OCR 引擎创建失败: {}", msg)
            }
        })?;

        // 4. 识别（仅纯文本，无逐行坐标）
        let result = engine.RecognizeAsync(&bitmap)?.get()?.Text()?.to_string_lossy();

        Ok(result.trim().to_string())
    }

    fn init_sessions(&mut self) -> Result<()> {
        if self.det_session.is_none() {
            let model_name = self.tier.model_name("det");
            let path = self.model_dir.join(model_name);
            if path.exists() {
                self.det_session = Some(create_session(&path)?);
            } else {
                log::warn!("检测模型不存在: {:?}", path);
            }
        }

        if self.rec_session.is_none() {
            let model_name = self.tier.model_name("rec");
            let path = self.model_dir.join(model_name);
            if path.exists() {
                self.rec_session = Some(create_session(&path)?);
            } else {
                log::warn!("识别模型不存在: {:?}", path);
            }
        }

        Ok(())
    }

    fn preprocess(&self, img: &image::DynamicImage) -> Result<ProcessedImage> {
        let (w, h) = (img.width() as i32, img.height() as i32);
        let mut img = if w.min(h) < 400 {
            img.resize_exact(
                (w * 2) as u32,
                (h * 2) as u32,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img.to_owned()
        };

        // det 模型要求输入宽高为 32 的倍数（Resize 节点上采样约束）
        let align32 = |v: u32| -> u32 { ((v + 31) / 32) * 32 };
        let (tw, th) = (align32(img.width()), align32(img.height()));
        if tw != img.width() || th != img.height() {
            img = img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3);
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
                log::warn!("检测模型未加载，跳过检测");
                return Ok(Vec::new());
            }
        };

        let h = img.height as usize;
        let w = img.width as usize;

        // 构建 NCHW 输入 [1, 3, H, W]，ImageNet 归一化
        let mut nchw = vec![0f32; 1 * 3 * h * w];
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
        let outputs = session.run(ort::inputs![ort::value::TensorRef::from_array_view(&input_array)?])?;

        // 解析输出 [1, 1, H, W] 概率图
        for (_name, output) in &outputs {
            let (shape, data) = output
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow::anyhow!("输出解析失败: {}", e))?;

            if shape.len() == 4 && shape[0] == 1 && shape[1] == 1 {
                let th = shape[2] as usize;
                let tw = shape[3] as usize;
                let threshold = 0.3;
                let mut pixels = Vec::new();

                for y in 0..th {
                    for x in 0..tw {
                        let idx = y * tw + x;
                        if data[idx] > threshold {
                            pixels.push(RawPixel {
                                x: x as i32,
                                y: y as i32,
                                score: data[idx],
                            });
                        }
                    }
                }

                return Ok(pixels);
            }
        }

        Ok(Vec::new())
    }

    fn recognize_cropped(
        &mut self,
        img: &image::DynamicImage,
        boxes: &[DetectedBox],
    ) -> Result<Vec<TextLine>> {
        let session = match &mut self.rec_session {
            Some(s) => s,
            None => {
                log::warn!("识别模型未加载，跳过识别");
                return Ok(Vec::new());
            }
        };

        let mut results = Vec::new();

        for box_item in boxes {
            // 透视变换裁剪
            let cropped = match crop_with_perspective(img, box_item) {
                Some(c) => c,
                None => continue,
            };

            // 计算缩放后的宽度（保持宽高比，高度固定为 rec_input_height）
            let scale = self.rec_input_height as f32 / cropped.height() as f32;
            let mut new_w = (cropped.width() as f32 * scale) as u32;

            // 限制最大宽度
            let max_w = (self.rec_input_height as f32 * self.max_wh_ratio) as u32;
            if new_w > max_w {
                new_w = max_w;
            }
            if new_w < self.rec_input_height {
                new_w = self.rec_input_height;
            }

            // Resize 到 [new_w, rec_input_height]
            let resized = cropped.resize_exact(
                new_w,
                self.rec_input_height,
                image::imageops::FilterType::Lanczos3,
            );

            // 转为 RGB 并归一化（rec 模型输入 [1, 3, 48, W]，CHW 排列）
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

            // 构建输入 [1, 3, 48, W]
            let input = ndarray::Array4::<f32>::from_shape_vec(
                (1, 3, rh, rw),
                chw,
            )
            .map_err(|e| anyhow::anyhow!("创建输入 tensor 失败: {}", e))?;

            let outputs = session.run(ort::inputs![ort::value::TensorRef::from_array_view(&input)?])?;

            for (_name, output) in &outputs {
                let (shape, data) = output
                    .try_extract_tensor::<f32>()
                    .map_err(|e| anyhow::anyhow!("输出解析失败: {}", e))?;

                if shape.len() == 3 {
                    // 输出 [batch, T, C]：T=时间步, C=类别数
                    let t = shape[1] as usize;
                    let c = shape[2] as usize;

                    // CTC 贪婪解码
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
}

/// 创建 ONNX 会话
fn create_session(path: &std::path::Path) -> Result<ort::session::Session> {
    use ort::session::builder::GraphOptimizationLevel;

    let session = ort::session::Session::builder()
        .map_err(|e| anyhow::anyhow!("创建 SessionBuilder 失败: {}", e))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| anyhow::anyhow!("设置优化级别失败: {}", e))?
        .with_intra_threads(2)
        .map_err(|e| anyhow::anyhow!("设置intra_threads 失败: {}", e))?
        .with_inter_threads(1)
        .map_err(|e| anyhow::anyhow!("设置inter_threads 失败: {}", e))?
        .commit_from_file(path)
        .map_err(|e| anyhow::anyhow!("加载模型失败: {:?} - {}", path, e))?;

    log::info!(
        "模型加载成功: {:?} (输入: {}, 输出: {})",
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

/// 原始像素（来自检测概率图）
#[derive(Debug, Clone, Copy)]
struct RawPixel {
    x: i32,
    y: i32,
    score: f32,
}

/// 检测框（最小外接矩形）
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct DetectedBox {
    points: [(f32, f32); 4],
    score: f32,
    cx: f32,
    cy: f32,
}

/// 文本行
#[derive(Debug, Clone)]
struct TextLine {
    text: String,
    score: f32,
    x: i32,
    y: i32,
}

// ============================================================
// 连通域合并
// ============================================================

/// 合并相邻像素为连通域，计算最小外接矩形
fn merge_connected_regions(pixels: &[RawPixel]) -> Vec<DetectedBox> {
    if pixels.is_empty() {
        return Vec::new();
    }

    // 找到边界
    let min_x = pixels.iter().map(|p| p.x).min().unwrap_or(0);
    let max_x = pixels.iter().map(|p| p.x).max().unwrap_or(0);
    let min_y = pixels.iter().map(|p| p.y).min().unwrap_or(0);
    let max_y = pixels.iter().map(|p| p.y).max().unwrap_or(0);

    let w = (max_x - min_x + 1) as usize;
    let h = (max_y - min_y + 1) as usize;

    if w == 0 || h == 0 {
        return Vec::new();
    }

    // 构建二值图
    let mut grid = vec![false; w * h];
    for p in pixels {
        let dx = (p.x - min_x) as usize;
        let dy = (p.y - min_y) as usize;
        grid[dy * w + dx] = true;
    }

    // BFS 连通域标记
    let mut visited = vec![false; w * h];
    let mut boxes = Vec::new();
    let directions: [(i32, i32); 8] = [
        (-1, -1), (0, -1), (1, -1),
        (-1, 0),           (1, 0),
        (-1, 1),  (0, 1),  (1, 1),
    ];

    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            if grid[idx] && !visited[idx] {
                // BFS
                let mut queue = VecDeque::new();
                queue.push_back((x as i32, y as i32));
                visited[idx] = true;

                let mut region_pixels = Vec::new();
                let mut sum_score = 0.0f32;

                while let Some((cx, cy)) = queue.pop_front() {
                    let _cidx = (cy as usize) * w + (cx as usize);
                    // 找到对应的 score
                    let px = cx + min_x;
                    let py = cy + min_y;
                    // 在原始 pixels 中查找 score（简化：用 0.5 作为默认值）
                    let score = pixels
                        .iter()
                        .find(|p| p.x == px && p.y == py)
                        .map(|p| p.score)
                        .unwrap_or(0.5);
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

                // 过滤太小的区域（噪点）
                if region_pixels.len() < 10 {
                    continue;
                }

                // 计算最小外接矩形
                let pmin_x = region_pixels.iter().map(|p| p.0).min().unwrap_or(0);
                let pmax_x = region_pixels.iter().map(|p| p.0).max().unwrap_or(0);
                let pmin_y = region_pixels.iter().map(|p| p.1).min().unwrap_or(0);
                let pmax_y = region_pixels.iter().map(|p| p.1).max().unwrap_or(0);

                let box_w = (pmax_x - pmin_x + 1) as f32;
                let box_h = (pmax_y - pmin_y + 1) as f32;

                // 过滤太细的区域（可能是线条）
                if box_w < 4.0 || box_h < 4.0 {
                    continue;
                }

                // 过滤宽高比异常的区域（正常文本行可达 20+，只拦极端线条）
                let wh_ratio = box_w / box_h;
                if wh_ratio > 50.0 || wh_ratio < 0.05 {
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

    boxes
}

// ============================================================
// 透视变换裁剪
// ============================================================

/// 从图片中裁剪四边形区域，矫正为矩形
fn crop_with_perspective(
    img: &image::DynamicImage,
    box_item: &DetectedBox,
) -> Option<image::DynamicImage> {
    let pts = &box_item.points;

    // 计算裁剪后宽高
    let w1 = ((pts[1].0 - pts[0].0).powi(2) + (pts[1].1 - pts[0].1).powi(2)).sqrt();
    let w2 = ((pts[2].0 - pts[3].0).powi(2) + (pts[2].1 - pts[3].1).powi(2)).sqrt();
    let h1 = ((pts[3].0 - pts[0].0).powi(2) + (pts[3].1 - pts[0].1).powi(2)).sqrt();
    let h2 = ((pts[2].0 - pts[1].0).powi(2) + (pts[2].1 - pts[1].1).powi(2)).sqrt();

    let max_w = (w1.max(w2)).ceil() as u32;
    let max_h = (h1.max(h2)).ceil() as u32;

    if max_w < 4 || max_h < 4 {
        return None;
    }

    // 限制最大尺寸
    let max_w = max_w.min(2048);
    let max_h = max_h.min(256);

    // 目标矩形顶点
    let dst_pts: [(f64, f64); 4] = [
        (0.0, 0.0),
        (max_w as f64, 0.0),
        (max_w as f64, max_h as f64),
        (0.0, max_h as f64),
    ];

    // 计算透视变换矩阵（3x3）
    let src_pts = [
        (pts[0].0 as f64, pts[0].1 as f64),
        (pts[1].0 as f64, pts[1].1 as f64),
        (pts[2].0 as f64, pts[2].1 as f64),
        (pts[3].0 as f64, pts[3].1 as f64),
    ];
// 计算 dst→src 的透视变换矩阵（3x3）
    // （crop 时需要从输出坐标映射回源图坐标，故 dst 在前）
    let matrix = match compute_perspective_transform(&dst_pts, &src_pts) {
        Some(m) => m,
        None => return None,
    };

    // 应用变换
    let rgb = img.to_rgb8();
    let (img_w, img_h) = (rgb.width() as i32, rgb.height() as i32);
    let mut output = image::RgbImage::new(max_w, max_h);

    for y in 0..max_h {
        for x in 0..max_w {
            // 逆变换：从目标坐标映射到源坐标
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

/// 计算透视变换矩阵（3x3，8 自由度）
fn compute_perspective_transform(
    src: &[(f64, f64); 4],
    dst: &[(f64, f64); 4],
) -> Option<[f64; 9]> {
    // 使用最小二乘法求解
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

    let b = [
        dst[0].0, dst[0].1,
        dst[1].0, dst[1].1,
        dst[2].0, dst[2].1,
        dst[3].0, dst[3].1,
    ];

    // 高斯消元
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
        // 找主元
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

        // 交换行
        if max_row != col {
            aug.swap(col, max_row);
        }

        // 消元
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

/// 按几何位置排序：先按 y 坐标分组行，再按 x 坐标排序
fn sort_text_lines(lines: &mut [TextLine]) {
    if lines.is_empty() {
        return;
    }

    // 先按 y 坐标排序（主要关键字），再按 x 坐标排序（次要关键字）
    lines.sort_by(|a, b| {
        a.y
            .cmp(&b.y)
            .then_with(|| a.x.cmp(&b.x))
    });
}
