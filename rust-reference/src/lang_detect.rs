//! 客户端语种检测（§18 / pot-desktop lang_detect 借鉴）
//!
//! 当 `config.src_lang == "auto"` 时，在本地对 OCR 文本做一次轻量预判，
//! 把结果回写进翻译请求，避免把所有「自动」都压到服务端去猜。
//!
//! 实现选择：采用 **Unicode 脚本启发式** 而非统计模型（如 lingua）。
//! 原因：
//!  - OCR 截取的文本通常很短，统计模型短文本准确率低（lingua 自身文档亦提示）；
//!  - 零外部依赖、零二进制体积、确定性强、不破坏现有干净构建；
//!  - 我们最需要的判别（中文 / 日文 / 韩文 / 俄文 vs 拉丁系）恰好是脚本区分最稳的。
//!
//! 接口刻意与「调用 lingua 的 detect_lang」保持一致（`text -> Option<String>`），
//! 将来若要对相近欧洲语言或中日细粒度消歧，只需替换本模块内部实现。

/// 文本中各 Unicode 脚本的计数
#[derive(Default)]
struct ScriptCounts {
    han: usize,
    kana: usize,
    hangul: usize,
    cyrillic: usize,
    latin: usize,
}

#[derive(PartialEq)]
enum Script {
    Han,
    Hiragana,
    Katakana,
    Hangul,
    Cyrillic,
    Latin,
    Other,
}

/// 判断字符所属脚本（仅覆盖本项目关心的语种）
fn script_of(ch: char) -> Script {
    let c = ch as u32;
    match c {
        0x3040..=0x309F => Script::Hiragana, // 平假名
        0x30A0..=0x30FF => Script::Katakana, // 片假名
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Script::Han, // CJK 统一表意文字
        0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7A3 => Script::Hangul, // 谚文
        0x0400..=0x052F => Script::Cyrillic, // 西里尔
        0x0041..=0x007A | 0x00C0..=0x024F => Script::Latin, // 基础拉丁 + 拉丁补充
        _ => Script::Other,
    }
}

/// 对 OCR 文本做语种预判。
///
/// 返回 `Some("<lang>")` 表示有把握的本地判定（zh/ja/ko/ru），
/// 返回 `None` 表示无法可靠判定（过短、纯拉丁系等），调用方应保持原 `src_lang`（通常是 "auto"）交给服务端。
pub fn detect_lang(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut counts = ScriptCounts::default();
    let mut total = 0usize;
    for ch in trimmed.chars() {
        // 只统计字母类字符，忽略空白/数字/标点（这些对语种判别无意义）
        if !ch.is_alphabetic() {
            continue;
        }
        total += 1;
        match script_of(ch) {
            Script::Han => counts.han += 1,
            Script::Hiragana | Script::Katakana => counts.kana += 1,
            Script::Hangul => counts.hangul += 1,
            Script::Cyrillic => counts.cyrillic += 1,
            Script::Latin => counts.latin += 1,
            Script::Other => {}
        }
    }

    // 字母过少：启发式不可靠，交给服务端
    if total < 3 {
        return None;
    }

    // 日文：只要出现假名（平/片假名）即可与中文区分
    if counts.kana > 0 {
        return Some("ja".to_string());
    }
    // 纯汉字主导：默认中文（韩文几乎必然带 Hangul，故不在此误判）
    if counts.han > 0 {
        return Some("zh".to_string());
    }
    if counts.hangul > 0 {
        return Some("ko".to_string());
    }
    if counts.cyrillic > 0 {
        return Some("ru".to_string());
    }
    // 拉丁系（fr/de/es/en 等）脚本层面无法细分，保持 auto 由服务端判定
    if counts.latin > 0 {
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_chinese() {
        assert_eq!(detect_lang("你好世界"), Some("zh".to_string()));
    }

    #[test]
    fn detects_japanese_with_kana() {
        assert_eq!(detect_lang("こんにちは世界"), Some("ja".to_string()));
    }

    #[test]
    fn detects_korean() {
        assert_eq!(detect_lang("안녕하세요"), Some("ko".to_string()));
    }

    #[test]
    fn detects_russian() {
        assert_eq!(detect_lang("Привет мир"), Some("ru".to_string()));
    }

    #[test]
    fn latin_returns_none() {
        // 拉丁系细分不可靠，应交给服务端
        assert_eq!(detect_lang("Hello world"), None);
        assert_eq!(detect_lang("Bonjour le monde"), None);
    }

    #[test]
    fn too_short_returns_none() {
        assert_eq!(detect_lang("你"), None);
        assert_eq!(detect_lang("   "), None);
    }
}
