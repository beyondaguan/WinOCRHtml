// ============================================================
// UI 层入口
// 用 slint::include_modules!() 引入 main_window.slint 编译出的 MainWindow，
// 将 Slint 回调接到 Services 占位实现（仅日志），最后启动事件循环。
// ============================================================
slint::include_modules!();

use crate::services;
use crate::services::translate::Translate;
use crate::services::persist::Persist;
use crate::services::ai::Ai;
use crate::services::collect::Collect;
use crate::services::export::{Export, ExportFormat};
use crate::core::types::Lang;
use crate::core::config::Config;
use log::{info, warn};
use slint::Model;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::path::PathBuf;
use std::collections::HashSet;
use tokio::runtime::Runtime;

// ============================================================
// P3-3 划词气泡常量
// ============================================================
/// 气泡窗口标题：Win32 FindWindowW 的定位键，必须与 app/src/ui/bubble.slint 的 title 逐字一致
const BUBBLE_TITLE: &str = "WinOCR-SelectionBubble";
const BUBBLE_W: i32 = 360;
const BUBBLE_H: i32 = 224;
/// 把气泡停到屏幕外 = 隐藏。
/// 铁律：显隐一律走 SetWindowPos（overlay_place），绝不调用 Slint 的 hide() ——
/// winit 的 apply_diff 只样式有 diff 就用缓存值整体重写 GWL_EXSTYLE，会把
/// WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW 清掉并 ShowWindow 抢焦点。
const BUBBLE_PARK: i32 = -32000;

/// 主窗口标题（Win32 FindWindowW 定位键）：必须与 main_window.slint 的 title
/// 逐字一致，且刻意不带版本号（见 main.rs 注释）。main.rs 也引用此常量。
pub const MAIN_TITLE: &str = "WinOCR";

/// 把主窗口显示、提到前台并停双栏页（ALT+Q 热键路径的结果回填前调用）。
/// 第一性原理：热键是"在他处触发、结果要即时浮现"——只把文本写进一个
/// 可能隐藏（关窗=托盘）或在后台被遮挡的窗口，等于把答案写在看不见的纸上。
fn bring_main_to_front(win: &MainWindow) {
    if !win.window().is_visible() {
        let _ = win.window().show();
    }
    // 工作台始终显示，无需切页；关掉任何打开的抽屉让工作台露出来
    win.set_drawer_type("".into());
    win.set_drawer_visible(false);
    // show() 在 Windows 前台锁定规则下不一定抢得到前台，显式 SetForegroundWindow 最稳
    let _ = crate::services::single_instance::activate_window_by_title(MAIN_TITLE);
}

/// 划词气泡的越界校正定位：锚点 = 选区落点 (ax, ay)，默认落右下 (ax+12, ay+18)，
/// 底部放不下翻到选区上方，右侧放不下左移，保证气泡完全在屏内。
/// 尺寸用 overlay_rect 实测物理像素（高 DPI 缩放下 Slint 逻辑尺寸偏小）；
/// 量不到窗口（尚未 show）时用 fb 的逻辑尺寸兜底。
#[cfg(windows)]
fn fit_bubble_pos(ax: i32, ay: i32, fb: (i32, i32)) -> (i32, i32) {
    let (sw, sh) = crate::services::attach::screen_size();
    let (w, h) = crate::services::attach::overlay_rect(BUBBLE_TITLE)
        .map(|(l, t, r, bo)| (r - l, bo - t))
        .unwrap_or(fb);
    let mut px = ax + 12;
    let mut py = ay + 18;
    if py + h + 8 > sh {
        py = ay - h - 18;
    }
    if px + w + 8 > sw {
        px = (sw - w - 8).max(0);
    }
    (px, py)
}

// ============================================================
// 生词本 · 候选词提取（截图后按词拆分，过滤忽略词/简单词）
// ============================================================
/// 从一段 OCR 文本中提取候选生词：
/// - 按非字母切分；单字母跳过；
/// - 小写化后与忽略词表比对（to/where 等简单词被排除）；
/// - 按首次出现顺序去重；保留原始大小写。
fn extract_candidates(text: &str, ignore: &[String]) -> Vec<String> {
    let ignore_set: HashSet<&str> = ignore.iter().map(|s| s.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for token in text.split(|c: char| !c.is_alphabetic()) {
        let raw = token.trim();
        if raw.is_empty() {
            continue;
        }
        let low = raw.to_lowercase();
        if low.len() < 2 || ignore_set.contains(low.as_str()) {
            continue;
        }
        if !seen.insert(low.clone()) {
            continue;
        }
        out.push(raw.to_string());
    }
    out
}

/// 候选词 chip 的排版预算（与 gen_slint_v2.py 里候选词面板的样式保持一致）。
const CAND_CHIP_H_PADDING: f32 = 20.0;
const CAND_CHIP_SPACING: f32 = 6.0;
const CAND_PANEL_PADDING_X: f32 = 16.0;
/// 12px / 600 字重的粗略平均字宽（无字体度量 API，误差由 max_width 兜底）。
const CAND_CHAR_W: f32 = 7.6;
/// 面板最多显示 4 行（与 slint 侧 max-height: 132px 对齐），超出给"+N 个"提示。
const CAND_MAX_ROWS: usize = 4;

/// 窗口最小尺寸（逻辑像素）。低于此宽度抽屉(480px)+导航轨(60px)会把中央舞台压扁。
const MIN_WINDOW_W: i32 = 900;
const MIN_WINDOW_H: i32 = 560;

/// 界面缩放档位（%）。下标与 slint 侧 `ui-scale-options` 模型一一对应。
const UI_SCALE_OPTIONS: [i32; 6] = [75, 100, 125, 150, 175, 200];
/// 界面字体族选项。下标与设置页 ComboBox 一一对应；空串 = Slint 内置默认。
const UI_FONT_OPTIONS: [&str; 5] = ["", "Microsoft YaHei UI", "SimSun", "SimHei", "KaiTi"];

/// 按容器宽度把候选词贪心分行为 chip 行。
///
/// Slint 没有自动换行容器，候选词若全塞进一个 `HorizontalLayout`：
/// ① 一行放不下的词被容器右缘裁掉（用户报的「多个词条后面无法显示」）；
/// ② 更糟的是 chip 若未设 `horizontal-stretch: 0`，Slint 会按 stretch 分配把它撑成等宽大块，
///    使这一行的 preferred-width 远超容器，把中央舞台（`clip: true`）的内容整体顶出可视区，
///    连带把翻译栏的「朗读/复制」按钮推出窗口。
///
/// 故这里在 Rust 侧按宽度预算分行，Slint 只负责逐行渲染（每行内 chip 不拉伸）。
///
/// 返回 `(每行的词, 溢出提示文案)`。
fn layout_candidate_rows(words: &[String], container_w: f32) -> (Vec<Vec<String>>, String) {
    if words.is_empty() {
        return (Vec::new(), String::new());
    }
    // 面板可用宽度：容器减去左右留白与边框。留 8px 余量避免像素取整导致最后一块被裁。
    let avail = (container_w - CAND_PANEL_PADDING_X - 8.0).max(80.0);

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_w = 0.0f32;

    for word in words {
        let w = CAND_CHIP_H_PADDING + word.chars().count() as f32 * CAND_CHAR_W;
        let need = if cur.is_empty() { w } else { w + CAND_CHIP_SPACING };
        if !cur.is_empty() && cur_w + need > avail {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0.0;
            if rows.len() >= CAND_MAX_ROWS {
                break;
            }
        }
        cur_w += if cur.is_empty() { w } else { w + CAND_CHIP_SPACING };
        cur.push(word.clone());
    }
    if !cur.is_empty() && rows.len() < CAND_MAX_ROWS {
        rows.push(cur);
    }

    let shown: usize = rows.iter().map(|r| r.len()).sum();
    let hidden = words.len().saturating_sub(shown);
    let overflow = if hidden > 0 {
        format!("另有 {hidden} 个…")
    } else {
        String::new()
    };
    (rows, overflow)
}

/// 把生词条目渲染成列表行文本：`单词 → 释义 [★☆…] [项目]`。
///
/// 释义为空时（双击候选词直接入本、释义待补）渲染成显式的「（待补释义）」，
/// 否则会显示成 `word →  [★★]` —— 看不出缺什么。
///
/// open-drawer 与 load-collect 两条路径共用，避免两处逻辑漂移。
fn format_collect_row(i: &crate::services::collect::CollectItem) -> String {
    let m = i.mastery.clamp(0, 5) as usize;
    let stars = "★".repeat(m);
    let spaces = "☆".repeat(5 - m);
    let tr = if i.translation.trim().is_empty() {
        "（待补释义）"
    } else {
        i.translation.as_str()
    };
    format!("{} → {} [{}{}] [{}]", i.word, tr, stars, spaces, i.project)
}

/// 单词学习窗口：把队列中某词填充到复习卡字段（摘要→正面/答案面共用）。
/// 按分类过滤排序学习队列（2026-10-17 分类选学）。
/// cat: 0 全部（待复习优先 + 未学垫后，学习中不进默认队列——防提前看释义虚高自评分）
/// 1 待复习（到期升序）/ 2 未学（入库先后）/ 3 学习中（未到期提前学）
fn study_filter_cat(
    all: Vec<crate::services::collect::CollectItem>,
    cat: i32,
) -> Vec<crate::services::collect::CollectItem> {
    let mut review: Vec<_> = all.iter().filter(|i| i.study_status() == 2).cloned().collect();
    let mut fresh: Vec<_> = all.iter().filter(|i| i.study_status() == 0).cloned().collect();
    let mut learning: Vec<_> = all.iter().filter(|i| i.study_status() == 1).cloned().collect();
    review.sort_by_key(|i| i.next_review);
    fresh.sort_by_key(|i| i.ts);
    learning.sort_by_key(|i| i.next_review);
    match cat {
        1 => review,
        2 => fresh,
        3 => learning,
        _ => {
            review.extend(fresh);
            review
        }
    }
}

/// 队列 → 结构化行模型（状态点颜色/右侧状态文案在 slint 侧按 status 渲染）
fn study_build_rows(
    items: &[crate::services::collect::CollectItem],
) -> slint::ModelRc<QueueRow> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let rows: Vec<QueueRow> = items
        .iter()
        .map(|i| {
            let st = i.study_status();
            let status_text: slint::SharedString = match st {
                0 => "未学".into(),
                1 => {
                    // 未到期 →「N 天后」（向上取整，至少 1 天）
                    let days = ((i.next_review - now + 86_399) / 86_400).max(1);
                    format!("{days} 天后").into()
                }
                _ => "待复习".into(),
            };
            QueueRow {
                word: i.word.clone().into(),
                meaning: if i.translation.trim().is_empty() {
                    "（待补释义）".into()
                } else {
                    i.translation.lines().next().unwrap_or("").to_string().into()
                },
                status: st as i32,
                status_text,
            }
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(rows))
}

fn study_load_word(w: &StudyWindow, item: &crate::services::collect::CollectItem) {
    w.set_word(item.word.clone().into());
    w.set_phonetic(item.phonetic.clone().into());
    w.set_definition(item.definition_en.clone().into());
    // 方案A 渐进披露（2026-10-17 用户拍板）：核心释义 = 中文释义首行，五段详解默认折叠；
    // 换词时重置折叠状态，避免上一张卡的详解残留
    let core = item
        .translation
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    w.set_core_meaning(core.into());
    w.set_detail_open(false);
    // 释义卡标题随词典来源切换（每次载词读 config，设置里切换后打开词卡即生效）
    w.set_dict_label(
        if crate::services::dict::study_dict_name() == "ecdict" {
            "英汉释义（ECDICT）"
        } else {
            "英英释义（English Definition）"
        }
        .into(),
    );
    w.set_example(item.example.clone().into());
    w.set_roots(item.roots.clone().into());
    w.set_context(item.context.clone().into());
    w.set_level(item.level.clone().into());
    w.set_project(item.project.clone().into());
    // 中文释义编号分行（按 ；; 换行 拆分 → ①②③…，对齐原型卡片列表）
    const NUMS: [&str; 10] = ["①", "②", "③", "④", "⑤", "⑥", "⑦", "⑧", "⑨", "⑩"];
    let parts: Vec<&str> = item
        .translation
        .split(|c| c == '；' || c == ';' || c == '\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let lines = if parts.len() > 1 {
        parts
            .iter()
            .enumerate()
            .map(|(k, s)| format!("{} {}", NUMS[k.min(9)], s))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        item.translation.trim().to_string()
    };
    w.set_trans_lines(lines.into());
    w.set_cn_hidden(false);
    w.set_reps(item.repetitions as i32);
    w.set_interval(item.interval as i32);
    // 词根块（原型 root-blocks）：roots_text "nephr（肾） + itis（炎症）" 按 + 拆块。
    // ⚠️ 截断到 4 块：数词类（eighty 等）的日耳曼词源链会拆出十几个块横向溢出
    let mut parts: Vec<&str> = item
        .roots
        .split('+')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() > 4 {
        parts.truncate(3);
        parts.push("…");
    }
    let chips: Vec<slint::SharedString> = parts
        .into_iter()
        .map(|s| slint::SharedString::from(s.to_string()))
        .collect();
    w.set_root_chips(slint::ModelRc::new(slint::VecModel::from(chips)));
    // 词源链（拆解块下方的"故事"行；enrich 侧已截断 300 字符）
    w.set_etymology(item.etymology.clone().into());
}

/// 学习窗口主题：**已废弃**（2026-10-17 复刻《单词学习卡片原型.html》后，
/// 窗口使用原型专属紫色配色，不跟随主窗主题）。保留占位说明，防止误加回来。

/// 词卡词典增强（后台线程）：「学习 = 查字典」核心链路。
/// open 词卡自动触发，「查词典」按钮手动重查，两条路径共用。
/// 1) open-dictionary 全文（词性分块+义项编号+例句跟随+词源）→ 英英释义区
/// 2) ipa_en.json 补音标（卡片为空时回填 UI + 写库 update_enrichment）
/// 3) 中文释义为空 → Argos en→zh → 中文释义区 + 写库 update_translation
/// 单项失败互不阻塞；词卡已换词时丢弃结果（回填前校验当前词一致）。
fn spawn_card_enrich(weak: slint::Weak<MainWindow>, word: String, tr: Arc<Translate>, rt: Arc<Runtime>) {
    let word_l = word.trim().to_lowercase();
    if word_l.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        // 1) 词典全文
        let dict = crate::services::dict::Dict::new();
        let full = match dict.lookup(&word_l) {
            Ok(Some(entry)) => crate::services::dict::format_full(&entry),
            Ok(None) => String::new(),
            Err(e) => {
                warn!("[ui] 词典查询 {word_l} 失败: {e:#}");
                String::new()
            }
        };
        // 2) 音标（ipa_en.json 进程级缓存）
        let ipa = crate::services::dict::phonetic_of(&word_l).unwrap_or_default();
        // 3) 中文释义（库内已有则不再翻译）
        let coll = Collect::new();
        let items = coll.list_all();
        let item = items.iter().find(|i| i.word.eq_ignore_ascii_case(&word_l));
        let need_zh = item.map(|i| i.translation.trim().is_empty()).unwrap_or(false);
        let need_ipa = item.map(|i| i.phonetic.trim().is_empty()).unwrap_or(false);
        let zh = if need_zh {
            rt.block_on(tr.translate(&word_l, Lang::En, Lang::Zh))
                .ok()
                .map(|r| r.text.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default()
        } else {
            String::new()
        };
        // 写库（只补空，不覆盖已有值）
        if let Some(it) = item {
            if !zh.is_empty() {
                let _ = coll.update_translation(&it.id, &zh);
            }
            if need_ipa && !ipa.is_empty() {
                let _ = coll.update_enrichment(&it.id, &ipa, "", "", "", "", "");
            }
        }
        // 回填 UI：仅当词卡当前仍是目标词
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = weak.upgrade() {
                if w.get_word_card_word().to_string().eq_ignore_ascii_case(&word_l) {
                    if !full.is_empty() {
                        w.set_word_card_definition(full.into());
                    }
                    if !ipa.is_empty() && w.get_word_card_phonetic().trim().is_empty() {
                        w.set_word_card_phonetic(ipa.into());
                    }
                    if !zh.is_empty() {
                        w.set_word_card_translation(zh.into());
                    }
                }
            }
        });
    });
}

pub fn run() -> anyhow::Result<()> {
    let window = MainWindow::new()?;
    // ===== 窗口初始尺寸与下限（必须在 slint 侧之外设置）=====
    // ⚠️ 关键：main_window.slint 里**故意不写** width/height/min-*。
    // 源码实证（i-slint-compiler-1.18.1/builtin_elements.rs:2088）：Window.width 标注
    // `OnlyInSC`，文档写明「应用创建组件时给予尺寸，这是文件读取的值，**绑定它就是错误**」。
    // 一旦在 .slint 里写 `width: 1130px`，就会覆盖应用给的真实尺寸 → 内容宽度被永久钉死，
    // 窗口拖到 1600px 宽内容仍只占 1130px（用户报的「不能全屏/放大」）。
    // 故初始尺寸与最小尺寸都在这里给出，slint 侧只需 stretch 即可正确跟随。
    {
        use slint::{LogicalSize, WindowSize};
        // 初始尺寸：从上次运行的 config 读（用户拖过窗口大小的记忆），缺省 1130x640。
        let (w0, h0) = match Config::load() {
            Ok(cfg) if cfg.window_w >= MIN_WINDOW_W && cfg.window_h >= MIN_WINDOW_H => {
                (cfg.window_w as f32, cfg.window_h as f32)
            }
            _ => (1130.0, 640.0),
        };
        window
            .window()
            .set_size(WindowSize::Logical(LogicalSize::new(w0, h0)));
    }

    // ============================================================
    // 单词学习窗口（独立 StudyWindow，2026-10-17 迁出主窗抽屉）
    // 进程级常驻（随 run() 作用域存活），默认隐藏；学习入口/引入单词时 show。
    // 回调接线在下方「单词学习窗口」区块。
    // ============================================================
    let study = StudyWindow::new()?;
    {
        use slint::{LogicalSize, WindowSize};
        // 原型复刻：卡片式竖版比例（430px 卡 + 边距），与《单词学习卡片原型.html》一致
        study
            .window()
            .set_size(WindowSize::Logical(LogicalSize::new(480.0, 800.0)));
        // UI 缩放与主窗同机制（default-font-size = 13px × pct%），否则用户缩放对独立窗口无效
        let cfg_s = crate::core::config::Config::load().unwrap_or_default();
        study.set_ui_scale_pct(if (75..=200).contains(&cfg_s.ui_scale_pct) {
            cfg_s.ui_scale_pct
        } else {
            100
        });
    }

    // 在主线程同步预热 OCR 模型（关键修复！）。
    // 原因：ort 的全局 Environment 必须在【主线程】初始化。此前 prewarm 在后台线程
    // 首次触发 Session 创建，ort 内部 panic 并污染其全局环境 Mutex —— 表现为每次 OCR
    // 都因 Mutex poison 崩溃，且进程退出时 abort。改为在主线程预热后：
    //   · 与单测（主线程创建 Session）成功路径一致，规避 off-main-thread panic；
    //   · 窗口出现时模型已就绪，首帧框选不再卡顿、不再崩溃。
    // 代价：启动多约 1-2s（模型加载），可接受。
    window.set_status_text("模型加载中…".into());
    services::ocr::prewarm();
    window.set_status_text("● 模型已就绪 · Alt+Q 截图".into());

    // 翻译服务 + tokio runtime（常驻，跨回调共享）。
    // translate 是 async（Argos 内部 spawn_blocking 需 runtime）；UI 回调在 UI 线程触发，
    // 实际翻译丢后台线程用 rt.block_on 驱动，避免阻塞 UI 事件循环。
    let translate = Arc::new(Translate::new());
    let rt = Arc::new(
        Runtime::new().map_err(|e| anyhow::anyhow!("tokio runtime 创建失败: {e}"))?,
    );

    // Phase 3.1 翻译预热：后台异步加载 Argos 离线模型（不阻塞启动，失败静默）。
    {
        let tr = translate.clone();
        let rt_pre = rt.clone();
        std::thread::spawn(move || {
            let _ = rt_pre.block_on(tr.prewarm());
        });
    }

    // 持久化（历史记录 SQLite，常驻）。
    let persist = Arc::new(Persist::new());

    // 旧 JSONL 数据迁移到 SQLite（幂等：有旧文件才执行）。
    if let Err(e) = crate::services::migrate::migrate_if_needed() {
        log::warn!("[ui] 旧数据迁移失败: {e:#}");
    }

    // AI 对话（SiliconFlow / Ollama 流式）。
    let ai = Arc::new(Ai::new());
    // 启动时立即应用持久化的 AI 配置（含解密后的 key），
    // 否则 AI 状态检测 is_available() 只看到空 key，永远显示「未连接」。
    {
        let cfg = crate::core::config::Config::load().unwrap_or_default();
        let backend = if cfg.ai_backend == "ollama" {
            crate::services::ai::AiBackend::Ollama
        } else {
            crate::services::ai::AiBackend::SiliconFlow
        };
        ai.configure(
            backend,
            &cfg.ai_sf_key,
            &cfg.ai_sf_model,
            &cfg.ollama_url,
            &cfg.ollama_model,
        );
    }

    // 生词本（P2：SM-2 + JSONL + 项目分组）。
    let collect = Arc::new(Collect::new());

    // 导出（P3：Markdown / 纯文本，按项目过滤）。
    let export_svc = Arc::new(Export::new());

    // ============================================================
    // P3-3 划词气泡（Selection Bubble）
    // 独立无边框置顶浮层窗口，常驻整个进程生命周期；显隐靠 services::attach 的
    // overlay_place（SetWindowPos）而非 Slint hide()（理由见 BUBBLE_PARK 注释）。
    // ============================================================
    let bubble = SelectionBubble::new()?;
    // 首次显示标记：首次 show() 会短暂抢焦点，需随即打 ex-style 并把前台窗口还回去。
    // 用 Arc<AtomicBool> 而非 Rc<Cell>：这些状态要经 invoke_from_event_loop 投递到
    // UI 线程（该 API 要求闭包 Send + 'static，Rc 不满足）。
    let bubble_armed = Arc::new(AtomicBool::new(false));
    // 气泡当前屏幕坐标（用于「点击气泡外部即隐」的矩形命中判定）；初值 = 隐藏态。
    let bubble_pos = Arc::new((AtomicI32::new(BUBBLE_PARK), AtomicI32::new(BUBBLE_PARK)));

    // ---- 气泡按钮回调（均在 UI 线程）----
    // 复制译文：真实写入系统剪贴板（走 services::attach::copy_text_to_clipboard，
    // UI 层不碰 Win32）。反馈写气泡自己的 notice-text —— 此刻译文区被译文占满，
    // 写 status-text 会被挤掉而永远看不见。
    {
        let weak_b = bubble.as_weak();
        bubble.on_copy_trans(move || {
            let Some(b) = weak_b.upgrade() else { return };
            let t = b.get_trans_text().to_string();
            if t.trim().is_empty() {
                return; // 译文还没出来，静默忽略
            }
            let n = t.chars().count();
            if services::attach::copy_text_to_clipboard(&t) {
                b.set_notice_text("已复制".into());
                info!("[ui] bubble copy-trans -> 剪贴板（{n} 字符）");
            } else {
                b.set_notice_text("复制失败".into());
                warn!("[ui] bubble copy-trans 失败（剪贴板被其他程序占用？）");
            }
        });
    }

    // 收藏生词：写生词本（项目名「划词」）。语言码取当前主窗口语言对的实际值。
    {
        let weak_b = bubble.as_weak();
        let weak_m = window.as_weak();
        let cb = collect.clone();
        let tr_sw = translate.clone();
        let rt_sw = rt.clone();
        bubble.on_star_word(move || {
            let Some(b) = weak_b.upgrade() else { return };
            let src = b.get_source_text().to_string();
            let trans = b.get_trans_text().to_string();
            if src.trim().is_empty() || trans.trim().is_empty() {
                b.set_notice_text("暂无译文".into());
                return;
            }
            let (src_lang, tgt_lang) = match weak_m.upgrade() {
                Some(m) => {
                    let (f, t) = resolve_langs(m.get_lang_from_index(), m.get_lang_to_index());
                    (f.code().to_string(), t.code().to_string())
                }
                None => ("en".to_string(), "zh".to_string()),
            };
            match cb.add_collect(&src, &trans, &src_lang, &tgt_lang, "划词", "", "未分级") {
                Ok(()) => {
                    info!("[ui] bubble star-word -> 已采集（{} 字符）", src.chars().count());
                    b.set_notice_text("已收藏".into());
                    // 入库即自动增强（音标/英英/词根/中文释义/分级，只补空）
                    crate::services::study::auto_enrich_new(tr_sw.clone(), rt_sw.clone(), src);
                }
                Err(e) => {
                    warn!("[ui] bubble star-word 失败: {e}");
                    b.set_notice_text("收藏失败".into());
                }
            }
        });
    }

    // 打开工作台：原文/译文回填主窗口双栏，显示主窗口并切到截图页，随后收起气泡。
    {
        let weak_b = bubble.as_weak();
        let weak_m = window.as_weak();
        let pos = bubble_pos.clone();
        bubble.on_open_workbench(move || {
            let (src, trans) = match weak_b.upgrade() {
                Some(b) => (b.get_source_text().to_string(), b.get_trans_text().to_string()),
                None => (String::new(), String::new()),
            };
            if let Some(m) = weak_m.upgrade() {
                m.set_ocr_src(src.into());
                m.set_ocr_out(trans.into());
                let _ = m.show();
                // 工作台始终显示，无需切页；关掉抽屉让工作台露出来
                m.set_drawer_type("".into());
                m.set_drawer_visible(false);
            }
            services::attach::overlay_place(BUBBLE_TITLE, BUBBLE_PARK, BUBBLE_PARK);
            pos.0.store(BUBBLE_PARK, Ordering::SeqCst);
            pos.1.store(BUBBLE_PARK, Ordering::SeqCst);
            info!("[ui] bubble open-workbench -> 回填主窗口并收起气泡");
        });
    }

    // 关闭：移出屏幕（不调用 hide()）。
    {
        let pos = bubble_pos.clone();
        bubble.on_close_bubble(move || {
            services::attach::overlay_place(BUBBLE_TITLE, BUBBLE_PARK, BUBBLE_PARK);
            pos.0.store(BUBBLE_PARK, Ordering::SeqCst);
            pos.1.store(BUBBLE_PARK, Ordering::SeqCst);
            info!("[ui] bubble close-bubble -> 移出屏幕");
        });
    }

    // ---- 划词监听服务（跨平台创建；非 Windows 上 start 为 no-op）----
    // 默认停用：attach 内部 enabled 初值为 false，必须由 UI 读配置后显式 set_enabled(true)。
    let attach_svc = Arc::new(services::attach::Attach::new());
    // 初始开关来自 config（旧 config.json 缺字段 → Config::load 回退默认 true）
    let cfg0 = crate::core::config::Config::load().unwrap_or_default();
    attach_svc.set_enabled(cfg0.selection_enabled);

    #[cfg(windows)]
    {
        use crate::services::attach::SelectionEvent;

        let weak_bubble_on = bubble.as_weak();
        let weak_main_on = window.as_weak();
        let armed = bubble_armed.clone();
        let pos = bubble_pos.clone();
        let tr_on = translate.clone();
        let rt_on = rt.clone();
        let cb_on = collect.clone();
        let pb_on = persist.clone();

        // on_event 在【后台工作线程】上被调用，回调里只做 clone + 投递，重活留给 UI 线程/后台线程。
        if let Err(e) = attach_svc.start(Box::new(move |ev: SelectionEvent| {
            match ev {
                SelectionEvent::ClickDown { x, y } => {
                    let weak = weak_bubble_on.clone();
                    let pos = pos.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let (bx, by) = (pos.0.load(Ordering::SeqCst), pos.1.load(Ordering::SeqCst));
                        if bx == BUBBLE_PARK {
                            return; // 已是隐藏态，什么都不做
                        }
                        // 命中判定用**实测物理矩形**：BUBBLE_W/H 是 Slint 逻辑像素，
                        // 高 DPI 缩放下实际窗口更大，算术矩形偏小 → 点气泡右下角会被误判成外部。
                        let inside = crate::services::attach::overlay_rect(BUBBLE_TITLE)
                            .map(|(l, t, r, bo)| x >= l && x <= r && y >= t && y <= bo)
                            // 兜底（量不到窗口时）：用逻辑尺寸粗判，宁可多留、不误收
                            .unwrap_or_else(|| {
                                x >= bx && x <= bx + BUBBLE_W && y >= by && y <= by + BUBBLE_H
                            });
                        if !inside {
                            crate::services::attach::overlay_place(
                                BUBBLE_TITLE,
                                BUBBLE_PARK,
                                BUBBLE_PARK,
                            );
                            pos.0.store(BUBBLE_PARK, Ordering::SeqCst);
                            pos.1.store(BUBBLE_PARK, Ordering::SeqCst);
                        }
                        let _ = weak.upgrade(); // 仅为保持 Weak 语义一致
                    });
                }
                SelectionEvent::Hit(hit) => {
                    let weak_b = weak_bubble_on.clone();
                    let weak_m = weak_main_on.clone();
                    let armed = armed.clone();
                    let pos = pos.clone();
                    let tr = tr_on.clone();
                    let rt = rt_on.clone();
                    let cb = cb_on.clone();
                    let pb = pb_on.clone();
                    // 回到 UI 线程：读主窗口语言对 + 显示气泡（含首次预热）+ 后台线程翻译
                    let _ = slint::invoke_from_event_loop(move || {
                        let (from, to, pair_label) = match weak_m.upgrade() {
                            Some(m) => {
                                let (f, t) = resolve_langs(
                                    m.get_lang_from_index(),
                                    m.get_lang_to_index(),
                                );
                                (
                                    f,
                                    t,
                                    format!(
                                        "{} → {}",
                                        f.code().to_uppercase(),
                                        t.code().to_uppercase()
                                    ),
                                )
                            }
                            None => (Lang::En, Lang::Zh, "EN → ZH".to_string()),
                        };

                        let src_full = hit.text.clone();
                        let src_show: String = src_full.chars().take(200).collect();
                        // 定位：默认落在选区右下；放不下则翻到上方 / 左移，保证气泡完全在屏内。
                        // fit_bubble_pos 内部用实测物理矩形（高 DPI 下逻辑尺寸会溢出屏幕）。
                        let (mut px, mut py) = (hit.x + 12, hit.y + 18);

                        if let Some(b) = weak_b.upgrade() {
                            b.set_source_text(src_show.into());
                            b.set_trans_text("".into());
                            b.set_status_text("翻译中…".into());
                            b.set_busy(true);
                            b.set_lang_pair(pair_label.into());
                            b.set_notice_text("".into()); // 清掉上一轮残留的复制/收藏反馈
                            if !armed.load(Ordering::SeqCst) {
                                // 首次：show() 会短暂抢焦点 → 打 ex-style + 量真实尺寸校正 +
                                // 定位 + 把焦点还给宿主。show() 之后窗口才存在（overlay_* 才生效）。
                                let prev = services::attach::foreground_hwnd();
                                let _ = b.show();
                                services::attach::overlay_make_nonactivating(BUBBLE_TITLE);
                                armed.store(true, Ordering::SeqCst);
                                // 先落到预估位置（避免在 (0,0) 闪一帧），再按实测尺寸校正并最终定位
                                let _ = services::attach::overlay_place(BUBBLE_TITLE, px, py);
                                let (nx, ny) = fit_bubble_pos(hit.x, hit.y, (BUBBLE_W, BUBBLE_H));
                                (px, py) = (nx, ny);
                                services::attach::overlay_place(BUBBLE_TITLE, px, py);
                                services::attach::restore_foreground(prev);
                                info!("[ui] 划词气泡首次显示 + 已置为不抢焦点浮层");
                            } else {
                                // 窗口已存在：直接按实测尺寸校正后定位
                                let (nx, ny) = fit_bubble_pos(hit.x, hit.y, (BUBBLE_W, BUBBLE_H));
                                (px, py) = (nx, ny);
                                services::attach::overlay_place(BUBBLE_TITLE, px, py);
                            }
                            pos.0.store(px, Ordering::SeqCst);
                            pos.1.store(py, Ordering::SeqCst);
                        }

                        // 翻译放后台线程（Translate 为 async，需 tokio runtime 驱动），避免阻塞 UI 事件循环
                        let weak_b2 = weak_b.clone();
                        let cb2 = cb.clone();
                        let pb2 = pb.clone();
                        let pair = (from, to);
                        // 高度自适应：译文回填后气泡会变高，需按锚点重新定位（防底部出屏）
                        let pos_r = pos.clone();
                        let (hit_x, hit_y) = (hit.x, hit.y);
                        std::thread::spawn(move || {
                            let r = rt.block_on(tr.translate(&src_full, pair.0, pair.1));
                            let weak = weak_b2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(b) = weak.upgrade() {
                                    match &r {
                                        Ok(res) => {
                                            b.set_trans_text(res.text.clone().into());
                                            b.set_busy(false);
                                            b.set_status_text("".into());
                                        }
                                        Err(e) => {
                                            b.set_busy(false);
                                            b.set_status_text(format!("翻译失败: {e:#}").into());
                                        }
                                    }
                                }
                                // 高度自适应跟随：气泡高度 = 内容 preferred（bubble.slint 无显式
                                // height），回填译文后窗口变高。延后一轮事件循环（等 winit 把新
                                // 尺寸落地），再按实测尺寸重新定位，防止变高后底部超出屏幕。
                                let pos_rr = pos_r.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    let (bx, by) = (
                                        pos_rr.0.load(Ordering::SeqCst),
                                        pos_rr.1.load(Ordering::SeqCst),
                                    );
                                    if bx != BUBBLE_PARK {
                                        let (nx, ny) =
                                            fit_bubble_pos(hit_x, hit_y, (BUBBLE_W, BUBBLE_H));
                                        if (nx, ny) != (bx, by) {
                                            crate::services::attach::overlay_place(
                                                BUBBLE_TITLE, nx, ny,
                                            );
                                            pos_rr.0.store(nx, Ordering::SeqCst);
                                            pos_rr.1.store(ny, Ordering::SeqCst);
                                        }
                                    }
                                });
                                // 落历史（仅追加写 JSONL，失败也不影响气泡）
                                if let Ok(res) = &r {
                                    let _ = pb2.record_translation(
                                        &src_full,
                                        &res.text,
                                        &res.engine,
                                        pair.0.code(),
                                        pair.1.code(),
                                    );
                                }
                                // 划词场景多为整句，不自动进采集队列；由用户点「采集」显式添加
                                let _ = &cb2;
                            });
                        });
                    });
                }
            }
        })) {
            warn!("[ui] 划词监听启动失败: {e:#}");
        } else {
            info!("[ui] 划词监听已启动（enabled={}）", attach_svc.enabled());
        }
    }

    // 非 Windows：划词监听不可用，仅日志；顺带消费仅 Windows 用到的项以避免 dead_code 警告
    // （气泡窗口本身必须跨平台存在，不能删）。
    #[cfg(not(windows))]
    {
        let _ = (&bubble_armed, BUBBLE_W, BUBBLE_H);
        info!("[ui] 非 Windows 平台：划词监听不可用（no-op）");
    }

    // 截图：UI 按钮 / Alt+Q 热键都汇到这里。
    // 蒙版+抓屏在独立线程阻塞执行（Win32 消息循环），随后同线程做系统 OCR，
    // 结果（状态 + OCR 文本）投递回 UI 线程更新状态条与左栏。
    {
        let weak = window.as_weak();
        let tr = translate.clone();
        let rtt = rt.clone();
        let ps = persist.clone();
        window.on_capture(move || {
            info!("[ui] capture triggered");
            // 重置取消标志（新操作开始）
            services::cancel::reset();
            let (from_idx, to_idx) = {
                let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
                (w.get_lang_from_index(), w.get_lang_to_index())
            };
            let weak = weak.clone();
            let tr = tr.clone();
            let rtt = rtt.clone();
            let ps = ps.clone();
            std::thread::spawn(move || {
                // Phase 3.3 骨架屏：进入处理中状态（OCR 骨架 + busy 指示）
                let weak_busy = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = weak_busy.upgrade() {
                        w.set_ocr_loading(true);
                        w.set_status_text("正在识别…".into());
                        // 骨架屏：busy 期间译文栏显示占位块，需先清空旧译文
                        w.set_ocr_out("".into());
                        w.set_busy(true);
                    }
                });
                match services::capture::capture_interactive() {
                    Ok(Some(r)) => {
                        let ocr = services::ocr::recognize_capture(&r.rgb, r.width as u32, r.height as u32);
                        let (status, text) = match &ocr {
                            Ok(t) => (
                                format!("OCR 完成 · {} 字符", t.chars().count()),
                                Some(t.clone()),
                            ),
                            Err(e) => (format!("OCR 失败: {e:#}"), None),
                        };
                        info!("[ui] ocr -> {}", status);
                        let ocr_text = text.clone();
                        let weak_ocr = weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = weak_ocr.upgrade() {
                                // 结果第一时间可见：弹出/置前主窗口并停双栏页
                                bring_main_to_front(&win);
                                win.set_status_text(status.into());
                                win.set_ocr_loading(false);
                                if let Some(t) = text {
                                    win.set_ocr_src(t.into());
                                } else {
                                    // OCR 失败：退出 busy（无翻译可做）
                                    win.set_busy(false);
                                }
                            }
                        });
                        // 翻译：后台线程 block_on 异步翻译服务
                        if let Some(src) = ocr_text {
                            let (from, to) = resolve_langs(from_idx, to_idx);
                            // 翻译前检查取消
                            if services::cancel::is_cancelled() {
                                let weak_cancel = weak.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = weak_cancel.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text("翻译已取消".into());
                                    }
                                });
                                return;
                            }
                            log::info!(
                                "[ui] translate -> 调用 ({} 字符, from={} to={})",
                                src.chars().count(),
                                from.code(),
                                to.code()
                            );
                            match rtt.block_on(tr.translate(&src, from, to)) {
                                Ok(res) => {
                                    log::info!("[ui] translate -> 成功 (引擎 {})", res.engine);
                                    let out = res.text;
                                    let eng = res.engine;
                                    // 自动写入历史记录（每次截图=一条原文+译文）
                                    ps.record_translation(
                                        &src,
                                        &out,
                                        &eng,
                                        from.code(),
                                        to.code(),
                                    );
                                    // 注意：生词本不再自动灌入整段译文。
                                    // 生词=单词合集，由用户在双栏页「选中原文单词→加入生词」或在生词页手动添加。
                                    let weak_tx = weak.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = weak_tx.upgrade() {
                                            win.set_ocr_out(out.clone().into());
                                            win.set_busy(false);
                                            win.set_status_text(
                                                format!(
                                                    "翻译完成 · {} 字符 · 引擎 {}",
                                                    out.chars().count(),
                                                    eng
                                                )
                                                .into(),
                                            );
                                        }
                                    });
                                }
                                Err(e) => {
                                    log::warn!("[ui] translate -> 失败: {e:#}");
                                    let weak_tx = weak.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = weak_tx.upgrade() {
                                            win.set_busy(false);
                                            win.set_status_text(format!("翻译失败: {e:#}").into());
                                        }
                                    });
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        info!("[ui] capture canceled (Esc / 过小选区)");
                        // 取消截图：退出骨架/busy 状态
                        let weak_cancel = weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = weak_cancel.upgrade() {
                                win.set_ocr_loading(false);
                                win.set_busy(false);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("[ui] capture failed: {e:#}");
                        let weak_err = weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = weak_err.upgrade() {
                                win.set_ocr_loading(false);
                                win.set_busy(false);
                                win.set_status_text(format!("截图失败: {e:#}").into());
                            }
                        });
                    }
                }
            });
        });
    }
    // v2.0 抽屉/模态回调注册（替代旧 on-navigate 切页逻辑）
    // 工作台始终显示；点击导航轨 → open-drawer；点击齿轮 → open-modal

    {
        let weak = window.as_weak();
        let cb_load = collect.clone();
        window.on_open_drawer(move |dtype: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                let s = dtype.to_string();
                w.set_drawer_type(dtype);
                w.set_drawer_visible(true);
                info!("[ui] open-drawer -> {s}");
                if s == "collect" {
                    // 进入生词本抽屉时自动加载数据（迁移自旧 on-navigate）
                    let project = w.get_collect_filter_tag().to_string();
                    let items = cb_load.get_by_project(&project);
                    let lines: Vec<slint::SharedString> = items
                        .iter()
                        .map(|i| format_collect_row(i).into())
                        .collect();
                    let ids: Vec<slint::SharedString> = items.iter().map(|i| slint::SharedString::from(i.id.clone())).collect();
                    w.set_collect_rows(slint::ModelRc::new(slint::VecModel::from(lines)));
                    w.set_collect_ids(slint::ModelRc::new(slint::VecModel::from(ids)));
                    let (total, _) = cb_load.stats();
                    w.set_collect_total(total as i32);
                    let projects = cb_load.get_tags();
                    let proj_str: Vec<slint::SharedString> = projects.iter().map(|p| slint::SharedString::from(p.clone())).collect();
                    w.set_collect_tags(slint::ModelRc::new(slint::VecModel::from(proj_str)));
                    w.invoke_load_ignore();
                } else if s == "history" {
                    w.invoke_load_history();
                }
                // 注：学习页已迁出抽屉（drawer-type "review" 不复存在），
                // Rail「学习」按钮直接走 on_open_study 打开独立 StudyWindow 窗口。
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_close_drawer(move || {
            if let Some(w) = weak.upgrade() {
                w.set_drawer_visible(false);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_open_modal(move |mtype: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                w.set_modal_type(mtype);
                w.set_modal_visible(true);
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_close_modal(move || {
            if let Some(w) = weak.upgrade() {
                w.set_modal_visible(false);
            }
        });
    }

    // 四卡片学习工作台：打开 / 评分 / TTS / 查词典 / 切换卡片
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        let tr_ow = translate.clone();
        let rt_ow = rt.clone();
        window.on_open_word_card(move |idx: i32| {
            if let Some(w) = weak.upgrade() {
                let ids_model = w.get_collect_ids();
                let id = ids_model
                    .row_data(idx as usize)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    return;
                }
                let items = cb.list_all();
                let Some(item) = items.iter().find(|i| i.id == id) else {
                    return;
                };
                // 填充四卡片数据
                w.set_word_card_word(item.word.clone().into());
                w.set_word_card_phonetic(item.phonetic.clone().into());
                w.set_word_card_definition(item.definition_en.clone().into());
                w.set_word_card_translation(item.translation.clone().into());
                w.set_word_card_example(item.example.clone().into());
                w.set_word_card_example_zh(item.example_zh.clone().into());
                w.set_word_card_roots(item.roots.clone().into());
                w.set_word_card_etymology(item.etymology.clone().into());
                w.set_word_card_level(item.level.clone().into());
                w.set_word_card_project(item.project.clone().into());
                w.set_word_card_context(item.context.clone().into());
                w.set_word_card_mastery(item.mastery as i32);
                w.set_word_card_next_review(format_time(item.next_review).into());
                w.set_word_card_active_tab(0);
                w.set_modal_type("word-card".into());
                w.set_modal_visible(true);
                info!("[ui] open-word-card -> {}", item.word);

                // 「学习 = 查字典」：打开词卡即自动后台查询词典全文 + 补音标 + 补中文释义
                // （enrich 链负责词根/词源；本链负责完整释义与 Argos 中文，均只补空不覆盖）
                spawn_card_enrich(weak.clone(), item.word.clone(), tr_ow.clone(), rt_ow.clone());

                // 若学习字段缺失，异步用 enrich 服务回填（词典/音标/词源数据就绪时）
                let needs_enrich = item.phonetic.is_empty()
                    || item.definition_en.is_empty()
                    || item.roots.is_empty()
                    || item.etymology.is_empty();
                if needs_enrich {
                    let weak2 = weak.clone();
                    let cb2 = cb.clone();
                    let word = item.word.clone();
                    let item_id = item.id.clone();
                    std::thread::spawn(move || {
                        let enrich = crate::services::enrich::Enrich::new();
                        let enriched = enrich.enrich(&word).ok().flatten();
                        if let Some(e) = enriched {
                            if let Err(err) = cb2.update_enrichment(
                                &item_id,
                                &e.phonetic,
                                &e.definition_en,
                                &e.example,
                                &e.example_zh,
                                &e.roots,
                                &e.etymology,
                            ) {
                                log::warn!("[ui] enrich 回填失败 {}: {err}", item_id);
                            }
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = weak2.upgrade() {
                                    win.set_word_card_phonetic(e.phonetic.into());
                                    win.set_word_card_definition(e.definition_en.into());
                                    win.set_word_card_example(e.example.into());
                                    win.set_word_card_example_zh(e.example_zh.into());
                                    win.set_word_card_roots(e.roots.into());
                                    win.set_word_card_etymology(e.etymology.into());
                                }
                            });
                        }
                    });
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        window.on_word_card_grade(move |quality: i32| {
            if let Some(w) = weak.upgrade() {
                let word = w.get_word_card_word().to_string();
                if word.is_empty() {
                    return;
                }
                let items = cb.list_all();
                let Some(item) = items.iter().find(|i| i.word.eq_ignore_ascii_case(&word)) else {
                    return;
                };
                let q = quality.clamp(0, 5) as u8;
                match cb.grade_collect(&item.id, q) {
                    Ok(()) => {
                        // 刷新数据
                        let items = cb.list_all();
                        let Some(item2) = items.iter().find(|i| i.id == item.id) else {
                            return;
                        };
                        w.set_word_card_mastery(item2.mastery as i32);
                        w.set_word_card_next_review(format_time(item2.next_review).into());
                        w.invoke_load_collect();
                        info!("[ui] word-card-grade {} q={}", item.word, q);
                    }
                    Err(e) => log::warn!("[ui] word-card-grade 失败: {e}"),
                }
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_word_card_play_tts(move || {
            if let Some(w) = weak.upgrade() {
                let word = w.get_word_card_word().to_string();
                if word.trim().is_empty() {
                    return;
                }
                // TTS 回退链（edge-tts → onecore → sapi），发音用 en-US
                let weak2 = weak.clone();
                std::thread::spawn(move || {
                    let word2 = word.clone();
                    crate::services::tts::speak_async(word, "en-US".to_string());
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = weak2.upgrade() {
                            win.set_status_text(format!("已朗读: {word2}").into());
                        }
                    });
                });
            }
        });
    }
    {
        let weak_ld = window.as_weak();
        let tr_ld = translate.clone();
        let rt_ld = rt.clone();
        window.on_word_card_lookup_dict(move || {
            if let Some(w) = weak_ld.upgrade() {
                let word = w.get_word_card_word().to_string();
                if word.trim().is_empty() {
                    return;
                }
                w.set_status_text(format!("正在查询词典: {word}").into());
                spawn_card_enrich(weak_ld.clone(), word, tr_ld.clone(), rt_ld.clone());
            }
        });
    }
    {
        let weak = window.as_weak();
        window.on_word_card_set_tab(move |tab: i32| {
            if let Some(w) = weak.upgrade() {
                w.set_word_card_active_tab(tab.clamp(0, 3));
            }
        });
    }

    // 历史记录：加载（从 persist JSONL 读真数据）
    let weak_hist = window.as_weak();
    let ps_hist = persist.clone();
    window.on_load_history(move || {
        if let Some(win) = weak_hist.upgrade() {
            info!("[ui] load-history");
            match ps_hist.load_history() {
                Ok(items) => {
                    let lines: Vec<slint::SharedString> = items
                        .iter()
                        .map(|item| {
                            let ts = format_time(item.ts);
                            // 每张卡片：元信息 + 完整原文 + 完整译文（不再截断）
                            let proj = if item.project.is_empty() {
                                String::new()
                            } else {
                                format!(" · 项目 {}", item.project)
                            };
                            format!(
                                "[{}] 引擎 {} · {}→{}{proj}\n原文: {}\n译文: {}",
                                ts, item.engine, item.src_lang, item.tgt_lang, item.src, item.tgt
                            )
                            .into()
                        })
                        .collect();
                    let model = slint::ModelRc::new(slint::VecModel::from(lines));
                    win.set_history_rows(model);
                }
                Err(e) => {
                    warn!("加载历史失败: {e}");
                    let model = slint::ModelRc::new(slint::VecModel::from(vec!["加载失败".to_string().into()]));
                    win.set_history_rows(model);
                }
            }
        }
    });

    // 历史记录：清空
    let weak_clear = window.as_weak();
    let ps_clear = persist.clone();
    window.on_clear_history(move || {
        if let Some(win) = weak_clear.upgrade() {
            info!("[ui] clear-history");
            if let Err(e) = ps_clear.clear_history() {
                warn!("清空历史失败: {e}");
            }
            let model = slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new()));
            win.set_history_rows(model);
        }
    });

    // 生词本（P2）：加载 + 渲染
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        window.on_load_collect(move || {
            if let Some(w) = weak.upgrade() {
                info!("[ui] load-collect");
                let project = w.get_collect_filter_tag().to_string();
                let items = cb.get_by_project(&project);
                let lines: Vec<String> = items
                    .iter()
                    .map(format_collect_row)
                    .collect();
                let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
                let sl: Vec<slint::SharedString> = lines.into_iter().map(|s| s.into()).collect();
                w.set_collect_rows(slint::ModelRc::new(slint::VecModel::from(sl)));
                let si: Vec<slint::SharedString> = ids.into_iter().map(|s| s.into()).collect();
                w.set_collect_ids(slint::ModelRc::new(slint::VecModel::from(si)));
                let (total, _due) = cb.stats();
                w.set_collect_total(total as i32);
                let projects = cb.get_tags();
                let proj_str: Vec<slint::SharedString> =
                    projects.iter().map(|p| slint::SharedString::from(p.clone())).collect();
                w.set_collect_tags(slint::ModelRc::new(slint::VecModel::from(proj_str)));
            }
        });
    }
    // ============================================================
    // 单词学习窗口（独立 StudyWindow）回调接线
    //   队列刷新 / 开始 / 翻面 / 评分 / 发音 / 上下词 / 单词引入
    // 入口：Rail「学习」按钮 on_open_study；引入走 services::study 接口。
    // ============================================================
    {
        let cb = collect.clone();
        let tr = translate.clone();
        let rt = rt.clone();
        let main_weak = window.as_weak();
        // 会话内队列快照：评分会更新该词 next_review，若每次重查队列会丢失剩余词
        let queue: std::rc::Rc<std::cell::RefCell<Vec<crate::services::collect::CollectItem>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

        // 组别过滤后的全量快照（分类切换时在此重筛，避免反复查库）
        let all_stage: std::rc::Rc<std::cell::RefCell<Vec<crate::services::collect::CollectItem>>> =
            std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));

        // ---- 打开窗口（Rail 学习按钮）----
        let weak_o = study.as_weak();
        window.on_open_study(move || {
            if let Some(s) = weak_o.upgrade() {
                s.invoke_refresh();
                let _ = s.show();
            }
        });

        // ---- 刷新：队列 + 角标 + 摘要 ----
        let weak_r = study.as_weak();
        let cb_r = cb.clone();
        let q_r = queue.clone();
        let main_r = main_weak.clone();
        let all_r = all_stage.clone();
        study.on_refresh(move || {
            if let Some(s) = weak_r.upgrade() {
                // 每次打开同步一次 UI 缩放（设置页改了缩放后无需重启）
                let cfg_s = crate::core::config::Config::load().unwrap_or_default();
                s.set_ui_scale_pct(if (75..=200).contains(&cfg_s.ui_scale_pct) {
                    cfg_s.ui_scale_pct
                } else {
                    100
                });
                let mut all = cb_r.list_all();
                // 学习组别过滤（2026-10-17 学段词表）：队列只显示所选组别的词；
                // due/stock 保持全表统计（库存/到期总量），组别只管"学哪些"。
                let before = all.len();
                all = crate::services::stage::apply_stage(all, &cfg_s.study_stage);
                info!(
                    "[ui::study] 队列组别过滤: {} → {} 词 [{}]",
                    before,
                    all.len(),
                    cfg_s.study_stage
                );
                let (stock, due) = cb_r.stats();
                s.set_due(due as i32);
                s.set_stock(stock as i32);
                // 分类计数（2026-10-17 分类选学：未学/学习中/待复习）
                s.set_cnt_new(all.iter().filter(|i| i.study_status() == 0).count() as i32);
                s.set_cnt_learn(all.iter().filter(|i| i.study_status() == 1).count() as i32);
                s.set_cnt_review(all.iter().filter(|i| i.study_status() == 2).count() as i32);
                *all_r.borrow_mut() = all.clone();
                // 按当前选中分类重建可见队列（保留用户上次选的 Tab）
                let items = study_filter_cat(all, s.get_cat_idx());
                // 主窗 Rail 角标同步
                if let Some(m) = main_r.upgrade() {
                    m.set_review_badge(if due > 99 {
                        "99+".into()
                    } else if due > 0 {
                        due.to_string().into()
                    } else {
                        "".into()
                    });
                }
                s.set_rows(study_build_rows(&items));
                *q_r.borrow_mut() = items;
                s.set_phase(0);
                s.set_idx(-1);
            }
        });

        // ---- 分类切换（2026-10-17 分类选学）：按 Tab 重建可见队列快照 ----
        let weak_c = study.as_weak();
        let all_c = all_stage.clone();
        let q_c = queue.clone();
        study.on_set_cat(move |cat| {
            if let Some(s) = weak_c.upgrade() {
                let items = study_filter_cat(all_c.borrow().clone(), cat);
                s.set_rows(study_build_rows(&items));
                *q_c.borrow_mut() = items;
            }
        });

        // ---- 队列行点击（Rust 侧判双击：450ms 内同一行两击 → 单卡直学）----
        // 学完自动接原队列下一张（picked 移到队首，后续 idx 顺延原列表）
        let weak_p = study.as_weak();
        let q_p = queue.clone();
        let last_click: std::rc::Rc<
            std::cell::RefCell<Option<(usize, std::time::Instant)>>,
        > = std::rc::Rc::new(std::cell::RefCell::new(None));
        let last_c = last_click.clone();
        study.on_row_click(move |idx| {
            let now = std::time::Instant::now();
            let is_double = {
                let mut last = last_c.borrow_mut();
                let hit = matches!(&*last, Some((i, t)) if *i == idx as usize && now.duration_since(*t).as_millis() < 450);
                if hit {
                    *last = None;
                } else {
                    *last = Some((idx as usize, now));
                }
                hit
            };
            if !is_double {
                return;
            }
            if let Some(s) = weak_p.upgrade() {
                let mut q = q_p.borrow_mut();
                let n = q.len();
                if n == 0 || (idx as usize) >= n {
                    return;
                }
                let item = q.remove(idx as usize);
                q.insert(0, item);
                s.set_total(n as i32);
                s.set_done(0);
                s.set_idx(0);
                let item = q[0].clone();
                drop(q);
                study_load_word(&s, &item);
                s.set_phase(1);
                info!("[ui] study-pick: 双击单卡直学");
            }
        });

        // ---- 开始复习 ----
        let weak_s = study.as_weak();
        let q_s = queue.clone();
        study.on_start_session(move || {
            if let Some(s) = weak_s.upgrade() {
                let n = q_s.borrow().len();
                if n == 0 {
                    return;
                }
                s.set_total(n as i32);
                s.set_done(0);
                s.set_idx(0);
                let item = q_s.borrow()[0].clone();
                study_load_word(&s, &item);
                s.set_phase(1);
                info!("[ui] study-start: {n} words");
            }
        });

        // ---- 翻面（双向：正面⇄答案面，2026-10-17 用户反馈补返回）----
        let weak_f = study.as_weak();
        study.on_flip(move || {
            if let Some(s) = weak_f.upgrade() {
                match s.get_phase() {
                    1 => s.set_phase(2),
                    2 => s.set_phase(1),
                    _ => {}
                }
            }
        });

        // ---- SM-2 评分（忘记0/模糊3/记得4/容易5）----
        let weak_g = study.as_weak();
        let cb_g = cb.clone();
        let q_g = queue.clone();
        study.on_grade(move |q: i32| {
            if let Some(s) = weak_g.upgrade() {
                let idx = s.get_idx();
                if idx < 0 {
                    return;
                }
                let item = match q_g.borrow().get(idx as usize) {
                    Some(it) => it.clone(),
                    None => return,
                };
                if let Err(e) = cb_g.grade_collect(&item.id, q.clamp(0, 5) as u8) {
                    warn!("[ui] study grade failed: {e:#}");
                }
                info!("[ui] study-grade {} q={q}", item.word);
                let done = s.get_done() + 1;
                s.set_done(done);
                let next = idx + 1;
                let n = q_g.borrow().len() as i32;
                if next >= n {
                    // 队列走完：回摘要页并刷新（被评分词 next_review 已推后，自动出队）
                    s.invoke_refresh();
                } else {
                    s.set_idx(next);
                    let it = q_g.borrow()[next as usize].clone();
                    study_load_word(&s, &it);
                    s.set_phase(1);
                }
            }
        });

        // ---- 发音（0=单词 1=慢速 2=英英释义 3=例句 4=采集原句）----
        let weak_t = study.as_weak();
        study.on_tts(move |kind: i32| {
            if let Some(s) = weak_t.upgrade() {
                let word = s.get_word().to_string();
                let (text, rate) = match kind {
                    1 => (word.clone(), -4),
                    2 => (s.get_definition().to_string(), 0),
                    3 => (s.get_example().to_string(), 0),
                    4 => (s.get_context().to_string(), 0),
                    _ => (word.clone(), 0),
                };
                if text.trim().is_empty() {
                    return;
                }
                std::thread::spawn(move || {
                    crate::services::tts::speak_async_rate(text.clone(), "en-US".to_string(), rate);
                });
            }
        });

        // ---- ◀ 上一词（回看）----
        let weak_p = study.as_weak();
        let q_p = queue.clone();
        study.on_prev(move || {
            if let Some(s) = weak_p.upgrade() {
                let idx = s.get_idx();
                if idx > 0 {
                    s.set_idx(idx - 1);
                    let it = q_p.borrow()[(idx - 1) as usize].clone();
                    study_load_word(&s, &it);
                    s.set_phase(1);
                }
            }
        });

        // ---- 下一词（跳过不评分）----
        let weak_n = study.as_weak();
        let q_n = queue.clone();
        study.on_next(move || {
            if let Some(s) = weak_n.upgrade() {
                let idx = s.get_idx();
                let n = q_n.borrow().len() as i32;
                if idx >= 0 && idx + 1 < n {
                    s.set_idx(idx + 1);
                    let it = q_n.borrow()[(idx + 1) as usize].clone();
                    study_load_word(&s, &it);
                    s.set_phase(1);
                }
            }
        });

        // ---- 单词引入（顶栏输入框回车 / 引入按钮）----
        // 唯一 UI 入口 → services::study::introduce_word（归一化+查重+分级+source）
        // 新词立即出卡；词典增强与中文释义后台跑，完成后词卡仍显示同词才回填。
        let weak_i = study.as_weak();
        let cb_i = cb.clone();
        let q_i = queue.clone();
        let tr_i = tr.clone();
        let rt_i = rt.clone();
        study.on_introduce(move |raw: slint::SharedString| {
            let Some(s) = weak_i.upgrade() else {
                return;
            };
            let text = raw.trim().to_string();
            if text.is_empty() {
                s.set_intro_status("请输入单词".into());
                return;
            }
            let word = match crate::services::study::normalize_word(&text) {
                Some(w) => w,
                None => {
                    s.set_intro_status(format!("不是合法英文单词: {text}").into());
                    return;
                }
            };
            let intro = match crate::services::study::introduce_word(&cb_i, &word, "", "manual") {
                Ok(it) => it,
                Err(e) => {
                    s.set_intro_status(format!("引入失败: {e:#}").into());
                    return;
                }
            };
            // 重新读取库内条目（分级/来源已回写）
            let item = cb_i
                .list_all()
                .into_iter()
                .find(|i| i.word.eq_ignore_ascii_case(&intro.item.word))
                .unwrap_or_else(|| intro.item.clone());
            // 会话处理：无会话 → 单词会话；进行中 → 追加队尾并跳到该词
            if s.get_phase() == 0 || s.get_idx() < 0 {
                *q_i.borrow_mut() = vec![item.clone()];
                s.set_total(1);
                s.set_done(0);
                s.set_idx(0);
            } else {
                let mut q = q_i.borrow_mut();
                q.push(item.clone());
                let n = q.len() as i32;
                s.set_total(n);
                s.set_idx(n - 1);
            }
            study_load_word(&s, &item);
            s.set_phase(1);
            s.set_intro_status(if intro.is_new {
                format!("已引入: {}（词典查询中…）", item.word).into()
            } else {
                format!("已在生词本: {}", item.word).into()
            });
            s.set_intro_input("".into());
            // 后台词典增强（音标/英英/词根/例句）
            let weak_e = weak_i.clone();
            let cb_e = cb_i.clone();
            let word_e = item.word.clone();
            crate::services::study::enrich_async(word_e.clone(), move |w| {
                let weak_e2 = weak_e.clone();
                let cb_e2 = cb_e.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(s) = weak_e2.upgrade() {
                        if s.get_word().eq_ignore_ascii_case(&w) {
                            if let Some(it) = cb_e2
                                .list_all()
                                .into_iter()
                                .find(|i| i.word.eq_ignore_ascii_case(&w))
                            {
                                study_load_word(&s, &it);
                            }
                        }
                    }
                });
            });
            // 后台中文释义（空才翻 Argos）
            let weak_z = weak_i.clone();
            let cb_z = cb_i.clone();
            let word_z = item.word.clone();
            let tr_z = tr_i.clone();
            let rt_z = rt_i.clone();
            crate::services::study::translate_zh_async(word_z.clone(), tr_z, rt_z, move |w, zh| {
                if zh.is_empty() {
                    return;
                }
                let weak_z2 = weak_z.clone();
                let cb_z2 = cb_z.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(s) = weak_z2.upgrade() {
                        if s.get_word().eq_ignore_ascii_case(&w)
                            && s.get_trans_lines().trim().is_empty()
                        {
                            if let Some(it) = cb_z2
                                .list_all()
                                .into_iter()
                                .find(|i| i.word.eq_ignore_ascii_case(&w))
                            {
                                study_load_word(&s, &it);
                            }
                        }
                    }
                });
            });
        });

        // ---- 一键补全生词内容（批量回填存量词条，2026-10-17）----
        let weak_b = study.as_weak();
        let tr_b = tr.clone();
        let rt_b = rt.clone();
        study.on_backfill(move || {
            let Some(s) = weak_b.upgrade() else {
                return;
            };
            if s.get_backfilling() {
                return; // 防重入
            }
            s.set_backfilling(true);
            s.set_backfill_status("准备补全…".into());
            let weak_p = weak_b.clone();
            let weak_f = weak_b.clone();
            let tr_b2 = tr_b.clone();
            let rt_b2 = rt_b.clone();
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                let res = crate::services::study::backfill_all(tr_b2, rt_b2, move |done, total, word| {
                    let wp = weak_p.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(s) = wp.upgrade() {
                            if word.is_empty() {
                                s.set_backfill_status("扫描完成".into());
                            } else {
                                s.set_backfill_status(format!("补全中 {}/{}：{}", done + 1, total, word).into());
                            }
                        }
                    });
                });
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(s) = weak_f.upgrade() {
                        s.set_backfilling(false);
                        match res {
                            Ok(r) => s.set_backfill_status(
                                format!("补全完成：扫描 {} 词，更新 {} 条（{:.0?}）", r.scanned, r.updated, started.elapsed())
                                    .into(),
                            ),
                            Err(e) => s.set_backfill_status(format!("补全失败: {e:#}").into()),
                        }
                        s.invoke_refresh();
                    }
                });
            });
        });

        // ---- 关闭窗口 ----
        let weak_c = study.as_weak();
        study.on_close_requested(move || {
            if let Some(s) = weak_c.upgrade() {
                let _ = s.hide();
            }
        });
    }
    // 启动时初始化复习角标（到期数），否则要点开学习窗口才出现
    {
        let (_stock, due) = collect.stats();
        window.set_review_badge(if due > 99 {
            "99+".into()
        } else if due > 0 {
            due.to_string().into()
        } else {
            "".into()
        });
    }
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        let tr_ac = translate.clone();
        let rt_ac = rt.clone();
        window.on_add_collect(move || {
            if let Some(w) = weak.upgrade() {
                let word = w.get_collect_add_src().to_string();
                let translation = w.get_collect_add_tgt().to_string();
                let project = w.get_collect_filter_tag().to_string();
                // 释义允许为空（2026-10-06 用户需求：生字条双击即可入本，释义后补）。
                if word.trim().is_empty() {
                    w.set_collect_status("原文不能为空".into());
                    return;
                }
                match cb.add_collect(&word, &translation, "auto", "auto", &project, "", "未分级") {
                    Ok(()) => {
                        w.set_collect_add_src("".into());
                        w.set_collect_add_tgt("".into());
                        // 释义为空时明确告知"已入本、待补释义"，避免用户以为没保存成功。
                        let msg = if translation.trim().is_empty() {
                            format!("已采集（待补释义）: {word}")
                        } else {
                            format!("已采集: {word} → {translation}")
                        };
                        w.set_collect_status(msg.into());
                        w.invoke_load_collect();
                        // 入库即自动增强（音标/英英/词根/中文释义/分级，只补空）
                        crate::services::study::auto_enrich_new(tr_ac.clone(), rt_ac.clone(), word);
                    }
                    Err(e) => w.set_collect_status(format!("采集失败: {e}").into()),
                }
            }
        });
    }
    // 候选词：ocr-src 变化时重算（截图/粘贴/打开图片/重识别 都走这里）
    {
        let weak = window.as_weak();
        window.on_recompute_candidates(move || {
            if let Some(w) = weak.upgrade() {
                let src = w.get_ocr_src().to_string();
                let cfg = crate::core::config::Config::load().unwrap_or_default();
                let cands = extract_candidates(&src, &cfg.ignore_words);
                let cm: Vec<slint::SharedString> =
                    cands.iter().map(|s| s.as_str().into()).collect();
                w.set_collect_candidates(slint::ModelRc::new(slint::VecModel::from(cm)));

                // Phase 3.5：按宽度预算分行（Slint 无自动换行容器，且单行不换行会撑破
                // 中央舞台容器 —— 表现为候选词被窗口右缘裁掉 + 翻译栏按钮被推出可视区）。
                let stage_w = w.window().size().width as f32;
                let (rows, overflow) = layout_candidate_rows(&cands, stage_w);
                let row_models: Vec<slint::ModelRc<slint::SharedString>> = rows
                    .into_iter()
                    .map(|r| {
                        let v: Vec<slint::SharedString> =
                            r.into_iter().map(|s| s.into()).collect();
                        slint::ModelRc::new(slint::VecModel::from(v))
                    })
                    .collect();
                w.set_collect_candidate_rows(slint::ModelRc::new(slint::VecModel::from(row_models)));
                w.set_collect_candidate_overflow(overflow.into());
            }
        });
    }
    // 候选词 → 预填到添加表单（单击；不自动落库，释义留空供用户补全）
    {
        let weak = window.as_weak();
        window.on_add_candidate(move |word: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                let word = word.trim().to_string();
                if word.is_empty() {
                    return;
                }
                w.set_collect_add_src(word.clone().into());
                w.set_collect_add_tgt("".into());
                w.set_collect_status(
                    format!("已填入候选词：{word}，请在上方补全释义后点「添加」").into(),
                );
                w.set_status_text(format!("已填入候选词：{word}（请补全释义）").into());
            }
        });
    }
    // 候选词 → 双击直接入生词本（2026-10-06 用户需求：「生字条双击就能保存」）
    // 单击仍走 add-candidate 预填表单；双击直接落库，释义留待后续补。
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        let tr_sc = translate.clone();
        let rt_sc = rt.clone();
        window.on_save_candidate(move |word: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                let word = word.trim().to_string();
                if word.is_empty() {
                    return;
                }
                let project = w.get_collect_filter_tag().to_string();
                match cb.add_collect(&word, "", "auto", "auto", &project, "", "未分级") {
                    Ok(()) => {
                        w.set_collect_status(format!("已收入生词本（待补释义）: {word}").into());
                        w.set_status_text(format!("已收入生词本: {word}").into());
                        // 刷新抽屉计数与列表（若抽屉正开着）
                        w.invoke_load_collect();
                        // 入库即自动增强（音标/英英/词根/中文释义/分级，只补空）
                        crate::services::study::auto_enrich_new(tr_sc.clone(), rt_sc.clone(), word);
                    }
                    Err(e) => w.set_collect_status(format!("入库失败: {e}").into()),
                }
            }
        });
    }
    // 忽略词：加载到 UI
    {
        let weak = window.as_weak();
        window.on_load_ignore(move || {
            if let Some(w) = weak.upgrade() {
                let cfg = crate::core::config::Config::load().unwrap_or_default();
                let im: Vec<slint::SharedString> = cfg
                    .ignore_words
                    .iter()
                    .map(|s| s.clone().into())
                    .collect();
                w.set_ignore_words(slint::ModelRc::new(slint::VecModel::from(im)));
            }
        });
    }
    // 忽略词：新增
    {
        let weak = window.as_weak();
        window.on_add_ignore(move |word: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                let word = word.trim().to_lowercase();
                if word.is_empty() {
                    w.set_ignore_status("词不能为空".into());
                    return;
                }
                let mut cfg = match crate::core::config::Config::load() {
                    Ok(c) => c,
                    Err(e) => {
                        w.set_ignore_status(format!("读取配置失败: {e}").into());
                        return;
                    }
                };
                if cfg.ignore_words.iter().any(|x| x.eq_ignore_ascii_case(&word)) {
                    w.set_ignore_status(format!("「{word}」已在忽略词表中").into());
                    return;
                }
                cfg.ignore_words.push(word.clone());
                match cfg.save() {
                    Ok(()) => {
                        w.set_ignore_input("".into());
                        w.set_ignore_status(format!("已添加忽略词：{word}").into());
                        let im: Vec<slint::SharedString> = cfg
                            .ignore_words
                            .iter()
                            .map(|s| s.clone().into())
                            .collect();
                        w.set_ignore_words(slint::ModelRc::new(slint::VecModel::from(im)));
                        w.invoke_recompute_candidates();
                    }
                    Err(e) => w.set_ignore_status(format!("保存失败: {e}").into()),
                }
            }
        });
    }
    // 忽略词：删除
    {
        let weak = window.as_weak();
        window.on_remove_ignore(move |word: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                let word = word.trim().to_string();
                let mut cfg = match crate::core::config::Config::load() {
                    Ok(c) => c,
                    Err(e) => {
                        w.set_ignore_status(format!("读取配置失败: {e}").into());
                        return;
                    }
                };
                let before = cfg.ignore_words.len();
                cfg.ignore_words.retain(|x| !x.eq_ignore_ascii_case(&word));
                if cfg.ignore_words.len() == before {
                    w.set_ignore_status(format!("「{word}」不在忽略词表中").into());
                    return;
                }
                match cfg.save() {
                    Ok(()) => {
                        w.set_ignore_status(format!("已删除忽略词：{word}").into());
                        let im: Vec<slint::SharedString> = cfg
                            .ignore_words
                            .iter()
                            .map(|s| s.clone().into())
                            .collect();
                        w.set_ignore_words(slint::ModelRc::new(slint::VecModel::from(im)));
                        w.invoke_recompute_candidates();
                    }
                    Err(e) => w.set_ignore_status(format!("保存失败: {e}").into()),
                }
            }
        });
    }
    // grade_collect 回调已移除（v2.0 不再有 SM-2 评分按钮）
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        window.on_remove_collect(move |idx: i32| {
            if let Some(w) = weak.upgrade() {
                let ids_model = w.get_collect_ids();
                if idx < 0 || idx as usize >= ids_model.row_count() {
                    return;
                }
                if let Some(id_s) = ids_model.row_data(idx as usize) {
                    let id = id_s.to_string();
                    if let Err(e) = cb.remove_collect(&id) {
                        warn!("删除失败: {e}");
                    } else {
                        w.invoke_load_collect();
                    }
                }
            }
        });
    }
    // 批量补全释义：后台线程逐条跑 Argos（en→zh）回填空 translation；
    // 顺带给「未分级」词条重标 CEFR（classify::cefr_tier 含词形归一）。
    // 写库走 update_translation / update_level_if_ungraded（均只补空/未分级，不覆盖已有值）。
    {
        let weak_bf = window.as_weak();
        let cb_bf = collect.clone();
        let tr_bf = translate.clone();
        let rt_bf = rt.clone();
        window.on_backfill_collect(move || {
            let weak2 = weak_bf.clone();
            let cb2 = cb_bf.clone();
            let tr2 = tr_bf.clone();
            let rt2 = rt_bf.clone();
            std::thread::spawn(move || {
                let items = cb2.list_all();
                let todo: Vec<_> = items
                    .iter()
                    .filter(|i| i.translation.trim().is_empty() || i.level == "未分级")
                    .collect();
                let total = todo.len();
                if total == 0 {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = weak2.upgrade() {
                            win.set_collect_status("没有需要补全的词条（释义与分级均已就绪）".into());
                        }
                    });
                    return;
                }
                info!("[ui] backfill-collect 开始: {total} 条");
                let mut done = 0usize;
                for item in &todo {
                    // 1) 中文释义：空 translation → Argos 单词翻译（逐条串行，避免引擎过载）
                    if item.translation.trim().is_empty() {
                        match rt2.block_on(tr2.translate(&item.word, Lang::En, Lang::Zh)) {
                            Ok(res) => {
                                let txt = res.text.trim().to_string();
                                if !txt.is_empty() {
                                    if let Err(e) = cb2.update_translation(&item.id, &txt) {
                                        warn!("[ui] backfill 写入 {} 失败: {e:#}", item.word);
                                    }
                                }
                            }
                            Err(e) => warn!("[ui] backfill 翻译 {} 失败: {e:#}", item.word),
                        }
                    }
                    // 2) 分级：未分级 → CEFR 重标
                    if item.level == "未分级" {
                        let lv = services::classify::cefr_tier(&item.word);
                        if lv != "未分级" {
                            let _ = cb2.update_level_if_ungraded(&item.id, lv);
                        }
                    }
                    done += 1;
                    if done % 10 == 0 || done == total {
                        let msg = format!("补全释义中… {done}/{total}");
                        let wc = weak2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_collect_status(msg.into());
                            }
                        });
                    }
                }
                info!("[ui] backfill-collect 完成: {total} 条");
                let wc = weak2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = wc.upgrade() {
                        win.set_collect_status(format!("补全完成：共处理 {total} 条").into());
                        win.invoke_load_collect();
                    }
                });
            });
        });
    }
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        window.on_refresh_collect_tags(move || {
            if let Some(w) = weak.upgrade() {
                let projects = cb.get_tags();
                let proj_str: Vec<slint::SharedString> =
                    projects.iter().map(|p| slint::SharedString::from(p.clone())).collect();
                w.set_collect_tags(slint::ModelRc::new(slint::VecModel::from(proj_str)));
            }
        });
    }

    // 导出（P3）：刷新项目 + 执行导出
    {
        let weak = window.as_weak();
        let cb = collect.clone();
        window.on_refresh_export_projects(move || {
            if let Some(w) = weak.upgrade() {
                let mut projects = cb.get_tags();
                if !projects.contains(&"全部".to_string()) {
                    projects.insert(0, "全部".to_string());
                }
                let proj_str: Vec<slint::SharedString> =
                    projects.iter().map(|p| slint::SharedString::from(p.clone())).collect();
                w.set_export_projects(slint::ModelRc::new(slint::VecModel::from(proj_str)));
            }
        });
    }
    {
        let weak = window.as_weak();
        let export_svc = export_svc.clone();
        let cb = collect.clone();
        let ps = persist.clone();
        window.on_export_file(move || {
            if let Some(w) = weak.upgrade() {
                let target = w.get_export_target();
                let fmt = w.get_export_format();
                let project = w.get_export_project().to_string();
                // 对齐 slint 导出模态：0=Anki TSV, 1=Markdown, 2=欧路 XML, 3=纯文本
                let format = match fmt {
                    0 => ExportFormat::AnkiTsv,
                    2 => ExportFormat::EudicXml,
                    3 => ExportFormat::PlainText,
                    _ => ExportFormat::Markdown,
                };
                // 收集数据
                let proj_filter = project.trim().to_string();
                match target {
                    0 => {
                        // 历史记录
                        let all = match ps.load_history() {
                            Ok(v) => v,
                            Err(e) => {
                                w.set_export_status(format!("加载失败: {e}").into());
                                return;
                            }
                        };
                        let filtered = if proj_filter == "全部" || proj_filter.is_empty() {
                            all
                        } else {
                            all.into_iter().filter(|h| h.project == proj_filter).collect()
                        };
                        let path = PathBuf::from(format!(
                            "winocr_history.{}",
                            format.extension()
                        ));
                        match export_svc.export_history(&filtered, format, &path) {
                            Ok(()) => w.set_export_status(format!("已导出 {}", path.display()).into()),
                            Err(e) => w.set_export_status(format!("导出失败: {e}").into()),
                        }
                    }
                    1 => {
                        // 采集队列
                        let items = if proj_filter == "全部" || proj_filter.is_empty() {
                            cb.list_all()
                        } else {
                            cb.get_by_project(&proj_filter)
                        };
                        let path = PathBuf::from(format!(
                            "winocr_collect.{}",
                            format.extension()
                        ));
                        match export_svc.export_collect(&items, format, &path) {
                            Ok(()) => w.set_export_status(format!("已导出 {}", path.display()).into()),
                            Err(e) => w.set_export_status(format!("导出失败: {e}").into()),
                        }
                    }
                    _ => {}
                }
            }
        });
    }

    // 语言对互换（⇄）：from/to 语义交换，并映射到各自 ComboBox model 的合法 index。
    // 注意两个 model 不对称：from 无「简体中文」、to 无「自动检测」，退化到最近合法项。
    window.on_swap_lang({
        let weak = window.as_weak();
        move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let (fi, ti) = (w.get_lang_from_index(), w.get_lang_to_index());
            let (from, to) = resolve_langs(fi, ti);
            // 交换：新 from = 旧 to，新 to = 旧 from
            w.set_lang_from_index(lang_to_from_idx(to));
            w.set_lang_to_index(lang_to_to_idx(from));
            info!("[ui] swap-lang -> from={} to={}", to, from);
        }
    });

    // AI 对话：发送消息 → 流式接收
    {
        let ai = ai.clone();
        let rt = rt.clone();
        let weak = window.as_weak();
        window.on_send_ai(move |text: slint::SharedString| {
            let msg = text.as_str().to_string();
            if msg.is_empty() {
                return;
            }
            info!("[ui] ai send -> {}", msg);

            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            // 追加 user 消息
            let mut msgs: Vec<slint::SharedString> = w.get_ai_messages().iter().collect();
            msgs.push(msg.clone().into());
            let model = slint::ModelRc::new(slint::VecModel::from(
                msgs.iter().map(|s| slint::SharedString::from(s.clone())).collect::<Vec<_>>(),
            ));
            w.set_ai_messages(model);

            // 同步追加 user role
            let mut roles: Vec<slint::SharedString> = w.get_ai_msg_roles().iter().collect();
            roles.push("user".into());
            let role_model = slint::ModelRc::new(slint::VecModel::from(
                roles.iter().map(|s| slint::SharedString::from(s.clone())).collect::<Vec<_>>(),
            ));
            w.set_ai_msg_roles(role_model);

            let w_reply = weak.clone();
            let w_final = weak.clone();
            let w_ctx = weak.clone(); // 专用于读取 OCR 上下文（避免 move 进 async 块）
            let ai = ai.clone();
            let rt = rt.clone();
            std::thread::spawn(move || {
                let _ = rt.block_on(async move {
                    // 读取当前 OCR 上下文（截图原文/译文），注入 AI 对话
                    let (ocr_src, ocr_tgt) = if let Some(win) = w_ctx.upgrade() {
                        (Some(win.get_ocr_src().to_string()), Some(win.get_ocr_out().to_string()))
                    } else {
                        (None, None)
                    };
                    let cb = move |chunk: String| {
                        let w = w_reply.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = w.upgrade() {
                                let mut cur = win.get_ai_streaming().to_string();
                                cur.push_str(&chunk);
                                win.set_ai_streaming(cur.into());
                            }
                        });
                    };
                    ai.chat_stream_with_context(ocr_src, ocr_tgt, msg, cb).await
                });
                // 流式完成：把 streaming 追加到 messages，清空 streaming
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = w_final.upgrade() {
                        let streaming = win.get_ai_streaming();
                        if !streaming.is_empty() {
                            let mut msgs = win.get_ai_messages().iter().collect::<Vec<_>>();
                            msgs.push(streaming);
                            let model = slint::ModelRc::new(slint::VecModel::from(
                                msgs.iter().map(|s| slint::SharedString::from(s.clone())).collect::<Vec<_>>(),
                            ));
                            win.set_ai_messages(model);

                            // 同步追加 assistant role
                            let mut roles = win.get_ai_msg_roles().iter().collect::<Vec<_>>();
                            roles.push("assistant".into());
                            let role_model = slint::ModelRc::new(slint::VecModel::from(
                                roles.iter().map(|s| slint::SharedString::from(s.clone())).collect::<Vec<_>>(),
                            ));
                            win.set_ai_msg_roles(role_model);
                        }
                        win.set_ai_streaming("".into());
                    }
                });
            });
        });
    }

    // AI 对话：清空上下文
    {
        let ai = ai.clone();
        let weak = window.as_weak();
        window.on_clear_ai(move || {
            ai.clear_context();
            if let Some(win) = weak.upgrade() {
                let model = slint::ModelRc::new(slint::VecModel::from(
                    vec!["欢迎使用 AI 助手！".to_string()].into_iter().map(|s| s.into()).collect::<Vec<_>>(),
                ));
                win.set_ai_messages(model);
                let role_model = slint::ModelRc::new(slint::VecModel::from(
                    vec!["assistant".to_string()].into_iter().map(|s| s.into()).collect::<Vec<_>>(),
                ));
                win.set_ai_msg_roles(role_model);
                win.set_ai_streaming("".into());
                info!("[ui] ai cleared");
            }
        });
    }

    {
        let weak = window.as_weak();
        window.on_minimize(move || {
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
                info!("[ui] minimize -> hide to tray");
            }
        });
    }

    // ---- 新按钮回调（真实实现）----
    // 文件打开：文件对话框 → 加载图片 → OCR → 自动翻译
    {
        let weak = window.as_weak();
        let tr = translate.clone();
        let rtt = rt.clone();
        window.on_open_file(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let path = rfd::FileDialog::new()
                .add_filter("图片", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tiff"])
                .add_filter("所有文件", &["*"])
                .pick_file();
            let Some(path) = path else { return };
            let path_str = path.display().to_string();
            w.set_status_text(format!("正在识别 {} …", path_str).into());
            // Phase 3.3 骨架屏：进入处理中状态
            w.set_ocr_loading(true);
            // 骨架屏：busy 期间译文栏显示占位块，需先清空旧译文
            w.set_ocr_out("".into());
            w.set_busy(true);
            let weak2 = weak.clone();
            let tr2 = tr.clone();
            let rtt2 = rtt.clone();
            std::thread::spawn(move || {
                match services::ocr::recognize_file(&path) {
                    Ok(text) => {
                        let t = text.clone();
                        let txt_count = text.chars().count();
                        let wc = weak2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_ocr_loading(false);
                                win.set_ocr_src(t.into());
                                win.set_status_text(format!("OCR 完成 · {} 字符", txt_count).into());
                            }
                        });
                        let (from, to) = match weak2.upgrade() {
                            Some(w3) => resolve_langs(w3.get_lang_from_index(), w3.get_lang_to_index()),
                            None => {
                                let wc = weak2.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text("翻译跳过：窗口已关闭".into());
                                    }
                                });
                                return;
                            }
                        };
                        match rtt2.block_on(tr2.translate(&text, from, to)) {
                            Ok(res) => {
                                let txt_count = text.chars().count();
                                let wc2 = weak2.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc2.upgrade() {
                                        win.set_ocr_out(res.text.into());
                                        win.set_busy(false);
                                        win.set_status_text(format!("翻译完成 · {} 字符 · 引擎 {}", txt_count, res.engine).into());
                                    }
                                });
                            }
                            Err(e) => {
                                let wc2 = weak2.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc2.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text(format!("翻译失败: {e:#}").into());
                                    }
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let wc = weak2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_ocr_loading(false);
                                win.set_busy(false);
                                win.set_status_text(format!("OCR 失败: {e:#}").into());
                            }
                        });
                    }
                }
            });
        });
    }

    // 在线图片：显示 URL 弹窗
    {
        let weak = window.as_weak();
        window.on_open_url(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            w.set_url_dialog_visible(true);
        });
    }

    // URL 弹窗提交（在后台线程下载+OCR+翻译）
    {
        let weak = window.as_weak();
        let tr = translate.clone();
        let rtt = rt.clone();
        window.on_url_dialog_submit(move |url: slint::SharedString| {
            let url = url.as_str().to_string();
            if url.trim().is_empty() {
                return;
            }
            let w2 = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            w2.set_url_dialog_visible(false);
            w2.set_status_text(format!("正在下载 {} …", url).into());
            // Phase 3.3 骨架屏：下载+OCR 阶段
            w2.set_ocr_loading(true);
            // 骨架屏：busy 期间译文栏显示占位块，需先清空旧译文
            w2.set_ocr_out("".into());
            w2.set_busy(true);
            let weak3 = weak.clone();
            let tr2 = tr.clone();
            let rtt2 = rtt.clone();
            std::thread::spawn(move || {
                match services::ocr::recognize_url(&url) {
                    Ok(text) => {
                        let t = text.clone();
                        let txt = text.clone();
                        let wc = weak3.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_ocr_loading(false);
                                win.set_ocr_src(t.into());
                                win.set_status_text(format!("OCR 完成 · {} 字符", txt.chars().count()).into());
                            }
                        });
                        let (from, to) = match weak3.upgrade() {
                            Some(w4) => resolve_langs(w4.get_lang_from_index(), w4.get_lang_to_index()),
                            None => {
                                let wc = weak3.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text("翻译跳过：窗口已关闭".into());
                                    }
                                });
                                return;
                            }
                        };
                        let txt_count = text.chars().count();
                        match rtt2.block_on(tr2.translate(&text, from, to)) {
                            Ok(res) => {
                                let wc2 = weak3.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc2.upgrade() {
                                        win.set_ocr_out(res.text.into());
                                        win.set_busy(false);
                                        win.set_status_text(format!("翻译完成 · {} 字符 · 引擎 {}", txt_count, res.engine).into());
                                    }
                                });
                            }
                            Err(e) => {
                                let wc2 = weak3.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc2.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text(format!("翻译失败: {e:#}").into());
                                    }
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let wc = weak3.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_ocr_loading(false);
                                win.set_busy(false);
                                win.set_status_text(format!("下载/识别失败: {e:#}").into());
                            }
                        });
                    }
                }
            });
        });
    }

    // 粘贴剪贴板：读剪贴板 → 有图片则 OCR，有文本则直接填入原文
    {
        let weak = window.as_weak();
        let tr = translate.clone();
        let rtt = rt.clone();
        window.on_paste_clipboard(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            // 1) 尝试从剪贴板读取图片
            let img = services::clipboard::get_image();
            if let Some(rgb) = img {
                w.set_status_text("正在识别剪贴板图片…".into());
                // Phase 3.3 骨架屏：进入处理中状态
                w.set_ocr_loading(true);
                // 骨架屏：busy 期间译文栏显示占位块，需先清空旧译文
                w.set_ocr_out("".into());
                w.set_busy(true);
                let weak2 = weak.clone();
                let tr2 = tr.clone();
                let rtt2 = rtt.clone();
                std::thread::spawn(move || {
                    let (w_u8, w_u32, h_u32) = rgb;
                    match services::ocr::recognize_capture(&w_u8, w_u32, h_u32) {
                        Ok(text) => {
                            let t = text.clone();
                            let txt = text.clone();
                            let wc = weak2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = wc.upgrade() {
                                    win.set_ocr_src(t.into());
                                    win.set_ocr_loading(false);
                                    win.set_status_text(format!("OCR 完成 · {} 字符", txt.chars().count()).into());
                                }
                            });
                            let (from, to) = match weak2.upgrade() {
                                Some(w3) => resolve_langs(w3.get_lang_from_index(), w3.get_lang_to_index()),
                                None => {
                                    let wc = weak2.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = wc.upgrade() {
                                            win.set_busy(false);
                                            win.set_status_text("翻译跳过：窗口已关闭".into());
                                        }
                                    });
                                    return;
                                }
                            };
                            let txt_count = text.chars().count();
                            match rtt2.block_on(tr2.translate(&text, from, to)) {
                                Ok(res) => {
                                    let wc2 = weak2.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = wc2.upgrade() {
                                            win.set_ocr_out(res.text.into());
                                        win.set_busy(false);
                                            win.set_status_text(format!("翻译完成 · {} 字符 · 引擎 {}", txt_count, res.engine).into());
                                        }
                                    });
                                }
                                Err(e) => {
                                    let wc2 = weak2.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(win) = wc2.upgrade() {
                                        win.set_busy(false);
                                            win.set_status_text(format!("翻译失败: {e:#}").into());
                                        }
                                    });
                                }
                            }
                        }
                        Err(e) => {
                            let wc = weak2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = wc.upgrade() {
                                    win.set_ocr_loading(false);
                                    win.set_busy(false);
                                    win.set_status_text(format!("OCR 失败: {e:#}").into());
                                }
                            });
                        }
                    }
                });
                return;
            }
            // 2) 尝试从剪贴板读取文本
            if let Some(text) = services::clipboard::get_text() {
                let t = text.clone();
                w.set_ocr_src(t.clone().into());
                w.set_status_text(format!("已粘贴 {} 字符", t.chars().count()).into());
                let weak2 = weak.clone();
                let tr2 = tr.clone();
                let rtt2 = rtt.clone();
                std::thread::spawn(move || {
                    let (from, to) = match weak2.upgrade() {
                        Some(w3) => resolve_langs(w3.get_lang_from_index(), w3.get_lang_to_index()),
                        None => {
                            let wc = weak2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = wc.upgrade() {
                                    win.set_status_text("翻译跳过：窗口已关闭".into());
                                }
                            });
                            return;
                        }
                    };
                    match rtt2.block_on(tr2.translate(&text, from, to)) {
                        Ok(res) => {
                            let wc = weak2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = wc.upgrade() {
                                    win.set_ocr_out(res.text.into());
                                    win.set_status_text(format!("翻译完成 · {} 字符 · 引擎 {}", t.chars().count(), res.engine).into());
                                }
                            });
                        }
                        Err(e) => {
                            let wc = weak2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = wc.upgrade() {
                                    win.set_status_text(format!("翻译失败: {e:#}").into());
                                }
                            });
                        }
                    }
                });
                return;
            }
            w.set_status_text("剪贴板为空（无图片/无文本）".into());
        });
    }

    // 重新识别：对最后截图的 rgb 数据重跑 OCR（不重新截图）
    {
        let weak = window.as_weak();
        let tr = translate.clone();
        let rtt = rt.clone();
        window.on_re_ocr(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let last = services::capture::last_capture();
            let Some(cap) = last else {
                w.set_status_text("没有可重新识别的截图（请先截图）".into());
                return;
            };
            w.set_status_text("正在重新识别…".into());
            // 骨架屏：busy 期间译文栏显示占位块，需先清空旧译文
            w.set_ocr_out("".into());
            w.set_busy(true);
            let weak2 = weak.clone();
            let tr2 = tr.clone();
            let rtt2 = rtt.clone();
            std::thread::spawn(move || {
                match services::ocr::recognize_capture(&cap.rgb, cap.width as u32, cap.height as u32) {
                    Ok(text) => {
                        let t = text.clone();
                        let txt = text.clone();
                        let wc = weak2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_ocr_src(t.into());
                                win.set_ocr_loading(false);
                                win.set_status_text(format!("重新识别完成 · {} 字符", txt.chars().count()).into());
                            }
                        });
                        let (from, to) = match weak2.upgrade() {
                            Some(w3) => resolve_langs(w3.get_lang_from_index(), w3.get_lang_to_index()),
                            None => {
                                let wc = weak2.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text("翻译跳过：窗口已关闭".into());
                                    }
                                });
                                return;
                            }
                        };
                        match rtt2.block_on(tr2.translate(&text, from, to)) {
                            Ok(res) => {
                                let txt_count = text.chars().count();
                                let wc2 = weak2.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc2.upgrade() {
                                        win.set_ocr_out(res.text.into());
                                        win.set_busy(false);
                                        win.set_status_text(format!("翻译完成 · {} 字符 · 引擎 {}", txt_count, res.engine).into());
                                    }
                                });
                            }
                            Err(e) => {
                                let wc2 = weak2.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(win) = wc2.upgrade() {
                                        win.set_busy(false);
                                        win.set_status_text(format!("翻译失败: {e:#}").into());
                                    }
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let wc = weak2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = wc.upgrade() {
                                win.set_busy(false);
                                win.set_status_text(format!("重新识别失败: {e:#}").into());
                            }
                        });
                    }
                }
            });
        });
    }

    // 朗读译文：TTS（services::tts 回退链：edge-tts → onecore → sapi）
    {
        let weak = window.as_weak();
        window.on_speak_out(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let text = w.get_ocr_out().to_string();
            if text.trim().is_empty() {
                w.set_status_text("没有可朗读的译文".into());
                return;
            }
            w.set_status_text("正在朗读…".into());
            let weak2 = weak.clone();
            std::thread::spawn(move || {
                crate::services::tts::speak_async(text, "zh-CN".to_string());
                let wc = weak2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = wc.upgrade() {
                        win.set_status_text("朗读完成".into());
                    }
                });
            });
        });
    }

    // 加入生词（选中即收藏）：Slint 端点击时先 save-clipboard 存原剪贴板，
    // 再 ocr-src-edit.copy() 把选区写入剪贴板，最后本回调读取选区文本。
    // 选区文本即「单词」，填入生词页的单词输入框并跳转到生词页，由用户补全释义后点「添加」。
    let clip_save = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    {
        let clip_save_c = clip_save.clone();
        window.on_save_clipboard(move || {
            *clip_save_c.borrow_mut() = crate::services::clipboard::get_text();
        });
    }
    {
        let weak = window.as_weak();
        let clip_save_c = clip_save.clone();
        window.on_add_word_from_selection(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let selected = crate::services::clipboard::get_text();
            let saved = clip_save_c.borrow().clone();
            // 还原用户原剪贴板（避免被选区覆盖）
            if let Some(ref orig) = saved {
                let _ = crate::services::clipboard::set_text(orig);
            }
            match selected {
                Some(word) if !word.trim().is_empty() && saved.as_deref() != Some(word.trim()) => {
                    let word = word.trim().to_string();
                    w.set_collect_add_src(word.clone().into());
                    w.set_collect_add_tgt("".into());
                    w.set_drawer_type("collect".into());
                    w.set_drawer_visible(true);
                    w.set_collect_status(
                        format!("已填入单词：{word}，请在采集抽屉补全释义后点「添加」").into(),
                    );
                    w.set_status_text(format!("已采集单词：{word}（请补全释义）").into());
                }
                _ => {
                    w.set_status_text("请先在原文中选中要采集的单词，再点「采集」".into());
                }
            }
        });
    }

    // 取消操作：设置取消标志（截图/翻译检查此标志）
    {
        let weak = window.as_weak();
        window.on_cancel_operation(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            services::cancel::request_cancel();
            w.set_status_text("已请求取消".into());
            w.set_status_progress(0);
        });
    }
    // 术语库：自定义术语增删（P1-2），实时对运行期 Translate 生效
    {
        let weak_add = window.as_weak();
        let tr = translate.clone();
        let tr_add = tr.clone();
        window.on_add_term(move || {
            if let Some(w) = weak_add.upgrade() {
                let src = w.get_term_custom_src().to_string();
                let tgt = w.get_term_custom_tgt().to_string();
                if src.is_empty() || tgt.is_empty() {
                    w.set_term_manage_status("源词与目标词均不能为空".into());
                    return;
                }
                let line = format!("{} → {}", src, tgt);
                let model = w.get_term_custom_lines();
                let mut lines: Vec<String> = (0..model.row_count())
                    .filter_map(|i| model.row_data(i).map(|s| s.to_string()))
                    .collect();
                if lines.iter().any(|l| l == &line) {
                    w.set_term_manage_status("该词条已存在".into());
                    return;
                }
                lines.push(line.clone());
                let ss: Vec<slint::SharedString> = lines.into_iter().map(|s| s.into()).collect();
                w.set_term_custom_lines(slint::ModelRc::new(slint::VecModel::from(ss)));
                tr_add.add_custom_term(&src, &tgt);
                w.set_term_custom_src("".into());
                w.set_term_custom_tgt("".into());
                w.set_term_manage_status((format!("已添加：{}", line)).into());
            }
        });
        let weak_remove = window.as_weak();
        let tr_remove = translate.clone();
        window.on_remove_term(move |idx: i32| {
            if let Some(w) = weak_remove.upgrade() {
                let model = w.get_term_custom_lines();
                let mut lines: Vec<String> = (0..model.row_count())
                    .filter_map(|i| model.row_data(i).map(|s| s.to_string()))
                    .collect();
                if idx < 0 || idx as usize >= lines.len() {
                    return;
                }
                let removed = lines.remove(idx as usize);
                if let Some((src, _)) = removed.split_once(" → ") {
                    tr_remove.remove_custom_term(src);
                }
                let ss: Vec<slint::SharedString> = lines.into_iter().map(|s| s.into()).collect();
                w.set_term_custom_lines(slint::ModelRc::new(slint::VecModel::from(ss)));
                w.set_term_manage_status((format!("已删除：{}", removed)).into());
            }
        });
    }

    // 启动时初始化主题（必须在 show 之前，确保首帧即正确着色）
    {
        let cfg0 = crate::core::config::Config::load().unwrap_or_default();
        let pal = crate::core::config::ThemePalette::by_name(&cfg0.theme);
        window.set_text(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.text[1..3], 16).unwrap_or(0x33),
            u8::from_str_radix(&pal.text[3..5], 16).unwrap_or(0x41),
            u8::from_str_radix(&pal.text[5..7], 16).unwrap_or(0x55),
        ));
        window.set_text_2(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.text_2[1..3], 16).unwrap_or(0x64),
            u8::from_str_radix(&pal.text_2[3..5], 16).unwrap_or(0x74),
            u8::from_str_radix(&pal.text_2[5..7], 16).unwrap_or(0x8B),
        ));
        window.set_text_3(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.text_3[1..3], 16).unwrap_or(0x94),
            u8::from_str_radix(&pal.text_3[3..5], 16).unwrap_or(0xA3),
            u8::from_str_radix(&pal.text_3[5..7], 16).unwrap_or(0xB8),
        ));
        window.set_bg(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.bg[1..3], 16).unwrap_or(0xF8),
            u8::from_str_radix(&pal.bg[3..5], 16).unwrap_or(0xFA),
            u8::from_str_radix(&pal.bg[5..7], 16).unwrap_or(0xFC),
        ));
        window.set_surface(slint::Color::from_rgb_u8(0xFF, 0xFF, 0xFF));
        window.set_surface_2(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.surface_2[1..3], 16).unwrap_or(0xF8),
            u8::from_str_radix(&pal.surface_2[3..5], 16).unwrap_or(0xFA),
            u8::from_str_radix(&pal.surface_2[5..7], 16).unwrap_or(0xFC),
        ));
        window.set_border(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.border[1..3], 16).unwrap_or(0xE2),
            u8::from_str_radix(&pal.border[3..5], 16).unwrap_or(0xE8),
            u8::from_str_radix(&pal.border[5..7], 16).unwrap_or(0xF0),
        ));
        window.set_accent(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.accent[1..3], 16).unwrap_or(0x0E),
            u8::from_str_radix(&pal.accent[3..5], 16).unwrap_or(0xA5),
            u8::from_str_radix(&pal.accent[5..7], 16).unwrap_or(0xE9),
        ));
        window.set_accent_2(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.accent_2[1..3], 16).unwrap_or(0x7D),
            u8::from_str_radix(&pal.accent_2[3..5], 16).unwrap_or(0xD3),
            u8::from_str_radix(&pal.accent_2[5..7], 16).unwrap_or(0xFC),
        ));
        window.set_accent_grad(slint::Brush::from(slint::Color::from_rgb_u8(
            u8::from_str_radix(&pal.accent[1..3], 16).unwrap_or(0x0E),
            u8::from_str_radix(&pal.accent[3..5], 16).unwrap_or(0xA5),
            u8::from_str_radix(&pal.accent[5..7], 16).unwrap_or(0xE9),
        )));
        window.set_current_theme(cfg0.theme.clone().into());
        info!("[ui] startup theme -> {}", cfg0.theme);
        // Phase 3.4：恢复对照区排列方向
        window.set_split_vertical(cfg0.split_vertical);
        info!(
            "[ui] startup split -> {}",
            if cfg0.split_vertical { "上下对照" } else { "左右对照" }
        );
        // 2026-10-06：恢复界面缩放与字体族（slint 侧 default-font-size / default-font-family
        // 已分别绑定到 ui-scale-pct / ui-font-family，这里只需回填属性）。
        {
            let pct = if (75..=200).contains(&cfg0.ui_scale_pct) {
                cfg0.ui_scale_pct
            } else {
                100
            };
            window.set_ui_scale_pct(pct);
            window.set_setting_ui_scale_idx(
                UI_SCALE_OPTIONS
                    .iter()
                    .position(|&v| v == pct)
                    .map(|i| i as i32)
                    .unwrap_or(1),
            );
            let fam = cfg0.ui_font_family.as_str();
            let fidx = UI_FONT_OPTIONS
                .iter()
                .position(|&v| v == fam)
                .map(|i| i as i32)
                .unwrap_or(0);
            window.set_ui_font_family(fam.into());
            window.set_setting_ui_font_idx(fidx);
            info!("[ui] startup ui-scale={pct}% font={fam:?}");
        }
    }

    // 设置：加载当前配置到 UI
    {
        let weak = window.as_weak();
        // 初始化设置页字段
        if let Some(win) = weak.upgrade() {
            let cfg = crate::core::config::Config::load().unwrap_or_default();
            let idx = match cfg.translate_engine.as_str() {
                "argos" => 0,
                "ollama" => 1,
                "sf" => 2,
                "mymemory" => 3,
                "youdao" => 4,
                _ => 0,
            };
            win.set_setting_engine_idx(idx);
            win.set_setting_sf_key(cfg.sf_key.into());
            win.set_setting_ollama_url(cfg.ollama_url.into());
            win.set_setting_ollama_model(cfg.ollama_model.into());
            // AI 后端
            let ai_idx = match cfg.ai_backend.as_str() {
                "siliconflow" | "sf" => 0,
                "ollama" => 1,
                _ => 0,
            };
            win.set_setting_ai_backend_idx(ai_idx);
            win.set_setting_ai_sf_key(cfg.ai_sf_key.into());
            // 术语库（P1-2）：启用开关 + 自定义词条列表
            win.set_setting_term_enabled(cfg.term_enabled);
            // 划词翻译（P3-3）
            win.set_setting_selection_enabled(cfg.selection_enabled);
            // 学习词典来源（2026-10-17 词典切换入口）
            win.set_setting_dict_idx(match cfg.study_dict.as_str() {
                "ecdict" => 1,
                _ => 0,
            });
            // 学习组别（学段词表 + 自定义 vocab/*.txt，动态填充下拉）
            let stage_opts = crate::services::stage::options();
            let stage_ss: Vec<slint::SharedString> =
                stage_opts.iter().map(|s| s.clone().into()).collect();
            win.set_stage_options(slint::ModelRc::new(slint::VecModel::from(stage_ss)));
            win.set_setting_stage_idx(
                stage_opts
                    .iter()
                    .position(|s| *s == cfg.study_stage)
                    .unwrap_or(0) as i32,
            );
            // 气泡字号档位：回填设置页 + 实时应用到气泡窗口
            win.set_setting_bubble_font_idx(cfg.bubble_font_idx);
            bubble.set_font_preset(cfg.bubble_font_idx);
            // 主题：设置页显示当前主题
            win.set_setting_theme_name(cfg.theme.clone().into());
            let custom_lines: Vec<String> = cfg
                .custom_terms
                .iter()
                .map(|(s, t)| format!("{} → {}", s, t))
                .collect();
            let ss: Vec<slint::SharedString> = custom_lines.into_iter().map(|s| s.into()).collect();
            win.set_term_custom_lines(slint::ModelRc::new(slint::VecModel::from(ss)));
        }
    }

    // 主题切换：apply-theme 回调 → 更新所有颜色令牌 + 持久化
    {
        let weak = window.as_weak();
        window.on_apply_theme(move |theme: slint::SharedString| {
            let t = theme.to_string();
            if let Some(w) = weak.upgrade() {
                let pal = crate::core::config::ThemePalette::by_name(&t);
                w.set_text(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.text[1..3], 16).unwrap_or(0x33),
                    u8::from_str_radix(&pal.text[3..5], 16).unwrap_or(0x41),
                    u8::from_str_radix(&pal.text[5..7], 16).unwrap_or(0x55),
                ));
                w.set_text_2(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.text_2[1..3], 16).unwrap_or(0x64),
                    u8::from_str_radix(&pal.text_2[3..5], 16).unwrap_or(0x74),
                    u8::from_str_radix(&pal.text_2[5..7], 16).unwrap_or(0x8B),
                ));
                w.set_text_3(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.text_3[1..3], 16).unwrap_or(0x94),
                    u8::from_str_radix(&pal.text_3[3..5], 16).unwrap_or(0xA3),
                    u8::from_str_radix(&pal.text_3[5..7], 16).unwrap_or(0xB8),
                ));
                w.set_bg(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.bg[1..3], 16).unwrap_or(0xF8),
                    u8::from_str_radix(&pal.bg[3..5], 16).unwrap_or(0xFA),
                    u8::from_str_radix(&pal.bg[5..7], 16).unwrap_or(0xFC),
                ));
                w.set_surface(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.surface[1..3], 16).unwrap_or(0xFF),
                    u8::from_str_radix(&pal.surface[3..5], 16).unwrap_or(0xFF),
                    u8::from_str_radix(&pal.surface[5..7], 16).unwrap_or(0xFF),
                ));
                w.set_surface_2(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.surface_2[1..3], 16).unwrap_or(0xF8),
                    u8::from_str_radix(&pal.surface_2[3..5], 16).unwrap_or(0xFA),
                    u8::from_str_radix(&pal.surface_2[5..7], 16).unwrap_or(0xFC),
                ));
                w.set_border(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.border[1..3], 16).unwrap_or(0xE2),
                    u8::from_str_radix(&pal.border[3..5], 16).unwrap_or(0xE8),
                    u8::from_str_radix(&pal.border[5..7], 16).unwrap_or(0xF0),
                ));
                w.set_accent(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.accent[1..3], 16).unwrap_or(0x0E),
                    u8::from_str_radix(&pal.accent[3..5], 16).unwrap_or(0xA5),
                    u8::from_str_radix(&pal.accent[5..7], 16).unwrap_or(0xE9),
                ));
                w.set_accent_2(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.accent_2[1..3], 16).unwrap_or(0x7D),
                    u8::from_str_radix(&pal.accent_2[3..5], 16).unwrap_or(0xD3),
                    u8::from_str_radix(&pal.accent_2[5..7], 16).unwrap_or(0xFC),
                ));
                w.set_accent_grad(slint::Brush::from(slint::Color::from_rgb_u8(
                    u8::from_str_radix(&pal.accent[1..3], 16).unwrap_or(0x0E),
                    u8::from_str_radix(&pal.accent[3..5], 16).unwrap_or(0xA5),
                    u8::from_str_radix(&pal.accent[5..7], 16).unwrap_or(0xE9),
                )));
                w.set_current_theme(t.clone().into());

                // 持久化主题
                if let Ok(mut cfg) = crate::core::config::Config::load() {
                    cfg.theme = t.to_string();
                    let _ = cfg.save();
                }

                info!("[ui] theme applied -> {t}");
            }
        });
    }

    // 设置页：change-theme → 触发 apply-theme
    {
        let weak = window.as_weak();
        window.on_change_theme(move |theme: slint::SharedString| {
            if let Some(w) = weak.upgrade() {
                w.set_setting_theme_name(theme.clone());
                w.invoke_apply_theme(theme);
            }
        });
    }

    // ===== 界面缩放（2026-10-06 用户需求：字体大小可调）=====
    // 机制：main_window.slint 里所有字号都用 rem 声明，rem 由 Window 的
    // default-font-size 换算，而 default-font-size = ui-base-size * ui-scale-pct / 100。
    // 故 Rust 侧只需改 ui-scale-pct 一个整数，整套界面字号随之缩放。
    // 即时生效并落盘，用户无需重启。
    {
        let weak = window.as_weak();
        window.on_apply_ui_scale(move |idx: i32| {
            if let Some(w) = weak.upgrade() {
                let pct = UI_SCALE_OPTIONS.get(idx as usize).copied().unwrap_or(100);
                w.set_setting_ui_scale_idx(idx);
                w.set_ui_scale_pct(pct);
                if let Ok(mut cfg) = Config::load() {
                    cfg.ui_scale_pct = pct;
                    let _ = cfg.save();
                }
                w.set_setting_save_status(format!("界面缩放已设为 {pct}%").into());
                info!("[ui] ui-scale -> {pct}%");
            }
        });
    }

    // ===== 界面字体族 =====
    // slint 侧 default-font-family 已绑定到 ui-font-family，这里改属性即全局换字体。
    // 空串 = 回到 Slint 内置默认字体。
    {
        let weak = window.as_weak();
        window.on_apply_ui_font(move |idx: i32| {
            if let Some(w) = weak.upgrade() {
                let family = UI_FONT_OPTIONS.get(idx as usize).copied().unwrap_or("");
                w.set_setting_ui_font_idx(idx);
                w.set_ui_font_family(family.into());
                if let Ok(mut cfg) = Config::load() {
                    cfg.ui_font_family = family.to_string();
                    let _ = cfg.save();
                }
                w.set_setting_save_status(
                    if family.is_empty() {
                        "界面字体已恢复默认".to_string()
                    } else {
                        format!("界面字体已设为 {family}")
                    }
                    .into(),
                );
                info!("[ui] ui-font -> {family:?}");
            }
        });
    }

    // 设置：保存
    let weak_save = window.as_weak();
    let ai_svc = ai.clone();
    let tr_save = translate.clone();
    // P3-3：保存时实时开关划词监听（Arc<Attach> 与常驻监听线程共享同一原子变量）
    let attach_for_save = attach_svc.clone();
    // 气泡字号：保存后实时应用到气泡窗口
    let bubble_for_save = bubble.as_weak();
    window.on_save_settings(move || {
        if let Some(win) = weak_save.upgrade() {
            let idx = win.get_setting_engine_idx();
            let mut cfg = crate::core::config::Config::load().unwrap_or_default();
            cfg.translate_engine = match idx {
                0 => "argos".into(),
                1 => "ollama".into(),
                2 => "sf".into(),
                3 => "mymemory".into(),
                4 => "youdao".into(),
                _ => "argos".into(),
            };
            cfg.sf_key = win.get_setting_sf_key().to_string();
            cfg.ollama_url = win.get_setting_ollama_url().to_string();
            cfg.ollama_model = win.get_setting_ollama_model().to_string();
            // AI 配置
            let ai_backend = win.get_setting_ai_backend_idx();
            cfg.ai_backend = if ai_backend == 0 { "siliconflow".into() } else { "ollama".into() };
            cfg.ai_sf_key = win.get_setting_ai_sf_key().to_string();
            cfg.ai_sf_model = win.get_setting_ai_sf_model().to_string();
            // 术语库（P1-2）：持久化启用开关 + 自定义词条，并实时同步运行期
            cfg.term_enabled = win.get_setting_term_enabled();
            let model = win.get_term_custom_lines();
            let mut custom = std::collections::HashMap::new();
            for i in 0..model.row_count() {
                if let Some(line) = model.row_data(i) {
                    let s = line.to_string();
                    if let Some((src, tgt)) = s.split_once(" → ") {
                        custom.insert(src.to_string(), tgt.to_string());
                    }
                }
            }
            cfg.custom_terms = custom;
            tr_save.set_term_enabled(cfg.term_enabled);
            // 翻译引擎切换：保存后立即对后续翻译生效（运行期重配，不重建）
            let engine_str = match idx {
                0 => "argos",
                1 => "ollama",
                2 => "sf",
                3 => "mymemory",
                4 => "youdao",
                _ => "argos",
            };
            let sf_key = win.get_setting_sf_key().to_string();
            let ollama_url = win.get_setting_ollama_url().to_string();
            let ollama_model = win.get_setting_ollama_model().to_string();
            tr_save.configure(engine_str, &sf_key, &ollama_url, &ollama_model);
            // P3-3 划词翻译开关：持久化 + 实时生效（钩子回调读同一原子变量，立即感知）
            cfg.selection_enabled = win.get_setting_selection_enabled();
            attach_for_save.set_enabled(cfg.selection_enabled);
            // 学习词典来源：持久化（下次查询即生效，无需重启——enrich 每次读 config）
            cfg.study_dict = match win.get_setting_dict_idx() {
                1 => "ecdict".into(),
                _ => "open-dictionary".into(),
            };
            // 学习组别：按下标取回显示名（"全部单词"/"小学词汇"/自定义组名…）
            {
                let model = win.get_stage_options();
                let idx = win.get_setting_stage_idx().max(0) as usize;
                cfg.study_stage = model
                    .row_data(idx)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "全部单词".into());
            }
            // 气泡字号档位：持久化 + 实时应用到气泡窗口
            cfg.bubble_font_idx = win.get_setting_bubble_font_idx();
            if let Some(b) = bubble_for_save.upgrade() {
                b.set_font_preset(cfg.bubble_font_idx);
            }
            match cfg.save() {
                Ok(_) => {
                    win.set_setting_save_status("✓ 已保存".into());
                    // 实时应用 AI 配置
                    let backend = if ai_backend == 0 {
                        crate::services::ai::AiBackend::SiliconFlow
                    } else {
                        crate::services::ai::AiBackend::Ollama
                    };
                    let sf_key = win.get_setting_ai_sf_key().to_string();
                    let sf_model = win.get_setting_ai_sf_model().to_string();
                    let ollama_url = win.get_setting_ollama_url().to_string();
                    let ollama_model = win.get_setting_ollama_model().to_string();
                    ai_svc.configure(
                        backend,
                        &sf_key,
                        &sf_model,
                        &ollama_url,
                        &ollama_model,
                    );
                    info!("[ui] ai backend -> {}", backend.as_str());
                    // 保存后立即刷新 AI 状态指示（is_available 只查配置非空，无网络请求）
                    if ai_svc.is_available() {
                        win.set_ai_status("已连接".into());
                        win.set_ai_status_ok(true);
                    } else {
                        win.set_ai_status("未连接".into());
                        win.set_ai_status_ok(false);
                    }
                }
                Err(e) => win.set_setting_save_status(format!("保存失败: {e}").into()),
            }
        }
    });
    {
        let weak = window.as_weak();
        window.on_request_close(move || {
            info!("[ui] request-close -> hide to tray");
            if let Some(w) = weak.upgrade() {
                let _ = w.hide();
            }
        });
    }

    // 复制译文：写入系统剪贴板
    {
        let weak = window.as_weak();
        window.on_copy_out(move || {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let t = w.get_ocr_out().to_string();
            if t.trim().is_empty() {
                w.set_status_text("没有可复制的译文".into());
                return;
            }
            if services::clipboard::set_text(&t) {
                w.set_status_text(format!("已复制 {} 字符", t.chars().count()).into());
                info!("[ui] copy-out -> 剪贴板（{} 字符）", t.chars().count());
            } else {
                w.set_status_text("复制失败".into());
                warn!("[ui] copy-out 失败（剪贴板被其他程序占用？）");
            }
        });
    }

    // 复制 AI 对话消息：写入系统剪贴板 + 按钮视觉反馈
    {
        let weak = window.as_weak();
        window.on_copy_ai_message(move |msg: slint::SharedString| {
            let w = match weak.upgrade() {
                Some(win) => win,
                None => return,
            };
            let t = msg.as_str().to_string();
            if t.trim().is_empty() {
                w.set_status_text("没有可复制的消息".into());
                return;
            }
            if services::clipboard::set_text(&t) {
                w.set_status_text(format!("已复制 AI 消息（{} 字符）", t.chars().count()).into());
                info!("[ui] copy-ai-message -> 剪贴板（{} 字符）", t.chars().count());
                // 按钮视觉反馈：显示「已复制」1.5 秒后恢复
                let w_fb = weak.clone();
                let msg_fb = msg.clone();
                w.set_ai_copied_msg(msg.clone().into());
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = w_fb.upgrade() {
                            if win.get_ai_copied_msg() == msg_fb {
                                win.set_ai_copied_msg("".into());
                            }
                        }
                    });
                });
            } else {
                w.set_status_text("复制失败".into());
                warn!("[ui] copy-ai-message 失败（剪贴板被其他程序占用？）");
            }
        });
    }

    // 清空双栏内容
    {
        let weak = window.as_weak();
        window.on_clear_content(move || {
            if let Some(w) = weak.upgrade() {
                w.set_ocr_src("".into());
                w.set_ocr_out("".into());
                w.set_status_text("已清空内容".into());
            }
        });
    }

    // Phase 3.4：左右 / 上下对照切换（持久化到 config，下次启动沿用）
    {
        let weak = window.as_weak();
        window.on_toggle_split(move || {
            if let Some(w) = weak.upgrade() {
                let next = !w.get_split_vertical();
                w.set_split_vertical(next);
                w.set_status_text(
                    if next { "已切换为上下对照" } else { "已切换为左右对照" }.into(),
                );
                // 持久化：读改写 config，失败仅告警（不影响本次切换生效）
                match Config::load() {
                    Ok(mut cfg) => {
                        cfg.split_vertical = next;
                        if let Err(e) = cfg.save() {
                            warn!("[ui] toggle-split 持久化失败: {e:#}");
                        } else {
                            info!("[ui] toggle-split -> split_vertical={next}（已保存）");
                        }
                    }
                    Err(e) => warn!("[ui] toggle-split 读取配置失败: {e:#}"),
                }
            }
        });
    }

    // 确认弹窗 OK
    {
        let weak = window.as_weak();
        window.on_confirm_dialog_ok(move || {
            if let Some(w) = weak.upgrade() {
                let target = w.get_confirm_dialog_target().to_string();
                w.set_confirm_dialog_visible(false);
                match target.as_str() {
                    "clear-ai" => {
                        w.set_ai_messages(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
                        w.set_ai_msg_roles(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
                        w.set_ai_streaming("".into());
                        w.set_status_text("对话已清空".into());
                        info!("[ui] clear-ai confirmed");
                    }
                    "clear-history" => {
                        w.set_history_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
                        w.set_status_text("历史已清空".into());
                        info!("[ui] clear-history confirmed");
                    }
                    _ => {}
                }
            }
        });
    }

    // 测试连接
    {
        let weak = window.as_weak();
        window.on_test_connection(move || {
            if let Some(w) = weak.upgrade() {
                w.set_setting_save_status("测试中…".into());
                let weak2 = weak.clone();
                std::thread::spawn(move || {
                    // 尝试翻译一个测试字符串
                    let test_text = "Hello";
                    let tr = services::Translate::new();
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    match rt.block_on(tr.translate(test_text, services::Lang::En, services::Lang::Zh)) {
                        Ok(res) => {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = weak2.upgrade() {
                                    win.set_setting_save_status(format!("✓ 连接成功（{}）", res.engine).into());
                                }
                            });
                        }
                        Err(e) => {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = weak2.upgrade() {
                                    win.set_setting_save_status(format!("✗ {}", e).into());
                                }
                            });
                        }
                    }
                });
            }
        });
    }

    // AI 状态检查：启动时检测
    {
        let weak = window.as_weak();
        let ai_svc = ai.clone();
        std::thread::spawn(move || {
            if ai_svc.is_available() {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        win.set_ai_status("已连接".into());
                        win.set_ai_status_ok(true);
                    }
                });
            } else {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        win.set_ai_status("未连接".into());
                        win.set_ai_status_ok(false);
                    }
                });
            }
        });
    }

    // 全局热键（P0-1 扩展，见 services::hotkey）：
    //   Alt+Q / Alt+T → 截图翻译；Alt+W → 开/关划词；Alt+D → 唤起主窗
    // 触发与注册结果都发生在热键线程，经 invoke_from_event_loop 投递回 UI 线程。
    {
        let weak = window.as_weak();
        let w_trigger = weak.clone();
        let attach_toggle = attach_svc.clone();
        let on_trigger = move |action: services::hotkey::HotkeyAction| {
            let w = w_trigger.clone();
            let at = attach_toggle.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = w.upgrade() {
                    match action {
                        services::hotkey::HotkeyAction::Capture => win.invoke_capture(),
                        services::hotkey::HotkeyAction::ShowWindow => bring_main_to_front(&win),
                        services::hotkey::HotkeyAction::ToggleSelection => {
                            // 与设置页开关同一路径：实时生效（钩子读同一原子变量）+ 持久化
                            let new_val = !at.enabled();
                            at.set_enabled(new_val);
                            win.set_setting_selection_enabled(new_val);
                            let mut cfg = crate::core::config::Config::load().unwrap_or_default();
                            cfg.selection_enabled = new_val;
                            if let Err(e) = cfg.save() {
                                warn!("[ui] hotkey Alt+W 持久化划词开关失败: {e:#}");
                            }
                            win.set_status_text(
                                if new_val { "划词取词已开启".into() } else { "划词取词已关闭".into() },
                            );
                        }
                    }
                }
            });
        };
        let w_status = weak.clone();
        let on_registered = move |summary: String| {
            let w = w_status.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = w.upgrade() {
                    win.set_status_text(summary.into());
                }
            });
        };
        match services::hotkey::Hotkey::start(Box::new(on_trigger), Box::new(on_registered)) {
            Ok(_hk) => info!("[ui] global hotkeys registered (Alt+Q/T, Alt+W, Alt+D)"),
            Err(e) => warn!("[ui] hotkey register failed: {e:#}"),
        }
    }

    // 系统托盘（Windows）：独立线程 + 隐藏窗口 + 右键菜单
    #[cfg(windows)]
    {
        use crate::services::tray::{TrayEvent, TrayIcon};

        // P2-4：点关闭按钮 = 隐藏到托盘（进程常驻），只有托盘「退出」才真正结束进程。
        let weak_close_size = window.as_weak();
        window.window().on_close_requested(move || {
            info!("[ui] 关闭主窗口 → 隐藏到托盘（进程常驻）");
            // 关闭瞬间保存窗口尺寸，供下次启动恢复。
            // 为什么在关闭时存而不是 resize 时存：Slint 1.18 未公开 resize 事件回调
            // （只有 on_close_requested / start_drag 等），且拖拽过程中 resize 回调会高频触发，
            // 每次都读+写 config 磁盘 IO 太重。关闭时存一次即可。
            if let Some(w) = weak_close_size.upgrade() {
                let size = w.window().size();
                let (pw, ph) = (size.width as i32, size.height as i32);
                if pw >= MIN_WINDOW_W && ph >= MIN_WINDOW_H {
                    if let Ok(mut cfg) = Config::load() {
                        if cfg.window_w != pw || cfg.window_h != ph {
                            cfg.window_w = pw;
                            cfg.window_h = ph;
                            let _ = cfg.save();
                        }
                    }
                }
            }
            slint::CloseRequestResponse::HideWindow
        });

        let (tray_tx, tray_rx) = std::sync::mpsc::channel::<TrayEvent>();
        if let Ok(tray) = TrayIcon::new(tray_tx, true) {
            let weak_tray = window.as_weak();
            std::thread::spawn(move || {
                let _tray = tray; // 在本线程保持存活，进程退出时 Drop 发 WM_QUIT
                while let Ok(ev) = tray_rx.recv() {
                    let weak = weak_tray.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = weak.upgrade() {
                            match ev {
                                TrayEvent::Capture => win.invoke_capture(),
                                TrayEvent::Settings => {
                                    win.set_modal_type("settings".into());
                                    win.set_modal_visible(true);
                                }
                                TrayEvent::History => {
                                    win.set_drawer_type("history".into());
                                    win.set_drawer_visible(true);
                                    win.invoke_load_history();
                                }
                                TrayEvent::Collect | TrayEvent::CollectReview => {
                                    win.set_drawer_type("collect".into());
                                    win.set_drawer_visible(true);
                                    win.invoke_load_collect();
                                }
                                // 关闭只是隐藏，这里从托盘把主窗口重新显示出来
                                TrayEvent::OpenMain => {
                                    let _ = win.show();
                                }
                                // 真正退出进程：关闭只隐藏窗口，必须显式退出事件循环
                                TrayEvent::Quit => {
                                    let _ = slint::quit_event_loop();
                                }
                                _ => {}
                            }
                        }
                    });
                }
            });
        } else {
            warn!("[ui] tray init failed");
        }

        // P2-4：窗口关闭仅隐藏到托盘，故必须用 run_event_loop_until_quit —— 普通 run_event_loop
        // 会在「最后一个窗口隐藏」时退出事件循环，托盘与全局热键会随之消失。
        window.show()?;
        slint::run_event_loop_until_quit()?;
    }

    #[cfg(not(windows))]
    window.run()?;

    Ok(())
}

// ============================================================
// 语言对映射 helper（UI ComboBox 索引 ↔ core::types::Lang）
// 顶栏两个 ComboBox model 不对称：
//   from: ["自动检测","English","日本語","한국어"]   → 0=Auto 1=En 2=Ja 3=Ko
//   to:   ["简体中文","English","日本語","한국어"]   → 0=Zh   1=En 2=Ja 3=Ko
// Argos 仅支持 en↔zh，故 from=Auto 时按目标语言归一（让离线引擎命中）。
// ============================================================

/// ComboBox 索引 → 真实 Lang（Auto 源按目标语言归一，便于 Argos 命中 en↔zh）。
fn resolve_langs(from_idx: i32, to_idx: i32) -> (Lang, Lang) {
    let from = match from_idx {
        0 => Lang::Auto,
        1 => Lang::En,
        2 => Lang::Ja,
        3 => Lang::Ko,
        _ => Lang::Auto,
    };
    let to = match to_idx {
        0 => Lang::Zh,
        1 => Lang::En,
        2 => Lang::Ja,
        3 => Lang::Ko,
        _ => Lang::Zh,
    };
    let from = match (from, to) {
        (Lang::Auto, Lang::Zh) => Lang::En,
        (Lang::Auto, Lang::En) => Lang::Zh,
        (other, _) => other,
    };
    (from, to)
}

/// Lang → from ComboBox 的 index（from model 无「简体中文」，退化 Auto）。
fn lang_to_from_idx(l: Lang) -> i32 {
    match l {
        Lang::Auto => 0,
        Lang::En => 1,
        Lang::Ja => 2,
        Lang::Ko => 3,
        Lang::Zh => 0,
    }
}

/// Lang → to ComboBox 的 index（to model 无「自动检测」，退化 En）。
fn lang_to_to_idx(l: Lang) -> i32 {
    match l {
        Lang::Zh => 0,
        Lang::En => 1,
        Lang::Ja => 2,
        Lang::Ko => 3,
        Lang::Auto => 1,
    }
}

/// 格式化 unix 秒为 HH:MM:SS（简化显示，与 rust-reference 一致）
fn format_time(ts: i64) -> String {
    let secs = (ts % 60) as u32;
    let mins = ((ts / 60) % 60) as u32;
    let hours = ((ts / 3600) % 24) as u32;
    format!("{:02}:{:02}:{:02}", hours, mins, secs)
}
