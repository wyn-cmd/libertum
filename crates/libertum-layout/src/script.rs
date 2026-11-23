//! Which script and language a run of text is in, for the two places layout cares: choosing
//! the regional font for Chinese, Japanese and Korean text, and keeping English hyphenation
//! patterns away from words they were not written for.
//!
//! Han characters are shared by Chinese, Japanese and Korean but are drawn differently in
//! each (a different glyph for the same code point), so a font picked without a language
//! shows Chinese text in Japanese forms or the reverse. Kana and Hangul belong to one
//! language each, which settles the language of the text around them.

use std::ops::Range;

/// The kinds of East Asian character layout tells apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cjk {
    /// A Han ideograph: Chinese, Japanese or Korean.
    Han,
    /// Hiragana or katakana: Japanese.
    Kana,
    /// Hangul: Korean.
    Hangul,
    /// Punctuation and full-width forms set with the ideographs around them.
    Punct,
}

fn cjk_class(c: char) -> Option<Cjk> {
    let u = u32::from(c);
    Some(match u {
        0x3005 | 0x3007 | 0x3021..=0x3029 | 0x3038..=0x303B => Cjk::Han,
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x20000..=0x2EBEF
        | 0x2F800..=0x2FA1F
        | 0x30000..=0x323AF => Cjk::Han,
        0x3041..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F | 0x1B000..=0x1B16F => Cjk::Kana,
        0x1100..=0x11FF | 0x3131..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF | 0xFFA0..=0xFFDC => {
            Cjk::Hangul
        }
        0x3000..=0x303F | 0xFE30..=0xFE4F | 0xFF00..=0xFF65 | 0xFFE0..=0xFFEE => Cjk::Punct,
        _ => return None,
    })
}

/// Whether `c` stands upright in vertical text: ideographs, kana, Hangul and East Asian
/// punctuation and full-width forms. Everything else (Latin, Cyrillic, digits) is turned
/// sideways, a quarter turn clockwise, as in a Japanese book.
pub fn is_upright_in_vertical(c: char) -> bool {
    cjk_class(c).is_some()
}

/// The byte range of the number at byte offset `at` of `text`, when it stands upright in
/// vertical text. A number is a run of ASCII digits, with `. , : / -` between digits and `%`
/// after one (`2026`, `3.14`, `2026-11-23`, `45%`). It stands upright, each digit in its own
/// cell down the column, unless it touches a Latin letter (`B2B`, `COVID-19`, `x2`), which goes
/// sideways with the word. `None` when `at` is not in a number, or the number goes sideways.
pub fn upright_number_run(text: &str, at: usize) -> Option<Range<usize>> {
    /// Longest stretch looked at each way, so a long paragraph is never scanned whole.
    const REACH: usize = 64;
    let char_at = |i: usize| text.get(i..).and_then(|rest| rest.chars().next());
    let char_before = |i: usize| text.get(..i).and_then(|head| head.chars().next_back());
    let numeric = |i: usize| -> bool {
        let Some(c) = char_at(i) else { return false };
        let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
        match c {
            '0'..='9' => true,
            '.' | ',' | ':' | '/' | '-' => digit(char_before(i)) && digit(char_at(i + 1)),
            '%' => digit(char_before(i)),
            _ => false,
        }
    };
    if !text.is_char_boundary(at) || !numeric(at) {
        return None;
    }
    let mut start = at;
    for _ in 0..REACH {
        match char_before(start) {
            Some(c) if numeric(start - c.len_utf8()) => start -= c.len_utf8(),
            _ => break,
        }
    }
    let mut end = at + char_at(at).map_or(0, char::len_utf8);
    for _ in 0..REACH {
        match char_at(end) {
            Some(c) if numeric(end) => end += c.len_utf8(),
            _ => break,
        }
    }
    // A Latin letter right beside the number, or one a hyphen or underscore away (`COVID-19`).
    let letter = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphabetic());
    let joined_before = match char_before(start) {
        Some('-' | '_') => letter(char_before(start - 1)),
        other => letter(other),
    };
    let joined_after = match char_at(end) {
        Some(c @ ('-' | '_')) => letter(char_at(end + c.len_utf8())),
        other => letter(other),
    };
    (!(joined_before || joined_after)).then_some(start..end)
}

/// Whether the character at byte offset `at` is part of a number that stands upright in
/// vertical text (see [`upright_number_run`]).
pub fn number_stands_upright(text: &str, at: usize) -> bool {
    upright_number_run(text, at).is_some()
}

/// The range of a two-digit number at `at` that is set horizontally in a single cell of the
/// column (tate-chu-yoko), as dates and short counts are in Japanese: `11` in `11月`, `23` in
/// `23日`. A number of one digit is just upright, one of three or more (or with a decimal
/// point, or a `%`) is stacked a digit to a cell.
pub fn tcy_pair_at(text: &str, at: usize) -> Option<Range<usize>> {
    let run = upright_number_run(text, at)?;
    (run.len() == 2 && text[run.clone()].bytes().all(|b| b.is_ascii_digit())).then_some(run)
}

/// How much room, as a fraction of the font size, an upright digit is given beyond its own
/// width, so that its cell down the column is about an em: an upright digit is taller than it
/// is wide, and without this the digits of a number would overlap.
pub const NUMBER_CELL_SPACING_EM: f32 = 0.45;

/// The byte ranges of `text` that are numbers stacked a digit to a cell in vertical text
/// (see [`number_stands_upright`]), adjacent characters joined. A two-digit number set
/// horizontally in one cell ([`tcy_pair_at`]) is not among them: it needs no extra room.
pub fn upright_number_ranges(text: &str) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for (at, c) in text.char_indices() {
        if !(c.is_ascii_digit() || matches!(c, '.' | ',' | ':' | '/' | '-' | '%')) {
            continue;
        }
        if number_stands_upright(text, at) && tcy_pair_at(text, at).is_none() {
            let end = at + c.len_utf8();
            match out.last_mut() {
                Some(last) if last.end == at => last.end = end,
                _ => out.push(at..end),
            }
        }
    }
    out
}

/// The locale layout hands the font selector for a document language tag, when the tag names
/// a language written with ideographs; `None` for every other tag (including English).
pub fn locale_for_tag(tag: &str) -> Option<&'static str> {
    let lower = tag.to_ascii_lowercase().replace('_', "-");
    let mut parts = lower.split('-');
    match parts.next()? {
        "ja" => Some("ja"),
        "ko" => Some("ko"),
        "zh" => {
            let rest: Vec<&str> = parts.collect();
            let traditional = rest
                .iter()
                .any(|part| matches!(*part, "hant" | "tw" | "hk" | "mo"));
            Some(if traditional { "zh-TW" } else { "zh-CN" })
        }
        _ => None,
    }
}

/// A Cyrillic language that has hyphenation patterns (only Ukrainian: its patterns are MIT).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cyrillic {
    Ukrainian,
}

/// The Cyrillic language of a document language tag (`uk`), if it has patterns.
pub fn cyrillic_for_tag(tag: &str) -> Option<Cyrillic> {
    let lower = tag.to_ascii_lowercase();
    match lower.split(['-', '_']).next()? {
        "uk" => Some(Cyrillic::Ukrainian),
        _ => None,
    }
}

/// True for the letters of the Cyrillic block (including Ukrainian ґ and the like).
pub fn is_cyrillic_letter(c: char) -> bool {
    c.is_alphabetic() && matches!(u32::from(c), 0x400..=0x52F)
}

/// The language of a Cyrillic word: Ukrainian if it has і ї є ґ, else the document's `hint`.
/// A word with ы э ъ ё is Russian, which has no patterns here, and so is left unhyphenated,
/// like Bulgarian, Serbian and the rest.
pub fn cyrillic_word_language(word: &str, hint: Option<Cyrillic>) -> Option<Cyrillic> {
    let lower = word.to_lowercase();
    let uk = lower.chars().any(|c| matches!(c, 'і' | 'ї' | 'є' | 'ґ'));
    let other = lower.chars().any(|c| matches!(c, 'ы' | 'э' | 'ъ' | 'ё'));
    match (uk, other) {
        (true, false) => Some(Cyrillic::Ukrainian),
        (false, false) => hint,
        _ => None,
    }
}

/// The locale of each stretch of East Asian text in `text`, as byte ranges: Hangul is Korean
/// and kana is Japanese wherever they are, and Han ideographs and punctuation take the
/// paragraph's language, which is Japanese if it has any kana, else Korean if it has any
/// Hangul, else `hint` (the document language), else Simplified Chinese. Latin text gets no
/// range. Adjacent stretches with one locale are joined.
pub fn cjk_locale_ranges(
    text: &str,
    hint: Option<&'static str>,
) -> Vec<(Range<usize>, &'static str)> {
    let mut has_kana = false;
    let mut has_hangul = false;
    let mut has_east_asian = false;
    for c in text.chars() {
        match cjk_class(c) {
            Some(Cjk::Kana) => has_kana = true,
            Some(Cjk::Hangul) => has_hangul = true,
            Some(_) => has_east_asian = true,
            None => {}
        }
    }
    if !(has_kana || has_hangul || has_east_asian) {
        return Vec::new();
    }
    let shared = if has_kana {
        "ja"
    } else if has_hangul {
        "ko"
    } else {
        hint.unwrap_or("zh-CN")
    };
    let mut out: Vec<(Range<usize>, &'static str)> = Vec::new();
    for (at, c) in text.char_indices() {
        let Some(class) = cjk_class(c) else { continue };
        let locale = match class {
            Cjk::Kana => "ja",
            Cjk::Hangul => "ko",
            Cjk::Han | Cjk::Punct => shared,
        };
        let end = at + c.len_utf8();
        match out.last_mut() {
            Some((range, last)) if range.end == at && *last == locale => range.end = end,
            _ => out.push((at..end, locale)),
        }
    }
    out
}

/// True for a letter English hyphenation patterns apply to: Latin script, including accents.
/// Ideographs, kana and Hangul count as alphabetic in Unicode, but breaking them at English
/// syllables would put a hyphen inside a word.
pub fn is_latin_letter(c: char) -> bool {
    c.is_alphabetic()
        && matches!(u32::from(c), 0x41..=0x5A | 0x61..=0x7A | 0xAA | 0xB5 | 0xBA | 0xC0..=0x24F | 0x1E00..=0x1EFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces<'a>(text: &'a str, hint: Option<&'static str>) -> Vec<(&'a str, &'static str)> {
        cjk_locale_ranges(text, hint)
            .into_iter()
            .map(|(range, locale)| (&text[range], locale))
            .collect()
    }

    #[test]
    fn latin_text_has_no_locale() {
        assert!(cjk_locale_ranges("Hello, world. Café naïve.", None).is_empty());
        assert!(cjk_locale_ranges("", Some("ja")).is_empty());
    }

    #[test]
    fn han_alone_follows_the_hint_and_defaults_to_simplified_chinese() {
        assert_eq!(pieces("你好，世界", None), [("你好，世界", "zh-CN")]);
        assert_eq!(
            pieces("你好，世界", Some("zh-TW")),
            [("你好，世界", "zh-TW")]
        );
        assert_eq!(pieces("漢字", Some("ja")), [("漢字", "ja")]);
        assert_eq!(
            pieces("한자 漢字", Some("zh-CN")),
            [("한자", "ko"), ("漢字", "ko")]
        );
    }

    #[test]
    fn kana_makes_the_paragraph_japanese_whatever_the_hint() {
        assert_eq!(
            pieces("吾輩は猫である。", Some("zh-CN")),
            [("吾輩は猫である。", "ja")]
        );
        // Latin words between East Asian runs split them, and each run keeps the locale.
        assert_eq!(
            pieces("日本語 and カタカナ", None),
            [("日本語", "ja"), ("カタカナ", "ja")]
        );
    }

    #[test]
    fn hangul_makes_the_ideographs_around_it_korean() {
        // Korean text carries Hanja, so ideographs next to Hangul are Korean forms even in a
        // document set to Chinese.
        assert_eq!(
            pieces("안녕하세요 你好", Some("zh-CN")),
            [("안녕하세요", "ko"), ("你好", "ko")]
        );
    }

    #[test]
    fn ranges_are_valid_byte_ranges_on_char_boundaries() {
        let text = "a你b好c한d";
        for (range, _) in cjk_locale_ranges(text, None) {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn tags_map_to_locales() {
        for (tag, expected) in [
            ("ja", Some("ja")),
            ("ja-JP", Some("ja")),
            ("ko-KR", Some("ko")),
            ("zh", Some("zh-CN")),
            ("zh-CN", Some("zh-CN")),
            ("zh-Hans", Some("zh-CN")),
            ("zh-SG", Some("zh-CN")),
            ("zh-TW", Some("zh-TW")),
            ("zh_hk", Some("zh-TW")),
            ("zh-Hant-TW", Some("zh-TW")),
            ("en-GB", None),
            ("fr", None),
            ("", None),
        ] {
            assert_eq!(locale_for_tag(tag), expected, "{tag:?}");
        }
    }

    #[test]
    fn a_number_stands_upright_unless_it_belongs_to_a_latin_word() {
        // Every character of these numbers stands upright.
        for text in [
            "2026年",
            "第3章",
            "価格は3.14円",
            "2026-11-23日",
            "45%です",
            "12:30に",
            " 7 ",
        ] {
            for (at, c) in text.char_indices() {
                let numeric = c.is_ascii_digit() || matches!(c, '.' | ':' | '-' | '%');
                if numeric {
                    assert!(number_stands_upright(text, at), "{text:?} at {at} ({c})");
                }
            }
        }
        // Touching a Latin letter keeps the whole number sideways with its word.
        for text in [
            "B2B", "x2", "COVID-19", "A123", "mp3", "ver 1.2b", "mp-3", "3_d",
        ] {
            for (at, c) in text.char_indices() {
                assert!(!number_stands_upright(text, at), "{text:?} at {at} ({c})");
            }
        }
        // A joiner with no digit beside it, and letters, are not numbers at all.
        for (text, at) in [
            ("a-b", 1),
            ("-5", 0),
            ("5.", 1),
            ("abc", 1),
            ("日本語", 0),
            ("", 0),
        ] {
            assert!(!number_stands_upright(text, at), "{text:?} at {at}");
        }
        // An offset inside a character, or past the end, is simply not a number.
        assert!(!number_stands_upright("日5", 1));
        assert!(!number_stands_upright("5", 9));
    }

    #[test]
    fn the_upright_numbers_of_a_text_are_found_as_ranges() {
        let text = "第3章、2026年11月、B2Bと45%";
        let found: Vec<&str> = upright_number_ranges(text)
            .into_iter()
            .map(|range| &text[range])
            .collect();
        // `11` is a two-digit number set in one cell, so it is not stacked.
        assert_eq!(found, ["3", "2026", "45%"]);
        assert!(upright_number_ranges("no numbers, B2B and COVID-19").is_empty());
        assert!(upright_number_ranges("").is_empty());
    }

    #[test]
    fn a_two_digit_number_is_set_horizontally_in_one_cell() {
        let text = "11月23日、2026年、3.14、45%、第3章、B22、 99";
        let mut pairs: Vec<&str> = text
            .char_indices()
            .filter_map(|(at, _)| tcy_pair_at(text, at))
            .map(|range| &text[range])
            .collect();
        // Both digits of a pair give the same range.
        pairs.dedup();
        assert_eq!(pairs, ["11", "23", "99"]);
        // The range is the whole pair from either digit.
        assert_eq!(tcy_pair_at("11月", 0), Some(0..2));
        assert_eq!(tcy_pair_at("11月", 1), Some(0..2));
        // One digit, three digits, a decimal and a percentage are not pairs; a number that
        // touches a Latin word goes sideways, so it is not either.
        for (text, at) in [
            ("3章", 0),
            ("2026年", 1),
            ("3.14", 2),
            ("45%", 0),
            ("B22", 2),
        ] {
            assert_eq!(tcy_pair_at(text, at), None, "{text}");
        }
    }

    #[test]
    fn only_latin_letters_take_english_hyphenation() {
        for c in "aZéÀñǿẞḀ".chars() {
            assert!(is_latin_letter(c), "{c}");
        }
        for c in "你ひカ한ЖΩ1-".chars() {
            assert!(!is_latin_letter(c), "{c}");
        }
    }
}
