// 离屏布局量测探针（不进 winocr 依赖图）
//
// 目的：`.slint` 语法通过 ≠ 布局正确。「穿模 / 整栏塌 0 宽 / 某栏不显示」
//       这类问题只有拿到**真实像素**才能定位。本工具用软件渲染器离屏出 PNG，
//       避免靠读代码推理布局（Slint 的 stretch / preferred-width 规则很反直觉）。
//
// 用法：
//   cargo run --manifest-path tools/slint-probe/Cargo.toml -- -- \
//       app/src/ui/main_window.slint .workbuddy/outputs/probe.png 1080 600
//
// 环境变量（复现不同状态）：
//   PROBE_SPLIT_VERTICAL=1   split-vertical=true → 上下对照
//   PROBE_WITH_TEXT=1        ocr-out 填样例（有译文态）
//   PROBE_CANDIDATES=N       造 N 个候选词（复现候选词面板溢出）
//   PROBE_DRAWER=collect     打开指定抽屉
use std::path::PathBuf;
use std::rc::Rc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::{LogicalSize, WindowSize};

slint::include_modules!();

fn env_bool(k: &str) -> bool {
    std::env::var(k).map(|v| v != "0" && !v.is_empty()).unwrap_or(false)
}

fn env_num(k: &str) -> Option<usize> {
    std::env::var(k).ok().and_then(|v| v.parse().ok())
}

/// 只提供软件渲染、不接管事件循环的平台。
struct OffscreenPlatform {
    window: Rc<MinimalSoftwareWindow>,
}

impl slint::platform::Platform for OffscreenPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::platform::PlatformError> {
        Ok(self.window.clone())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let slint_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "app/src/ui/main_window.slint".to_string());
    let out = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| ".workbuddy/outputs/probe.png".to_string());
    let w: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1080);
    let h: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(600);

    let msw = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(OffscreenPlatform {
        window: msw.clone(),
    }))
    .expect("set_platform");
    msw.set_size(WindowSize::Logical(LogicalSize::new(w as f32, h as f32)));

    let window = MainWindow::new().expect("MainWindow::new");
    // ⚠️ 关键：必须把尺寸设到**组件自己的窗口**上。只设 msw 不会改变组件根的
    // width/height（那是 slint 里写死的 1130x640），会出现「窗口变了但布局没变」的伪影。
    window.window().set_size(WindowSize::Logical(LogicalSize::new(
        w as f32, h as f32,
    )));
    // PROBE_BARE=1 时跳过所有属性设置，用于渲染最小复现组件（验证纯几何行为）
    let bare = env_bool("PROBE_BARE");
    if !bare {
    window.set_ocr_src(
        "SHANGHAI. Oct.5 (Xinhua) -- With one month to go before the 9th China International \
         Import Expo (CIIE), companies from around the world are preparing to gather in Shanghai \
         in east China, bringing products ranging from advanced manufacturing equipment and \
         medical technologies to African coffee and Syrian soap."
            .into(),
    );
    if env_bool("PROBE_WITH_TEXT") {
        window.set_ocr_out(
            "With one month to go before the 9th China Industrial Report Expo. Companies from \
             around the world are e preparing to carry our Shanghal in East China."
                .into(),
        );
    }
    if env_bool("PROBE_SPLIT_VERTICAL") {
        window.set_split_vertical(true);
    }
    if let Some(n) = env_num("PROBE_CANDIDATES") {
        let words: Vec<slint::SharedString> = (0..n)
            .map(|i| slint::SharedString::from(format!("word{i}")))
            .collect();
        window.set_collect_candidates(slint::ModelRc::new(slint::VecModel::from(words.clone())));
        // 与 app 侧 layout_candidate_rows 同构的分行逻辑（探针不依赖 winocr crate）
        let avail = (w as f32 - 16.0 - 8.0).max(80.0);
        let (cw, spacing, hp) = (7.6f32, 6.0f32, 20.0f32);
        let mut rows: Vec<Vec<slint::SharedString>> = Vec::new();
        let mut cur: Vec<slint::SharedString> = Vec::new();
        let mut cur_w = 0.0f32;
        for word in words.iter() {
            let ww = hp + word.len() as f32 * cw;
            let need = if cur.is_empty() { ww } else { ww + spacing };
            if !cur.is_empty() && cur_w + need > avail {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0.0;
                if rows.len() >= 4 {
                    break;
                }
            }
            cur_w += if cur.is_empty() { ww } else { ww + spacing };
            cur.push(word.clone());
        }
        if !cur.is_empty() && rows.len() < 4 {
            rows.push(cur);
        }
        let shown: usize = rows.iter().map(|r| r.len()).sum();
        let hidden = n.saturating_sub(shown);
        let row_models: Vec<slint::ModelRc<slint::SharedString>> = rows
            .into_iter()
            .map(|r| slint::ModelRc::new(slint::VecModel::from(r)))
            .collect();
        window.set_collect_candidate_rows(slint::ModelRc::new(slint::VecModel::from(row_models)));
        window.set_collect_candidate_overflow(if hidden > 0 {
            format!("另有 {hidden} 个…").into()
        } else {
            "".into()
        });
    }
    if let Ok(d) = std::env::var("PROBE_DRAWER") {
        if !d.is_empty() {
            window.set_drawer_type(d.as_str().into());
            window.set_drawer_visible(true);
        }
    }
    // PROBE_AI_MESSAGES=N：PROBE_DRAWER=ai 时造 N 条交替 user/assistant 消息（验证气泡配色与复制按钮）
    if let Ok(d) = std::env::var("PROBE_DRAWER") {
        if d == "ai" {
            if let Some(n) = env_num("PROBE_AI_MESSAGES") {
                let msgs: Vec<slint::SharedString> = (0..n)
                    .map(|i| {
                        if i % 2 == 0 {
                            slint::SharedString::from(format!("用户问题 {i}：什么是 WinOCR？"))
                        } else {
                            slint::SharedString::from(format!(
                                "助手回答 {i}：WinOCR 是一款 Windows 原生 OCR + 翻译工具，支持划词翻译、截图识别与生词本管理。\n第二行内容用于验证换行。"
                            ))
                        }
                    })
                    .collect();
                let roles: Vec<slint::SharedString> = (0..n)
                    .map(|i| slint::SharedString::from(if i % 2 == 0 { "user" } else { "assistant" }))
                    .collect();
                window.set_ai_messages(slint::ModelRc::new(slint::VecModel::from(msgs.clone())));
                window.set_ai_msg_roles(slint::ModelRc::new(slint::VecModel::from(roles.clone())));
            } else {
                // 默认造 4 条
                let msgs: Vec<slint::SharedString> = vec![
                    "用户：你好".into(),
                    "助手：你好！我是 WinOCR AI 助手，有什么可以帮你？".into(),
                    "用户：帮我翻译这段话".into(),
                    "助手：请把要翻译的内容粘贴到原文栏，或直接截图识别。".into(),
                ];
                let roles: Vec<slint::SharedString> = vec![
                    "user".into(),
                    "assistant".into(),
                    "user".into(),
                    "assistant".into(),
                ];
                window.set_ai_messages(slint::ModelRc::new(slint::VecModel::from(msgs.clone())));
                window.set_ai_msg_roles(slint::ModelRc::new(slint::VecModel::from(roles.clone())));
            }
        }
    }
    // PROBE_UI_SCALE / PROBE_UI_FONT：复现设置页的界面缩放与字体切换效果
    if let Some(p) = env_num("PROBE_UI_SCALE") {
        window.set_ui_scale_pct(p as i32);
    }
    if let Ok(m) = std::env::var("PROBE_MODAL") {
        if !m.is_empty() {
            window.set_modal_type(m.as_str().into());
            window.set_modal_visible(true);
        }
    }
    if let Ok(f) = std::env::var("PROBE_UI_FONT") {
        window.set_ui_font_family(f.as_str().into());
    }
    } // end !bare

    window.show();

    // 多跑几轮，确保条件元素与布局稳定
    for _ in 0..3 {
        window.window().request_redraw();
        msw.draw_if_needed(|renderer| {
            // 渲染缓冲：用软件渲染器导出的 PremultipliedRgbaColor（既是 TargetPixel，
// 又带 red()/green()/blue() 访问器），渲染完直接拼成 RGB8 存 PNG。
let n = (w as usize) * (h as usize);
let mut buffer = vec![PremultipliedRgbaColor::from_rgb(0, 0, 0); n];
renderer.render(buffer.as_mut_slice(), w as usize);
let rgb: Vec<u8> = buffer
    .iter()
    .flat_map(|p| [p.red, p.green, p.blue])
    .collect();
let img: image::RgbImage = image::RgbImage::from_raw(w, h, rgb).expect("from_raw");
if let Some(parent) = PathBuf::from(&out).parent() {
    let _ = std::fs::create_dir_all(parent);
}
img.save(&out).expect("save png");
        });
    }
    // PROBE_CLICK —— 必须在渲染循环**之后**派发：命中测试依赖已完成的布局，
    // 紧随 show() 派发会因布局未就绪而收不到事件（表现为「点了没反应」的伪影）。
    if let Ok(spec) = std::env::var("PROBE_CLICK") {
        let nums: Vec<f32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if nums.len() == 2 {
            let (cx, cy) = (nums[0], nums[1]);
            let pos = slint::LogicalPosition::new(cx, cy);
            let btn = slint::platform::PointerEventButton::Left;
            let core_win = msw.as_ref();
            core_win.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: pos });
            core_win.dispatch_event(slint::platform::WindowEvent::PointerPressed { position: pos, button: btn });
            core_win.dispatch_event(slint::platform::WindowEvent::PointerReleased { position: pos, button: btn });
            // ⚠️ TouchArea.clicked 是 **queued** 回调（排进事件循环），派发完必须跑一轮
            // 事件循环才会执行；否则状态永远不变，表现为「点了没反应」的探针伪影。
            slint::run_event_loop_until_quit();
            println!(
                "[slint-probe] 已点击 ({cx},{cy}) → drawer_visible={} drawer_type={:?}",
                window.get_drawer_visible(),
                window.get_drawer_type()
            );
        }
    }

    println!(
        "[slint-probe] 已渲染 {out}  {w}x{h}  split_vertical={}  ocr_src_len={}  slint={slint_path}",
        window.get_split_vertical(),
        window.get_ocr_src().len()
    );
}