// ============================================================
// Services · 单词学习（study）
//
// 学习闭环的「进词口」：任何来源（学习窗口手动输入 / 主程序采集 /
// 浏览器扩展 / OCR 文本 / 复习队列）都通过本模块的单词引入接口入库，
// 保证归一化、查重、分级、来源标记的行为一致。
//
// UI 侧（ui::StudyWindow）只调 introduce_word + enrich_async 两步：
//   1) introduce_word —— 归一化 + 查重 + 入库（含 level/project/source）
//   2) enrich_async   —— 后台线程跑词典增强（音标/英英/词根/例句/中文释义）
//                        完成后经 invoke_from_event_loop 回填 UI
// ============================================================
use anyhow::Result;
use log::info;

use super::collect::Collect;

/// 引入结果
#[derive(Debug, Clone)]
pub struct Introduced {
    pub item: super::collect::CollectItem,
    pub is_new: bool,
}

/// 词形归一：trim + 小写 + 仅保留英文字母/撇号/连字符。
/// 返回 None 表示不是合法英文词（空串/纯数字/含中文等）。
pub fn normalize_word(raw: &str) -> Option<String> {
    let w = raw.trim().to_lowercase();
    if w.is_empty() {
        return None;
    }
    if !w.chars().all(|c| c.is_ascii_alphabetic() || c == '\'' || c == '-') {
        return None;
    }
    if !w.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(w)
}

/// ── 单词引入唯一入口 ──
/// 已存在（word 去重）→ 返回已有条目（is_new = false），不重复入库；
/// 不存在 → classify/cefr 分级 + DEFAULT_PROJECT + source 标记后入库。
///
/// `context`：采集原句（可空）；`source`：manual / browser / selection / dict / import / ocr
pub fn introduce_word(
    cb: &Collect,
    raw: &str,
    context: &str,
    source: &str,
) -> Result<Introduced> {
    let word = normalize_word(raw)
        .ok_or_else(|| anyhow::anyhow!("不是合法的英文单词: \"{raw}\""))?;

    // 查重：同 word（忽略大小写）只保留一条
    let existing = cb
        .list_all()
        .into_iter()
        .find(|i| i.word.eq_ignore_ascii_case(&word));
    if let Some(item) = existing {
        info!("[services::study] 引入命中已有: {word} (id={})", item.id);
        return Ok(Introduced { item, is_new: false });
    }

    // 分级：优先术语库/词根（医学词 → 专业），否则 CEFR 档位
    let (project, level) =
        if let Some(c) = super::classify::classify(&word, &[]) {
            (c.project, c.level)
        } else {
            (
                super::collect::DEFAULT_PROJECT.to_string(),
                super::classify::cefr_tier(&word).to_string(),
            )
        };

    cb.add_collect(&word, "", "en", "zh", &project, context, &level)?;
    // add_collect 不带 source 参数（历史签名），入库后补写来源标记
    let items = cb.list_all();
    let item = items
        .into_iter()
        .find(|i| i.word.eq_ignore_ascii_case(&word))
        .ok_or_else(|| anyhow::anyhow!("引入后未找到条目: {word}"))?;
    cb.update_source(&item.id, source)?;
    info!(
        "[services::study] 引入新词: {word} [{} / {}] source={source}",
        item.project, item.level
    );
    Ok(Introduced { item, is_new: true })
}

/// ── 词典增强（同步版，建议放后台线程跑）──
/// 音标 + 英英全文 + 例句 + 词根 + 词源 → update_enrichment（只补空）。
/// 数据未就绪时静默跳过，不报错。
pub fn enrich_sync(cb: &Collect, word: &str) {
    let w = word.trim().to_lowercase();
    if w.is_empty() {
        return;
    }
    let en = super::enrich::Enrich::new();
    if let Ok(Some(info)) = en.enrich(&w) {
        let _ = cb.update_enrichment(
            // 需要条目 id：按 word 查
            &find_id(cb, &w),
            &info.phonetic,
            &info.definition_en,
            &info.example,
            &info.example_zh,
            &info.roots,
            &info.etymology,
        );
    }
}

/// ── 词典增强（后台线程版）──
/// 完成后在 UI 线程回调 `on_done(word)`（词卡仍显示同词时由调用方决定是否回填）。
/// 注：Collect 只是 &'static 连接的薄包装，线程内 Collect::new() 零成本重建，
///     避免 &Collect 引用逃逸出函数体（E0521）。
pub fn enrich_async(word: String, on_done: impl FnOnce(String) + Send + 'static) {
    std::thread::spawn(move || {
        let cb = Collect::new();
        enrich_sync(&cb, &word);
        on_done(word);
    });
}

/// ── 中文释义补全（后台线程版）──
/// 释义为空时跑 Argos en→zh，写入后回调。失败静默（离线翻译未就绪等）。
pub fn translate_zh_async(
    word: String,
    tr: std::sync::Arc<super::translate::Translate>,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    on_done: impl FnOnce(String, String) + Send + 'static,
) {
    std::thread::spawn(move || {
        let cb = Collect::new();
        let items = cb.list_all();
        let Some(item) = items.iter().find(|i| i.word.eq_ignore_ascii_case(&word)) else {
            return;
        };
        if !item.translation.trim().is_empty() {
            return; // 已有释义，不重跑
        }
        let id = item.id.clone();
        // 中文释义：ECDICT 英汉库直查（离线即时）优先，Argos 兜底
        let zh = super::dict::zh_translation_of(&word)
            .unwrap_or_else(|| {
                rt.block_on(tr.translate(&word, super::Lang::En, super::Lang::Zh))
                    .ok()
                    .map(|r| r.text.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_default()
            });
        if !zh.is_empty() {
            let _ = cb.update_translation(&id, &zh);
        }
        on_done(word, zh);
    });
}

fn find_id(cb: &Collect, word: &str) -> String {
    cb.list_all()
        .into_iter()
        .find(|i| i.word.eq_ignore_ascii_case(word))
        .map(|i| i.id)
        .unwrap_or_default()
}

// ============================================================
// 批量内容补全（2026-10-17：存量 250 词 92-96% 字段为空，用户拍板"一键补全"）
// ============================================================

/// 采集入库后的自动增强（fire-and-forget，2026-10-17）：
/// 划词气泡 / 生词本手动添加 / 候选词双击等入口 add_collect 成功后调用。
/// 后台跑：词典三件套（音标/英英/例句/词根/词源）+ 中文释义（术语库→Argos）+ 未分级补级。
/// 只补空、不覆盖；重复调用安全（与一键补全/词卡增强幂等共存）。
pub fn auto_enrich_new(
    tr: std::sync::Arc<super::translate::Translate>,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    raw: String,
) {
    let word = normalize_word(&raw).unwrap_or_else(|| raw.trim().to_lowercase());
    if word.is_empty() {
        return;
    }
    // 1) 词典三件套（enrich_async 内部自建 Collect，缓存表进程级共享）
    enrich_async(word.clone(), |_w| {});
    // 2) 中文释义 + 分级
    std::thread::spawn(move || {
        let cb = Collect::new();
        let termdb = super::terms::TermDb::new();
        let Some(item) = cb
            .list_all()
            .into_iter()
            .find(|i| i.word.eq_ignore_ascii_case(&word))
        else {
            return;
        };
        if item.translation.trim().is_empty() {
            // 中文释义三级链：术语库（医学优先）→ ECDICT 英汉库 → Argos 兜底
            let zh = if let Some(e) = termdb.entry(&word) {
                Some(e.zh.clone())
            } else if let Some(zh) = super::dict::zh_translation_of(&word) {
                Some(zh)
            } else {
                rt.block_on(tr.translate(&word, super::Lang::En, super::Lang::Zh))
                    .ok()
                    .map(|r| r.text.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            if let Some(zh) = zh {
                let _ = cb.update_translation(&item.id, &zh);
            }
        }
        if item.level == "未分级" {
            let level = if termdb.entry(&word).is_some() {
                "专业".to_string()
            } else {
                super::classify::classify(&word, &[])
                    .map(|c| c.level)
                    .unwrap_or_else(|| super::classify::cefr_tier(&word).to_string())
            };
            if level != "未分级" {
                let _ = cb.update_level_if_ungraded(&item.id, &level);
            }
        }
    });
}

/// 补全报告
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackfillReport {
    pub scanned: usize,
    pub updated: usize,
}

/// 批量补全：遍历生词本，逐词回填**缺失**字段（只补空，不覆盖已有值）：
///   - 音标/英英释义/例句/词根/词源链：本地三件套（ipa_en.json / open-dictionary / etymology 子集）
///   - 中文释义：术语库 medical.json 命中优先（含学科），否则 Argos en→zh 兜底
///   - 未分级：术语库命中 → 专业；否则 classify/CEFR
///
/// 阻塞调用，请在后台线程运行；`on_progress(done, total, word)` 在同一线程同步回调
/// （word 为空串表示收尾），调用方经 invoke_from_event_loop 刷 UI。
pub fn backfill_all(
    tr: std::sync::Arc<super::translate::Translate>,
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    mut on_progress: impl FnMut(usize, usize, String) + Send,
) -> Result<BackfillReport> {
    let cb = Collect::new();
    let termdb = super::terms::TermDb::new();
    let en = super::enrich::Enrich::new();
    let items = cb.list_all();
    let total = items.len();
    let mut updated = 0usize;

    for (done, item) in items.iter().enumerate() {
        on_progress(done, total, item.word.clone());

        // 可取消（主窗「取消」按钮置位全局标志）
        if crate::services::cancel::is_cancelled() {
            return Err(anyhow::anyhow!("补全已取消"));
        }

        // ⚠️ 只处理英语单词（2026-10-17 用户拍板："我只要英语单词，不需要汉语"）：
        // CJK 脏数据 / 整句新闻直接跳过——它们送 Argos 翻译会离线跑几分钟，表现为"卡死"。
        if normalize_word(&item.word).is_none() {
            continue;
        }

        let mut changed = false;

        // 1) 词典增强（音标/英英/例句/词根/词源）—— 任一缺字段就跑，update_enrichment 只补空
        if item.phonetic.is_empty()
            || item.definition_en.is_empty()
            || item.example.is_empty()
            || item.roots.is_empty()
            || item.etymology.is_empty()
        {
            if let Ok(Some(info)) = en.enrich(&item.word) {
                let _ = cb.update_enrichment(
                    &item.id,
                    &info.phonetic,
                    &info.definition_en,
                    &info.example,
                    &info.example_zh,
                    &info.roots,
                    &info.etymology,
                );
                changed = true;
            }
        }

        // 2) 中文释义三级链：术语库命中优先（即时）→ ECDICT 英汉库（离线，词组也可查）
        //    → Argos 仅对**纯单词**跑兜底（短语/长句离线翻译极慢且无单词价值）
        if item.translation.trim().is_empty() {
            let zh = if let Some(e) = termdb.entry(&item.word) {
                Some(e.zh.clone())
            } else if let Some(zh) = super::dict::zh_translation_of(&item.word) {
                Some(zh)
            } else if !item.word.contains(' ') {
                rt.block_on(tr.translate(&item.word, super::Lang::En, super::Lang::Zh))
                    .ok()
                    .map(|r| r.text.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            };
            if let Some(zh) = zh {
                let _ = cb.update_translation(&item.id, &zh);
                changed = true;
            }
        }

        // 3) 未分级 → 术语库命中=专业，否则 classify/CEFR（update_level_if_ungraded 只补未分级）
        if item.level == "未分级" {
            let level = if termdb.entry(&item.word).is_some() {
                "专业".to_string()
            } else {
                super::classify::classify(&item.word, &[])
                    .map(|c| c.level)
                    .unwrap_or_else(|| super::classify::cefr_tier(&item.word).to_string())
            };
            if level != "未分级" {
                let _ = cb.update_level_if_ungraded(&item.id, &level);
                changed = true;
            }
        }

        if changed {
            updated += 1;
        }
    }

    on_progress(total, total, String::new());
    Ok(BackfillReport { scanned: total, updated })
}
