//! 视频字幕批量翻译（M9）：原生宿主侧的翻译管线
//!
//! 职责（对应计划 §3.3 / M9）：
//! - 接收浏览器扩展发来的字幕段（文本 + 时间轴），批量翻译，返回双语字幕段。
//! - 经过术语库（M11）保护，确保术语一致性。
//! - 仅做翻译；DOM 监听 / 字幕轨道提取 / 双语叠加由扩展侧负责（无 GUI 不可在此验证）。
//!
//! 设计：`translate_segments` 接受一个可注入的翻译闭包 `F`，便于单元测试
//! （无需真实网络/密钥）。NM handler 用真实 `TranslateEngine` 包装该闭包。

use serde::{Deserialize, Serialize};

/// 单条字幕段（输入/输出共用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleSegment {
    pub text: String,
    /// 起始时间（秒），时间轴对齐用
    #[serde(default)]
    pub start: f64,
    /// 结束时间（秒）
    #[serde(default)]
    pub end: f64,
    /// 译文（输出时填充；输入为空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated: Option<String>,
}

/// 批量翻译字幕段。
///
/// `translate` 为单条文本翻译闭包；内部对每条非空文本调用一次。
/// 空文本段直接透传（保持时间轴对齐）。
pub fn translate_segments<F>(segments: &[SubtitleSegment], translate: F) -> Vec<SubtitleSegment>
where
    F: Fn(&str) -> anyhow::Result<String>,
{
    segments
        .iter()
        .map(|seg| {
            let mut out = seg.clone();
            if !seg.text.trim().is_empty() {
                match translate(&seg.text) {
                    Ok(t) => out.translated = Some(t),
                    // 翻译失败时保留原文译文为空，不破坏整批时间轴
                    Err(e) => {
                        log::warn!("字幕段翻译失败: {} | 原文: {}", e, &seg.text);
                        out.translated = None;
                    }
                }
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_nonempty_keeps_timeline() {
        let segs = vec![
            SubtitleSegment { text: "Hello".into(), start: 0.0, end: 2.0, translated: None },
            SubtitleSegment { text: "".into(), start: 2.0, end: 2.5, translated: None },
            SubtitleSegment { text: "World".into(), start: 2.5, end: 4.0, translated: None },
        ];
        let out = translate_segments(&segs, |t| Ok(format!("译:{}", t)));
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].translated.as_deref(), Some("译:Hello"));
        assert_eq!(out[1].translated, None); // 空段透传
        assert_eq!(out[2].translated.as_deref(), Some("译:World"));
        // 时间轴保留
        assert_eq!(out[0].start, 0.0);
        assert_eq!(out[2].end, 4.0);
    }

    #[test]
    fn failed_segment_does_not_break_batch() {
        let segs = vec![
            SubtitleSegment { text: "A".into(), start: 0.0, end: 1.0, translated: None },
            SubtitleSegment { text: "B".into(), start: 1.0, end: 2.0, translated: None },
        ];
        let out = translate_segments(&segs, |t| {
            if t == "A" { Err(anyhow::anyhow!("网络错误")) } else { Ok("译B".into()) }
        });
        assert_eq!(out[0].translated, None); // 失败保留空
        assert_eq!(out[1].translated.as_deref(), Some("译B"));
    }
}
