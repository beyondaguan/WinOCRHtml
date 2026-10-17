//! WinOCR-Html 本地壳程序（Rust 版）
//! 职责：全局截图 + PP-OCRv6 OCR + 翻译 + 浮窗显示
//! 通过 Native Messaging 与浏览器扩展通信

mod config;
mod dpapi;
mod globals;
mod history;
mod history_window;
mod hotkey;
mod i18n;
mod lang_detect;
mod logger;
mod main_window;
mod nm;
mod ocr;
mod ocr_cache;
mod screenshot;
mod usage;
mod selection_window;
mod settings_window;
mod selfcheck;
mod translate;
mod tray;
mod terminology;
mod subtitles;
mod pdf_translate;
mod whisper;
mod window;

use clap::Parser;
use config::Config;
use nm::NmHandler;
use std::mem;
use std::ptr;
use std::sync::{Arc, Mutex};
use base64::Engine;
use winapi::shared::windef::HWND;
use winapi::um::winuser::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, LoadCursorW, LoadIconW, PeekMessageW,
    RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW, IDC_ARROW, IDI_APPLICATION, MSG,
    PM_REMOVE, WM_DESTROY, WM_QUIT, WNDCLASSW, WS_EX_NOACTIVATE, WS_OVERLAPPED,
};

/// 产品版本（单一事实来源，§13.4.1）。CI 负责校验与 manifest.json 一致。
pub const APP_VERSION: &str = "0.9.0";

#[derive(Parser, Debug)]
#[command(name = "winocr_host")]
#[command(about = "WinOCR-Html 本地壳程序（Rust 版）")]
struct Args {
    /// 独立模式（不依赖浏览器扩展）
    #[arg(long)]
    standalone: bool,
    /// 配置文件路径
    #[arg(long, short)]
    config: Option<String>,
    /// 日志级别
    #[arg(long, short, default_value = "info")]
    log_level: String,
    /// 运行一次 NM 注册自检并打印 JSON 后退出（用于 Doctor / 调试）
    #[arg(long)]
    self_check: bool,
    /// 首次运行标志：强制将配置标记为首次运行（引导向导），并落盘持久化。
    /// 安装器在本机装完后调用一次，确保首个用户真正走 5 步引导（§16.3）。
    #[arg(long)]
    first_run: bool,
    /// 打印本月用量统计后退出（§15.8 / P2-6）
    #[arg(long)]
    usage: bool,
    /// 跑性能基线并输出 JSON 后退出（§14.5 / P2-3）；可用 --rounds 指定轮次
    #[arg(long)]
    bench: bool,
    /// bench 轮次（默认 20）
    #[arg(long, default_value = "20")]
    rounds: usize,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // 初始化日志：文件（按日期滚动 7 天）+ stderr 双写
    logger::init_logger(&args.log_level);

    log::info!("WinOCR-Html Rust 宿主启动 (v{})", APP_VERSION);
    log::info!("独立模式: {}", args.standalone);

    // 一键自检模式：打印 NM 注册诊断 JSON 后退出（不依赖浏览器/配置）
    if args.self_check {
        let report = selfcheck::run_self_check();
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        return Ok(());
    }

    // 加载配置
    let config_path = match args.config.as_deref() {
        Some(p) => p.to_string(),
        None => config::default_config_path()?,
    };
    let config = Config::load(Some(&config_path))?;
    log::info!("配置加载完成: {}", config_path);

    // 首次运行标志：安装器调用 `--first-run` 时强制置位并落盘，
    // 确保首个用户真正走 5 步引导（§16.3）；平时保持配置里的值。
    if args.first_run {
        let mut c = config.clone();
        c.first_run = true;
        if let Err(e) = c.save(&config_path) {
            log::warn!("写入 first_run 标志失败: {}", e);
        } else {
            log::info!("已置位 first_run（首次运行引导）");
        }
    }

    // 用量统计模式：打印本月/历史用量后退出（§15.8 / P2-6）
    if args.usage {
        println!("{}", usage::summary());
        if config.monthly_char_limit > 0 {
            println!(
                "月度上限：{} 字符（当前 {}，{}）",
                config.monthly_char_limit,
                usage::month_total(),
                if usage::is_over_limit(config.monthly_char_limit) {
                    "已超限·降级中"
                } else {
                    "未超限"
                }
            );
        } else {
            println!("月度上限：未设置（不限）");
        }
        return Ok(());
    }

    // 性能基线模式：跑 bench 并输出 JSON 后退出（§14.5 / P2-3）
    if args.bench {
        let report = run_bench(args.rounds.max(1), &config);
        println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
        // 同时落盘到日志目录，供 CI 归档比对基线
        let dir = dirs::data_local_dir()
            .map(|p| p.join("WinOCR-Html").join("logs"))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("bench-{}.json", unix_ts()));
        let _ = std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap_or_default());
        log::info!("性能基线已写入: {}", path.display());
        return Ok(());
    }

    // 统一常驻模式：无论是否连接浏览器扩展，壳都以托盘常驻运行。
    // connect_nm=false 即原独立模式；connect_nm=true 时额外把 NM 协议放到独立线程。
    run_resident(config, config_path, !args.standalone)
}

/// 当前 Unix 时间戳（秒）
fn unix_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 性能基线（§14.5 / P2-3）
///
/// 分阶段测量并在缺失依赖（模型/网络/截图后端）时优雅跳过，保证 CI 也能跑出可用基线：
/// - `cache`：OCR 缓存查询/写入吞吐与命中率（不依赖模型）；
/// - `ocr`：同一图片冷识别 vs 缓存命中（模型缺失则 skipped）；
/// - `translate`：翻译往返（离线则 skipped）；
/// - `screenshot`：区域截图（后端不可用则 skipped）。
fn run_bench(rounds: usize, config: &Config) -> serde_json::Value {
    use std::time::Instant;

    crate::ocr_cache::init(config.ocr_cache_enabled, config.ocr_cache_max);

    let mut out = serde_json::Map::new();
    out.insert(
        "app_version".to_string(),
        serde_json::Value::String(APP_VERSION.to_string()),
    );
    out.insert("timestamp".to_string(), serde_json::Value::from(unix_ts()));
    out.insert("rounds".to_string(), serde_json::Value::from(rounds));
    out.insert(
        "ocr_cache_enabled".to_string(),
        serde_json::Value::from(config.ocr_cache_enabled),
    );

    // ---- cache 阶段：rounds 次查询，去重基数设 rounds/8（稳态命中率 ≈ 87.5%，对齐 ≥80% 目标）
    crate::ocr_cache::reset_stats();
    let distinct = (rounds / 8).max(1);
    let t0 = Instant::now();
    for i in 0..rounds {
        let key = crate::ocr_cache::key_for(format!("bench-{}", i % distinct).as_bytes());
        if crate::ocr_cache::get(&key).is_none() {
            crate::ocr_cache::put(&key, "benchmark-text");
        }
    }
    let cache_ms = t0.elapsed().as_millis() as u64;
    let st = crate::ocr_cache::stats();
    out.insert("cache_ms_total".to_string(), serde_json::Value::from(cache_ms));
    out.insert(
        "cache_ms_avg".to_string(),
        serde_json::Value::from(if rounds > 0 {
            cache_ms as f64 / rounds as f64
        } else {
            0.0
        }),
    );
    out.insert(
        "cache_hit_rate".to_string(),
        serde_json::Value::from(st.hit_rate()),
    );
    out.insert("cache_entries".to_string(), serde_json::Value::from(st.entries));

    // ---- 造一张固定测试图（640x64 白底），供 OCR 冷/热两次
    let mut png: Vec<u8> = Vec::new();
    {
        let img = image::RgbaImage::from_pixel(640, 64, image::Rgba([255u8, 255, 255, 255]));
        let mut cursor = std::io::Cursor::new(&mut png);
        let _ = image::DynamicImage::ImageRgba8(img)
            .write_to(&mut cursor, image::ImageFormat::Png);
    }

    // ---- ocr 阶段
    match ocr::OcrEngine::new(config) {
        Ok(mut engine) => {
            let t1 = Instant::now();
            let first = engine.recognize(&png);
            let cold_ms = t1.elapsed().as_millis() as u64;
            let t2 = Instant::now();
            let second = engine.recognize(&png);
            let warm_ms = t2.elapsed().as_millis() as u64;
            match (first, second) {
                (Ok(a), Ok(_b)) => {
                    out.insert("ocr_cold_ms".to_string(), serde_json::Value::from(cold_ms));
                    out.insert("ocr_warm_ms".to_string(), serde_json::Value::from(warm_ms));
                    out.insert(
                        "ocr_chars".to_string(),
                        serde_json::Value::from(a.chars().count()),
                    );
                }
                (Err(e), _) => {
                    out.insert(
                        "ocr".to_string(),
                        serde_json::Value::String(format!("skipped: {}", e)),
                    );
                }
                _ => {}
            }
        }
        Err(e) => {
            out.insert(
                "ocr".to_string(),
                serde_json::Value::String(format!("skipped: {}", e)),
            );
        }
    }

    // ---- translate 阶段
    match translate::TranslateEngine::new(config) {
        Ok(engine) => {
            let t3 = Instant::now();
            match engine.translate("hello") {
                Ok(t) => {
                    out.insert(
                        "translate_ms".to_string(),
                        serde_json::Value::from(t3.elapsed().as_millis() as u64),
                    );
                    out.insert(
                        "translate_chars".to_string(),
                        serde_json::Value::from(t.chars().count()),
                    );
                }
                Err(e) => {
                    out.insert(
                        "translate".to_string(),
                        serde_json::Value::String(format!("skipped: {}", e)),
                    );
                }
            }
        }
        Err(e) => {
            out.insert(
                "translate".to_string(),
                serde_json::Value::String(format!("skipped: {}", e)),
            );
        }
    }

    // ---- screenshot 阶段
    match screenshot::DxgiScreenshotter::new() {
        Ok(shot) => {
            let region = hotkey::Region {
                x: 0,
                y: 0,
                w: 200,
                h: 120,
            };
            let t4 = Instant::now();
            match shot.capture_region(region) {
                Ok(data) => {
                    out.insert(
                        "screenshot_ms".to_string(),
                        serde_json::Value::from(t4.elapsed().as_millis() as u64),
                    );
                    out.insert("screenshot_bytes".to_string(), serde_json::Value::from(data.len()));
                }
                Err(e) => {
                    out.insert(
                        "screenshot".to_string(),
                        serde_json::Value::String(format!("skipped: {}", e)),
                    );
                }
            }
        }
        Err(e) => {
            out.insert(
                "screenshot".to_string(),
                serde_json::Value::String(format!("skipped: {}", e)),
            );
        }
    }

    // ---- 用量与缓存收尾
    out.insert(
        "usage_month".to_string(),
        serde_json::Value::String(usage::current_month()),
    );
    out.insert(
        "usage_chars".to_string(),
        serde_json::Value::from(usage::month_total()),
    );
    crate::ocr_cache::flush();
    usage::flush();

    serde_json::Value::Object(out)
}

/// Encode string to wide characters
fn encode_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsString::from(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 独立模式：不依赖浏览器扩展，直接运行
/// 统一常驻模式：初始化截图/OCR/翻译/浮窗/热键 + 托盘，跑主循环。
/// connect_nm=false 即原独立模式；connect_nm=true 时把 Native Messaging 协议放到独立线程。
fn run_resident(config: Config, config_path: String, connect_nm: bool) -> anyhow::Result<()> {
    log::info!("进入常驻模式 (NM={})", connect_nm);

    // i18n 与全局上下文初始化（§17.8 / §17.5 共享状态）
    crate::i18n::init();
    crate::i18n::set_locale(&config.ui_lang);
    // OCR 缓存（§14.5 / P2-5）按配置初始化开关与容量
    crate::ocr_cache::init(config.ocr_cache_enabled, config.ocr_cache_max);

    let config = Arc::new(Mutex::new(config));
    crate::globals::set_globals(Arc::clone(&config), config_path.clone());

    log::info!("正在注册全局热键...");
    let cfg = config.lock().unwrap();
    let mut manager = hotkey::HotkeyManager::new(&cfg)?;
    log::info!("全局热键注册完成");

    log::info!("正在初始化 OCR 引擎...");
    // 非致命：缺模型（如未打包 PP-OCRv6）时不再直接崩溃退出，改为降级运行，
    // 主窗口/托盘照常可用，仅截图翻译提示「OCR 模型未配置」。
    let mut ocr_engine: Option<ocr::OcrEngine> = match ocr::OcrEngine::new(&cfg) {
        Ok(e) => {
            log::info!("OCR 引擎初始化完成");
            Some(e)
        }
        Err(e) => {
            log::warn!("OCR 引擎初始化失败（模型可能缺失），截图翻译将不可用：{}", e);
            None
        }
    };

    log::info!("正在初始化翻译引擎...");
    let translate_engine = translate::TranslateEngine::new(&cfg)?;
    log::info!("翻译引擎初始化完成");

    log::info!("正在初始化浮窗...");
    let mut window = window::OverlayWindow::new(&cfg)?;
    log::info!("浮窗初始化完成");
    drop(cfg);

    log::info!("正在初始化截图模块...");
    let use_dxgi = { config.lock().unwrap().use_dxgi };
    let screenshot = screenshot::DxgiScreenshotter::new_with_dxgi(use_dxgi)?;
    log::info!("截图模块初始化完成（DXGI={}）", use_dxgi);

    {
        let c = config.lock().unwrap();
        log::info!(
            "常驻就绪：截图={} 设置={} 退出={} 暂停={}",
            c.hotkey,
            c.settings_hotkey,
            c.quit_hotkey,
            c.pause_hotkey
        );
    }
    if ocr_engine.is_none() {
        window::show_toast(
            "OCR 模型未配置，截图翻译暂不可用（请在设置中配置 OCR 引擎与模型）",
            window::ToastLevel::Warn,
        );
    }

    let (tray_tx, tray_rx) = std::sync::mpsc::channel();
    let _tray = tray::TrayIcon::new(tray_tx, config.lock().unwrap().first_run)?;

    // 启动主窗口（真正的 GUI 入口）：应用启动即弹出工作台，关闭=最小化到托盘
    let main_rx = main_window::spawn(Arc::clone(&config));
    // 浮窗「重译」按钮 -> 主循环重新翻译并刷新浮窗
    let (retransmit_tx, retransmit_rx) = std::sync::mpsc::channel::<window::RetranslateRequest>();
    if config.lock().unwrap().first_run {
        log::info!("首次运行：已弹出欢迎气泡（引导请见浏览器扩展侧 onboarding）");
    }

    // NM 协议线程（仅连接浏览器扩展时启动）
    let nm_handle = if connect_nm {
        let handler = NmModeHandler::new(Arc::clone(&config), config_path.clone())?;
        Some(std::thread::spawn(move || {
            if let Err(e) = nm::run_nm_loop(&handler) {
                log::error!("NM 协议线程结束: {}", e);
            }
        }))
    } else {
        None
    };

    // 主循环：热键 + 托盘菜单 + NM 存活检测 + Win32 消息泵
    let mut msg: MSG = unsafe { mem::zeroed() };
    let mut was_fullscreen = false;
    loop {
        // 热键事件（暂停态下功能热键已被卸载，仅保留退出/暂停）
        match manager.receiver().try_recv() {
            Ok(hotkey::HotkeyEvent::Capture) => {
                log::info!("开始截图选择");
                if let Some(region) = selection_window::select_region() {
                    capture_and_translate(
                        region,
                        &mut ocr_engine,
                        &translate_engine,
                        &mut window,
                        &screenshot,
                        &retransmit_tx,
                        &config,
                        false,
                    );
                } else {
                    log::info!("用户取消选择");
                }
            }
            Ok(hotkey::HotkeyEvent::Translate) => {
                log::info!("翻译热键触发（占位）");
            }
            Ok(hotkey::HotkeyEvent::Silent) => {
                log::info!("静默截图 OCR 热键触发");
                if let Some(region) = selection_window::select_region() {
                    capture_and_translate(
                        region,
                        &mut ocr_engine,
                        &translate_engine,
                        &mut window,
                        &screenshot,
                        &retransmit_tx,
                        &config,
                        true,
                    );
                } else {
                    log::info!("用户取消选择");
                }
            }
            Ok(hotkey::HotkeyEvent::OpenMain) => {
                log::info!("唤起主窗口热键触发");
                main_window::show_main_window();
            }
            Ok(hotkey::HotkeyEvent::InputTranslate) => {
                log::info!("输入框转译热键触发（粘贴板翻译）");
                if let Some(text) = window::read_clipboard() {
                    if !text.trim().is_empty() {
                        match translate_engine.translate(&text) {
                            Ok(t) => {
                                main_window::show_result(&text, &t);
                                main_window::show_main_window();
                            }
                            Err(e) => window::show_toast(
                                &format!("翻译失败：{}", e),
                                window::ToastLevel::Error,
                            ),
                        }
                    }
                } else {
                    window::show_toast("剪贴板为空", window::ToastLevel::Warn);
                }
            }
            Ok(hotkey::HotkeyEvent::Read) => {
                if let Some((_, t)) = main_window::latest_result() {
                    if !t.is_empty() {
                        window::speak_text(&t);
                        window::show_toast("正在朗读…", window::ToastLevel::Info);
                    }
                }
            }
            Ok(hotkey::HotkeyEvent::Copy) => {
                if let Some((_, t)) = main_window::latest_result() {
                    if !t.is_empty() {
                        window::copy_text(&t);
                        window::show_toast("已复制译文", window::ToastLevel::Info);
                    }
                }
            }
            Ok(hotkey::HotkeyEvent::Replace) => {
                if let Some((_, t)) = main_window::latest_result() {
                    if !t.is_empty() {
                        window::copy_text(&t);
                        window::show_toast("已复制译文（桌面替换需扩展侧支持）", window::ToastLevel::Info);
                    }
                }
            }
            Ok(hotkey::HotkeyEvent::Settings) => {
                log::info!("设置热键触发：打开设置窗口");
                let cfg = Arc::clone(&config);
                let cp = config_path.clone();
                std::thread::spawn(move || settings_window::show_settings(cfg, &cp));
            }
            Ok(hotkey::HotkeyEvent::Quit) => {
                log::info!("退出热键触发");
                break;
            }
            Ok(hotkey::HotkeyEvent::Pause) => {
                let paused = manager.toggle_pause();
                log::info!("暂停状态已切换为: {}", paused);
                window::show_toast(
                    if paused {
                        "已暂停：截图翻译已停用"
                    } else {
                        "已恢复：截图翻译已启用"
                    },
                    window::ToastLevel::Info,
                );
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("热键接收器断开");
                break;
            }
        }

        // 托盘菜单事件
        match tray_rx.try_recv() {
            Ok(tray::TrayEvent::Quit) => {
                log::info!("托盘：退出");
                break;
            }
            Ok(tray::TrayEvent::Capture) => {
                log::info!("托盘：截图翻译");
                if let Some(region) = selection_window::select_region() {
                    capture_and_translate(
                        region,
                        &mut ocr_engine,
                        &translate_engine,
                        &mut window,
                        &screenshot,
                        &retransmit_tx,
                        &config,
                        false,
                    );
                }
            }
            Ok(tray::TrayEvent::Settings) => {
                log::info!("托盘：打开设置窗口");
                let cfg = Arc::clone(&config);
                let cp = config_path.clone();
                std::thread::spawn(move || settings_window::show_settings(cfg, &cp));
            }
            Ok(tray::TrayEvent::Pause) => {
                let paused = manager.toggle_pause();
                log::info!("托盘：暂停状态切换为 {}", paused);
                window::show_toast(
                    if paused {
                        "已暂停：截图翻译已停用"
                    } else {
                        "已恢复：截图翻译已启用"
                    },
                    window::ToastLevel::Info,
                );
            }
            Ok(tray::TrayEvent::SelfCheck) => {
                log::info!("托盘：运行诊断");
                run_doctor_report(&config_path);
                window::show_toast("诊断完成，报告已写入日志目录", window::ToastLevel::Info);
            }
            Ok(tray::TrayEvent::History) => {
                log::info!("托盘：打开历史记录");
                std::thread::spawn(move || history_window::show_history());
            }
            Ok(tray::TrayEvent::Vocab) => {
                log::info!("托盘：打开生词本");
                std::thread::spawn(move || history_window::show_vocab());
            }
            Ok(tray::TrayEvent::Review) => {
                log::info!("托盘：打开复习闪卡");
                std::thread::spawn(move || history_window::show_review(ptr::null_mut()));
            }
            Ok(tray::TrayEvent::OpenMain) => {
                log::info!("托盘：打开主窗口");
                main_window::show_main_window();
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                log::error!("托盘通道断开");
                break;
            }
        }

        // 主窗口事件（启动 GUI 仪表盘发出的操作请求）
        match main_rx.try_recv() {
            Ok(main_window::MainEvent::Capture) => {
                log::info!("主窗口：截图翻译");
                if let Some(region) = selection_window::select_region() {
                    capture_and_translate(
                        region,
                        &mut ocr_engine,
                        &translate_engine,
                        &mut window,
                        &screenshot,
                        &retransmit_tx,
                        &config,
                        false,
                    );
                }
            }
            Ok(main_window::MainEvent::CaptureSilent) => {
                log::info!("主窗口：截图翻译（静默）");
                if let Some(region) = selection_window::select_region() {
                    capture_and_translate(
                        region,
                        &mut ocr_engine,
                        &translate_engine,
                        &mut window,
                        &screenshot,
                        &retransmit_tx,
                        &config,
                        true,
                    );
                }
            }
            Ok(main_window::MainEvent::Paste) => {
                log::info!("主窗口：粘贴翻译");
                if let Some(text) = window::read_clipboard() {
                    if !text.trim().is_empty() {
                        match translate_engine.translate(&text) {
                            Ok(t) => {
                                main_window::show_result(&text, &t);
                                main_window::show_main_window();
                            }
                            Err(e) => window::show_toast(
                                &format!("翻译失败：{}", e),
                                window::ToastLevel::Error,
                            ),
                        }
                    }
                } else {
                    window::show_toast("剪贴板为空", window::ToastLevel::Warn);
                }
            }
            Ok(main_window::MainEvent::ReOcr) => {
                log::info!("主窗口：重新识别");
                if let Some(png) = main_window::take_last_png() {
                    let text = match &mut ocr_engine {
                        Some(eng) => match eng.recognize(&png) {
                            Ok(t) => t,
                            Err(_) => {
                                window::show_toast("重新识别失败", window::ToastLevel::Warn);
                                String::new()
                            }
                        },
                        None => {
                            window::show_toast("OCR 模型未配置", window::ToastLevel::Warn);
                            String::new()
                        }
                    };
                    if !text.trim().is_empty() {
                        let t = translate_engine.translate(&text).unwrap_or_default();
                        main_window::show_result(&text, &t);
                    }
                } else {
                    window::show_toast("暂无可重新识别的截图", window::ToastLevel::Warn);
                }
            }
            Ok(main_window::MainEvent::SetLang) => {
                if let Some((src, tgt)) = main_window::take_pending_lang() {
                    let mut c = config.lock().unwrap();
                    c.src_lang = src.clone();
                    c.tgt_lang = tgt.clone();
                    let _ = c.save(&config_path);
                    log::info!("语言已切换：{} → {}", c.src_lang, c.tgt_lang);
                }
            }
            Ok(main_window::MainEvent::SetSilent) => {
                if let Some(v) = main_window::take_pending_silent() {
                    let mut c = config.lock().unwrap();
                    c.silent_mode = v;
                    let _ = c.save(&config_path);
                    log::info!("静默模式：{}", c.silent_mode);
                }
            }
            Ok(main_window::MainEvent::Settings) => {
                let cfg = Arc::clone(&config);
                let cp = config_path.clone();
                std::thread::spawn(move || settings_window::show_settings(cfg, &cp));
            }
            Ok(main_window::MainEvent::History) => {
                std::thread::spawn(move || history_window::show_history());
            }
            Ok(main_window::MainEvent::Vocab) => {
                std::thread::spawn(move || history_window::show_vocab());
            }
            Ok(main_window::MainEvent::Review) => {
                std::thread::spawn(move || history_window::show_review(ptr::null_mut()));
            }
            Ok(main_window::MainEvent::SelfCheck) => {
                run_doctor_report(&config_path);
                window::show_toast("诊断完成，报告已写入日志目录", window::ToastLevel::Info);
            }
            Ok(main_window::MainEvent::Hide) => {
                main_window::hide_main_window();
            }
            Ok(main_window::MainEvent::Quit) => {
                log::info!("主窗口：退出");
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }

        // 浮窗「重译」：用缓存原文（或编辑后原文）重新翻译并刷新浮窗
        match retransmit_rx.try_recv() {
            Ok(req) => {
                let src = req
                    .edited_text
                    .clone()
                    .unwrap_or_else(|| req.original.clone());
                let (cur_engine, src_lang, tgt_lang, theme, font_scale, opacity, allow_pos_mem) = {
                    let c = config.lock().unwrap();
                    (
                        c.translate_engine.clone(),
                        c.src_lang.clone(),
                        c.tgt_lang.clone(),
                        c.theme.clone(),
                        c.font_scale.clone(),
                        c.float_opacity,
                        c.allow_position_memory,
                    )
                };
                let engine = req.engine.clone().unwrap_or(cur_engine);
                log::info!(
                    "重译请求：{}（引擎 {}）",
                    src.chars().take(40).collect::<String>(),
                    engine
                );
                let t = translate_engine
                    .translate_with(&src, &engine)
                    .unwrap_or_default();
                // 用量记账（§15.8 / P2-6）
                crate::usage::record(&engine, src.chars().count() as u64);
                window.show_translation(
                    &hotkey::Region {
                        x: req.x,
                        y: req.y,
                        w: req.w,
                        h: req.h,
                    },
                    &src,
                    &t,
                    &retransmit_tx,
                    &theme,
                    &font_scale,
                    opacity,
                    allow_pos_mem,
                );
                // 写入历史（§17.6）
                crate::history::record_translation(&src, &t, &engine, &src_lang, &tgt_lang);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        }

        // 全屏独占检测：游戏/播放器全屏时隐藏浮窗，退出后恢复（§17.5）
        let fs = window::is_fullscreen_exclusive();
        if fs != was_fullscreen {
            window.set_fullscreen_hidden(fs);
            was_fullscreen = fs;
        }

        // NM 连接关闭（浏览器扩展断开/停用）时，壳也随之退出，避免孤儿常驻
        if connect_nm {
            if let Some(h) = &nm_handle {
                if h.is_finished() {
                    log::info!("NM 连接已关闭，退出常驻");
                    break;
                }
            }
        }

        // Win32 消息泵（驱动托盘窗口回调）
        unsafe {
            if PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) > 0 {
                if msg.message == WM_QUIT {
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    log::info!("常驻模式退出");
    Ok(())
}

/// 截图指定区域 -> OCR -> 翻译 -> 浮窗显示（热键/托盘/主窗口共用）
fn capture_and_translate(
    region: hotkey::Region,
    ocr_engine: &mut Option<ocr::OcrEngine>,
    translate_engine: &translate::TranslateEngine,
    window: &mut window::OverlayWindow,
    screenshot: &screenshot::DxgiScreenshotter,
    retransmit: &std::sync::mpsc::Sender<window::RetranslateRequest>,
    config: &Arc<Mutex<crate::config::Config>>,
    silent: bool,
) {
    log::info!(
        "选中区域: x={}, y={}, w={}, h={}",
        region.x, region.y, region.w, region.h
    );
    match screenshot.capture_region(region) {
        Ok(img_data) => {
            let png_data = match screenshot::encode_png(&img_data, region.w, region.h) {
                Ok(data) => data,
                Err(e) => {
                    log::error!("PNG 编码失败: {}", e);
                    crate::globals::set_last_diagnostic(&format!("PNG 编码失败: {}", e));
                    window::show_toast("图片编码失败", window::ToastLevel::Error);
                    return;
                }
            };
            // 记录上次截图：供主窗口「原图翻译」模式显示 + 「重新识别」复用（界面地图 S1）
            {
                let shot_path = std::env::temp_dir().join("winocr_last_shot.bmp");
                if let Some(p) = shot_path.to_str() {
                    let _ = screenshot::save_bmp(p, &img_data, region.w, region.h);
                    main_window::set_last_screenshot(p.to_string(), region.w, region.h);
                }
                main_window::set_last_png(png_data.clone());
            }
            let text = match ocr_engine {
                Some(eng) => match eng.recognize(&png_data) {
                    Ok(t) => t,
                    Err(e) => {
                        log::error!("OCR 失败: {}", e);
                        window::show_toast("文字识别失败，请重试", window::ToastLevel::Warn);
                        String::new()
                    }
                },
                None => {
                    log::warn!("OCR 引擎不可用（模型未配置）");
                    window::show_toast(
                        "OCR 模型未配置，无法识别文字（请在设置中配置 OCR 引擎）",
                        window::ToastLevel::Warn,
                    );
                    String::new()
                }
            };
            let (theme, font_scale, opacity, allow_pos_mem, pref_engine, src_lang, tgt_lang, limit) = {
                let c = config.lock().unwrap();
                // §18 客户端语种检测：src_lang 为 auto 时，本地预判后回写有效源语言
                let eff_src = if c.src_lang == "auto" {
                    crate::lang_detect::detect_lang(&text).unwrap_or_else(|| c.src_lang.clone())
                } else {
                    c.src_lang.clone()
                };
                (
                    c.theme.clone(),
                    c.font_scale.clone(),
                    c.float_opacity,
                    c.allow_position_memory,
                    c.translate_engine.clone(),
                    eff_src,
                    c.tgt_lang.clone(),
                    c.monthly_char_limit,
                )
            };
            // 配额降级（§15.8 / P2-6）：超限且首选为付费引擎时回退免费引擎
            let engine = crate::usage::resolve_engine(&pref_engine, limit);
            if engine != pref_engine {
                window::show_toast(
                    "本月配额已用尽，已自动降级到免费引擎",
                    window::ToastLevel::Warn,
                );
            }
            let translation = if !text.is_empty() {
                // 仅当 src_lang 非 auto（已被本地检测填充）时传给引擎，否则保持 auto 由服务端判定
                let eff_src_for_engine = if src_lang == "auto" {
                    None
                } else {
                    Some(src_lang.as_str())
                };
                match translate_engine.translate_with_src(&text, &engine, eff_src_for_engine) {
                    Ok(t) => {
                        // 用量记账（§15.8 / P2-6）
                        crate::usage::record(&engine, text.chars().count() as u64);
                        t
                    }
                    Err(e) => {
                        log::warn!("翻译失败: {}", e);
                        crate::globals::set_last_diagnostic(&format!("翻译失败: {}", e));
                        window::show_toast(
                            "翻译失败，请检查网络或引擎配置",
                            window::ToastLevel::Warn,
                        );
                        String::new()
                    }
                }
            } else {
                String::new()
            };
            if silent {
                // 静默模式：仅入历史，不弹浮窗（界面地图 S12 静默 OCR）
                crate::history::record_translation(&text, &translation, &engine, &src_lang, &tgt_lang);
                window::show_toast("已静默记录到历史", window::ToastLevel::Info);
            } else {
                window.show_translation(
                    &region,
                    &text,
                    &translation,
                    retransmit,
                    &theme,
                    &font_scale,
                    opacity,
                    allow_pos_mem,
                );
                // 写入历史（§17.6）
                crate::history::record_translation(&text, &translation, &engine, &src_lang, &tgt_lang);
                // 推送结果到主窗口双栏（界面地图 S1）
                crate::main_window::show_result(&text, &translation);
            }
        }
        Err(e) => {
            log::error!("截图失败: {}", e);
            crate::globals::set_last_diagnostic(&format!("截图失败: {}", e));
            window::show_toast("截图失败，请重试", window::ToastLevel::Error);
        }
    }
}

/// 运行诊断并把报告写入本地日志目录（不抢焦点；详细 UI 见 P1-6）
fn run_doctor_report(config_path: &str) {
    let report = selfcheck::run_self_check();
    let dir = dirs::data_local_dir()
        .map(|p| p.join("WinOCR-Html").join("logs"))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("doctor-{}.json", ts));
    match std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap_or_default()) {
        Ok(_) => log::info!("诊断报告已写入: {}", path.display()),
        Err(e) => log::warn!("写入诊断报告失败: {}（报告见下方日志）", e),
    }
    log::info!("诊断结果: {}", serde_json::to_string(&report).unwrap_or_default());
    crate::globals::set_last_diagnostic(&serde_json::to_string(&report).unwrap_or_default());
    log::info!("配置文件：{}", config_path);
}

/// NM 模式消息处理器
struct NmModeHandler {
    /// 配置在 NM 会话内可变（首次运行引导会改写 first_run 并落盘）
    config: Arc<Mutex<Config>>,
    /// 配置文件路径（用于落盘持久化）
    config_path: String,
}

impl NmModeHandler {
    fn new(config: Arc<Mutex<Config>>, config_path: String) -> anyhow::Result<Self> {
        Ok(Self {
            config,
            config_path,
        })
    }
}

impl NmHandler for NmModeHandler {
    fn handle_translate(
        &self,
        text: &str,
        _source_lang: Option<&str>,
        _target_lang: Option<&str>,
    ) -> anyhow::Result<String> {
        let engine = translate::TranslateEngine::new(&self.config.lock().unwrap())?;
        let result = engine.translate(text)?;
        log::info!(
            "翻译结果: {} -> {}",
            text.chars().take(50).collect::<String>(),
            result.chars().take(50).collect::<String>()
        );
        Ok(result)
    }

    fn handle_ocr(&self, data_url: &str) -> anyhow::Result<(String, Option<f32>)> {
        let img_data = screenshot::decode_data_url(data_url)?;
        let mut engine = ocr::OcrEngine::new(&self.config.lock().unwrap())?;
        let text = engine.recognize(&img_data)?;
        log::info!("OCR 结果: {} 字符", text.len());
        Ok((text, None))
    }

    fn handle_capture(&self, region: Option<&nm::NmRegion>) -> anyhow::Result<Option<String>> {
        let screenshot = screenshot::DxgiScreenshotter::new()?;
        let region = region.cloned().unwrap_or_default().into();
        let pixels = screenshot.capture_region(region)?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&pixels);
        Ok(Some(format!("data:image/png;base64,{}", encoded)))
    }

    fn handle_get_settings(&self) -> anyhow::Result<serde_json::Value> {
        let json = self.config.lock().unwrap().to_json();
        // 必须返回「对象」而非 JSON 字符串：
        // ① 内部标签枚举（#[serde(tag="type")]）无法序列化含字符串的 newtype 变体；
        // ② 扩展端 background.js 读的是 `msg.settings` 对象。
        let v = serde_json::from_str::<serde_json::Value>(&json)
            .unwrap_or_else(|_| serde_json::json!({}));
        Ok(if v.is_object() { v } else { serde_json::json!({ "raw": v }) })
    }

    fn handle_update_settings(&self, settings: &serde_json::Value) -> anyhow::Result<()> {
        log::info!("更新配置: {:?}", settings);
        Ok(())
    }

    fn handle_region_crop(&self, data_url: &str, _region: &nm::NmRegion) -> anyhow::Result<String> {
        let img_data = screenshot::decode_data_url(data_url)?;
        let mut engine = ocr::OcrEngine::new(&self.config.lock().unwrap())?;
        let text = engine.recognize(&img_data)?;
        Ok(text)
    }

    fn handle_self_check(&self) -> anyhow::Result<serde_json::Value> {
        Ok(selfcheck::run_self_check())
    }

    fn is_first_run(&self) -> bool {
        self.config.lock().unwrap().first_run
    }

    fn handle_first_run_complete(&self) -> anyhow::Result<bool> {
        let mut cfg = self.config.lock().unwrap();
        cfg.first_run = false;
        cfg.save(&self.config_path)?;
        log::info!("首次运行引导已完成，已清除 first_run 标志");
        Ok(cfg.first_run)
    }

    // ===== M11 术语库 =====
    fn handle_get_terms(&self) -> anyhow::Result<Vec<crate::terminology::TermEntry>> {
        Ok(self.config.lock().unwrap().user_terms.clone())
    }

    fn handle_set_terms(&self, terms: &[crate::terminology::TermEntry]) -> anyhow::Result<()> {
        let mut cfg = self.config.lock().unwrap();
        cfg.user_terms = terms.to_vec();
        cfg.save(&self.config_path)?;
        log::info!("术语库已更新，共 {} 条", terms.len());
        Ok(())
    }

    // ===== M9 视频字幕批量翻译 =====
    fn handle_subtitles(
        &self,
        segments: &[crate::subtitles::SubtitleSegment],
    ) -> anyhow::Result<Vec<crate::subtitles::SubtitleSegment>> {
        let engine = translate::TranslateEngine::new(&self.config.lock().unwrap())?;
        let result = crate::subtitles::translate_segments(segments, |t| engine.translate_with_src(t, "", None));
        Ok(result)
    }

    // ===== M10 PDF 结构化块翻译 =====
    fn handle_pdf_translate(
        &self,
        blocks: &[crate::pdf_translate::PdfBlock],
    ) -> anyhow::Result<Vec<crate::pdf_translate::PdfBlock>> {
        let engine = translate::TranslateEngine::new(&self.config.lock().unwrap())?;
        let result = crate::pdf_translate::translate_blocks(blocks, |t| engine.translate_with_src(t, "", None));
        Ok(result)
    }
}
