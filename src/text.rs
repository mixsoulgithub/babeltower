use unicode_normalization::UnicodeNormalization;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Zh,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Classified {
    Word { lang: Lang },
    Sentence,
}

pub fn is_cjk(ch: char) -> bool {
    matches!(ch as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

pub fn has_cjk(text: &str) -> bool {
    text.chars().any(is_cjk)
}

pub fn cjk_count(text: &str) -> usize {
    text.chars().filter(|ch| is_cjk(*ch)).count()
}

pub fn classify(input: &str) -> Classified {
    let text = input.trim();
    if text.is_empty() {
        return Classified::Sentence;
    }
    if has_cjk(text) {
        let punctuated = text
            .chars()
            .any(|ch| ch.is_whitespace() || "。！？；，、,.!?;:".contains(ch));
        if punctuated || cjk_count(text) > 4 {
            return Classified::Sentence;
        }
        return Classified::Word { lang: Lang::Zh };
    }
    let mut chars = text.chars();
    let first = chars.next().unwrap_or(' ');
    if first.is_ascii_alphabetic() {
        let rest_ok =
            chars.all(|ch| ch.is_ascii_alphabetic() || ch == '\'' || ch == '’' || ch == '-');
        if rest_ok {
            return Classified::Word { lang: Lang::En };
        }
    }
    Classified::Sentence
}

pub fn normalize_answer(text: &str) -> String {
    let normalized: String = text.nfkc().collect::<String>().to_lowercase();
    let mut out = String::new();
    let mut space = false;
    for ch in normalized.chars() {
        if ch.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(ch);
    }
    out.trim_matches(|ch: char| {
        ch.is_whitespace() || ".,!?;:'\"()[]{}<>，。！？；：、".contains(ch)
    })
    .to_string()
}

pub fn strip_pos(text: &str) -> (Option<String>, String) {
    let text = text.trim();
    if text.is_empty() {
        return (None, String::new());
    }
    if let Some(rest) = text.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let pos = rest[..end].to_string();
            let body = rest[end + 1..].trim_start().to_string();
            return (Some(format!("[{pos}]")), body);
        }
    }
    let mut parts = text.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim_start();
    let looks_like_pos = first.ends_with('.')
        && first.len() <= 8
        && first
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || ch == '.');
    if looks_like_pos && !rest.is_empty() {
        return (Some(first.to_string()), rest.to_string());
    }
    (None, text.to_string())
}

pub fn answer_parts(expected: &str) -> Vec<String> {
    let text = expected.replace("\\n", "\n");
    let mut parts = Vec::new();
    for chunk in text.split(['\n', ';', '/']) {
        let (_, body) = strip_pos(chunk);
        for piece in body.split([',', '，', '、']) {
            let normalized = normalize_answer(piece);
            if !normalized.is_empty() {
                parts.push(normalized);
            }
        }
    }
    parts
}

pub fn answer_matches(given: &str, expected: &str) -> bool {
    let guess = normalize_answer(given);
    if guess.is_empty() {
        return false;
    }
    let full = normalize_answer(&expected.replace("\\n", "\n"));
    if guess == full {
        return true;
    }
    answer_parts(expected).iter().any(|part| *part == guess)
}

pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub fn truncate_width(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + char_width > width {
            break;
        }
        out.push(ch);
        used += char_width;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_words_and_sentences() {
        assert_eq!(classify("box"), Classified::Word { lang: Lang::En });
        assert_eq!(classify("don't"), Classified::Word { lang: Lang::En });
        assert_eq!(classify("take off"), Classified::Sentence);
        assert_eq!(classify("你好"), Classified::Word { lang: Lang::Zh });
        assert_eq!(classify("今天天气很好"), Classified::Sentence);
        assert_eq!(classify("你好。"), Classified::Sentence);
        assert_eq!(classify(""), Classified::Sentence);
    }

    #[test]
    fn answer_matching_handles_senses_and_pos() {
        assert!(answer_matches("箱", "n. 盒子, 箱; vi. 拳击"));
        assert!(answer_matches("拳击", "n. 盒子, 箱; vi. 拳击"));
        assert!(answer_matches("盒子", "n. 盒子, 箱"));
        assert!(answer_matches("a b", "A B"));
        assert!(!answer_matches("", "x"));
        assert!(!answer_matches("wrong", "right"));
    }

    #[test]
    fn width_helpers_do_not_split_cjk() {
        assert_eq!(display_width("你好ab"), 6);
        assert_eq!(truncate_width("你好ab", 5), "你好a");
    }
}
