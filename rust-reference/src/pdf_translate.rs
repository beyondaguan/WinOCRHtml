//! PDF 结构化块翻译（M10）：原生宿主侧的翻译管线
//!
//! 职责（对应计划 §13.2 / M10）：
//!
//! 翻译闭包注入说明见文末。

use serde::{Deserialize, Serialize};

/// 单条 PDF 文本块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfBlock {
    pub text: String,
    /// 页码（从 1 开始）
    #[serde(default)]
    pub page: usize,
    /// 块左上角 x 坐标（PDF.js 视图坐标系，相对页面）
    #[serde(default)]
    pub x: f32,
    /// 块左上角 y 坐标
    #[serde(default)]
    pub y: f32,
    /// 块宽
    #[serde(default)]
    pub w: f32,
    /// 块高
    #[serde(default)]
    pub h: f32,
    /// 译文（输出时填充；输入为空）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated: Option<String>,
}

/// 批量翻译 PDF 文本块，保结构（坐标/页码/顺序不变，仅填 `translated`）。
///
/// `translate` 为单块文本翻译闭包。空文本块直接透传。
pub fn translate_blocks<F>(blocks: &[PdfBlock], translate: F) -> Vec<PdfBlock>
where
    F: Fn(&str) -> anyhow::Result<String>,
{
    blocks
        .iter()
        .map(|blk| {
            let mut out = blk.clone();
            if !blk.text.trim().is_empty() {
                match translate(&blk.text) {
                    Ok(t) => out.translated = Some(t),
                    Err(e) => {
                        log::warn!("PDF 块翻译失败: {} | 原文: {}", e, &blk.text);
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

    fn blk(text: &str, page: usize, x: f32, y: f32) -> PdfBlock {
        PdfBlock { text: text.into(), page, x, y, w: 100.0, h: 20.0, translated: None }
    }

    #[test]
    fn preserves_structure_and_fills_translated() {
        let blocks = vec![
            blk("Introduction", 1, 10.0, 20.0),
            blk("", 1, 10.0, 50.0),
            blk("Methodology", 2, 10.0, 20.0),
        ];
        let out = translate_blocks(&blocks, |t| Ok(format!("译:{}", t)));
        assert_eq!(out.len(), 3);
        // 结构保持
        assert_eq!(out[0].page, 1);
        assert_eq!(out[0].x, 10.0);
        assert_eq!(out[0].y, 20.0);
        assert_eq!(out[2].page, 2);
        // 译文填充
        assert_eq!(out[0].translated.as_deref(), Some("译:Introduction"));
        assert_eq!(out[1].translated, None);
        assert_eq!(out[2].translated.as_deref(), Some("译:Methodology"));
    }

    #[test]
    fn failed_block_keeps_position() {
        let blocks = vec![blk("A", 1, 5.0, 5.0), blk("B", 1, 5.0, 30.0)];
        let out = translate_blocks(&blocks, |t| {
            if t == "A" { Err(anyhow::anyhow!("err")) } else { Ok("译B".into()) }
        });
        assert_eq!(out[0].translated, None);
        assert_eq!(out[0].x, 5.0);
        assert_eq!(out[1].translated.as_deref(), Some("译B"));
    }
}
