// ============================================================
// Services · 导出（Phase 2 扩展）
// 格式：Markdown / 纯文本 / Anki TSV / 欧路 XML
// 依赖 HistoryItem / CollectItem（由调用方提供数据，本模块只负责序列化）
// 规格见 docs/WinOCR-开发指南.md §5
// ============================================================
use anyhow::Result;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::core::types::HistoryItem;

/// 导出格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    PlainText,
    AnkiTsv,
    EudicXml,
}

impl ExportFormat {
    /// 显示名（供导出模态 ComboBox 使用）
    #[allow(dead_code)] // 预留：UI 下拉框显示（当前用 index 映射）
    pub fn label(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "Markdown",
            ExportFormat::PlainText => "纯文本",
            ExportFormat::AnkiTsv => "Anki TSV",
            ExportFormat::EudicXml => "欧路 XML",
        }
    }

    /// 文件扩展名
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Markdown => "md",
            ExportFormat::PlainText => "txt",
            ExportFormat::AnkiTsv => "tsv",
            ExportFormat::EudicXml => "xml",
        }
    }

    /// 从显示名解析（未知 → Markdown）
    #[allow(dead_code)] // 预留：UI 下拉框反向映射
    pub fn from_label(s: &str) -> Self {
        match s {
            "Anki TSV" => ExportFormat::AnkiTsv,
            "欧路 XML" => ExportFormat::EudicXml,
            "纯文本" => ExportFormat::PlainText,
            _ => ExportFormat::Markdown,
        }
    }
}

/// 导出服务
pub struct Export;

impl Export {
    pub fn new() -> Self {
        Self
    }

    /// 导出历史记录
    pub fn export_history(
        &self,
        items: &[HistoryItem],
        format: ExportFormat,
        path: &Path,
    ) -> Result<()> {
        match format {
            ExportFormat::Markdown => self.history_md(items, path),
            ExportFormat::PlainText => self.history_txt(items, path),
            ExportFormat::AnkiTsv => self.history_anki(items, path),
            ExportFormat::EudicXml => self.history_eudic(items, path),
        }
    }

    /// 导出生词本
    pub fn export_collect(
        &self,
        items: &[super::collect::CollectItem],
        format: ExportFormat,
        path: &Path,
    ) -> Result<()> {
        match format {
            ExportFormat::Markdown => self.collect_md(items, path),
            ExportFormat::PlainText => self.collect_txt(items, path),
            ExportFormat::AnkiTsv => self.collect_anki(items, path),
            ExportFormat::EudicXml => self.collect_eudic(items, path),
        }
    }

    // ---------------- 历史 ----------------

    fn history_md(&self, items: &[HistoryItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "# WinOCR 翻译历史")?;
        writeln!(f)?;
        writeln!(f, "> 共 {} 条记录", items.len())?;
        writeln!(f)?;
        writeln!(f, "| 时间 | 原文 | 译文 | 引擎 | 语言 | 项目 |")?;
        writeln!(f, "|------|------|------|------|------|------|")?;
        for item in items {
            let ts = format_ts(item.ts);
            let src_esc = item.src.replace('|', "\\|").replace('\n', " ");
            let tgt_esc = item.tgt.replace('|', "\\|").replace('\n', " ");
            let proj_esc = item.project.replace('|', "\\|");
            writeln!(
                f,
                "| {ts} | {src_esc} | {tgt_esc} | {} | {}→{} | {proj_esc} |",
                item.engine, item.src_lang, item.tgt_lang
            )?;
        }
        Ok(())
    }

    fn history_txt(&self, items: &[HistoryItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "WinOCR 翻译历史")?;
        writeln!(f, "================")?;
        writeln!(f, "共 {} 条记录\n", items.len())?;
        for item in items {
            let ts = format_ts(item.ts);
            writeln!(f, "[{}] {} → {}", ts, item.src_lang, item.tgt_lang)?;
            writeln!(f, "原文: {}", item.src)?;
            writeln!(f, "译文: {}", item.tgt)?;
            writeln!(f, "引擎: {}", item.engine)?;
            writeln!(f, "项目: {}", item.project)?;
            writeln!(f, "---")?;
        }
        Ok(())
    }

    /// Anki TSV：原文\t译文\t项目（可再补标签）
    fn history_anki(&self, items: &[HistoryItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        for item in items {
            let src = item.src.replace('\t', " ").replace('\n', " ");
            let tgt = item.tgt.replace('\t', " ").replace('\n', " ");
            let proj = item.project.replace('\t', " ").replace('\n', " ");
            writeln!(f, "{}\t{}\t{}", src, tgt, proj)?;
        }
        Ok(())
    }

    /// 欧路 XML
    fn history_eudic(&self, items: &[HistoryItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
        writeln!(f, "<wordbook>")?;
        for item in items {
            let src = xml_escape(&item.src);
            let tgt = xml_escape(&item.tgt);
            let tags = xml_escape(&item.project);
            writeln!(f, "  <item>")?;
            writeln!(f, "    <word>{src}</word>")?;
            writeln!(f, "    <trans><![CDATA[{tgt}]]></trans>")?;
            if !tags.is_empty() {
                writeln!(f, "    <tags>{tags}</tags>")?;
            }
            writeln!(f, "  </item>")?;
        }
        writeln!(f, "</wordbook>")?;
        Ok(())
    }

    // ---------------- 生词本 ----------------

    fn collect_md(&self, items: &[super::collect::CollectItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "# WinOCR 生词本")?;
        writeln!(f)?;
        writeln!(f, "> 共 {} 个生词", items.len())?;
        writeln!(f)?;
        writeln!(f, "| 单词 | 翻译 | 音标 | 英英释义 | 熟练度 | 项目 | 难度 |")?;
        writeln!(f, "|------|------|------|----------|--------|------|------|")?;
        for item in items {
            let stars = "★".repeat(item.mastery as usize);
            let word_esc = item.word.replace('|', "\\|");
            let trans_esc = item.translation.replace('|', "\\|");
            let phon_esc = item.phonetic.replace('|', "\\|");
            let def_esc = item.definition_en.replace('|', "\\|").replace('\n', " ");
            writeln!(
                f,
                "| {word_esc} | {trans_esc} | {phon_esc} | {def_esc} | {stars} | {} | {} |",
                item.project, item.level
            )?;
        }
        Ok(())
    }

    fn collect_txt(&self, items: &[super::collect::CollectItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "WinOCR 生词本")?;
        writeln!(f, "============")?;
        writeln!(f, "共 {} 个生词\n", items.len())?;
        for item in items {
            writeln!(
                f,
                "{} → {} [{}] ({})",
                item.word, item.translation, item.mastery, item.project
            )?;
        }
        Ok(())
    }

    /// Anki TSV：word\t翻译\t音标\t项目\t难度（可再补标签）
    fn collect_anki(&self, items: &[super::collect::CollectItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        for item in items {
            let word = item.word.replace('\t', " ").replace('\n', " ");
            let trans = item.translation.replace('\t', " ").replace('\n', " ");
            let phon = item.phonetic.replace('\t', " ").replace('\n', " ");
            let proj = item.project.replace('\t', " ").replace('\n', " ");
            let lvl = item.level.replace('\t', " ").replace('\n', " ");
            writeln!(f, "{word}\t{trans}\t{phon}\t{proj}\t{lvl}")?;
        }
        Ok(())
    }

    /// 欧路 XML
    fn collect_eudic(&self, items: &[super::collect::CollectItem], path: &Path) -> Result<()> {
        let mut f = File::create(path)?;
        writeln!(f, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
        writeln!(f, "<wordbook>")?;
        for item in items {
            let word = xml_escape(&item.word);
            let trans = xml_escape(&item.translation);
            let tags = xml_escape(&item.project);
            let phon = xml_escape(&item.phonetic);
            let def = xml_escape(&item.definition_en);
            let ex = xml_escape(&item.example);
            let lvl = xml_escape(&item.level);
            writeln!(f, "  <item>")?;
            writeln!(f, "    <word>{word}</word>")?;
            writeln!(f, "    <trans><![CDATA[{trans}]]></trans>")?;
            if !phon.is_empty() {
                writeln!(f, "    <phonetic>{phon}</phonetic>")?;
            }
            if !def.is_empty() {
                writeln!(f, "    <definition><![CDATA[{def}]]></definition>")?;
            }
            if !ex.is_empty() {
                writeln!(f, "    <example><![CDATA[{ex}]]></example>")?;
            }
            if !lvl.is_empty() {
                writeln!(f, "    <level>{lvl}</level>")?;
            }
            if !tags.is_empty() {
                writeln!(f, "    <tags>{tags}</tags>")?;
            }
            writeln!(f, "  </item>")?;
        }
        writeln!(f, "</wordbook>")?;
        Ok(())
    }
}

fn format_ts(ts: i64) -> String {
    let secs = (ts % 60) as u32;
    let mins = ((ts / 60) % 60) as u32;
    let hours = ((ts / 3600) % 24) as u32;
    format!("{hours:02}:{mins:02}:{secs:02}")
}

/// XML 转义（用于 <word>/<tags> 文本节点；<trans> 用 CDATA 不需转义）
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
        .replace('\n', " ")
        .replace('\t', " ")
}