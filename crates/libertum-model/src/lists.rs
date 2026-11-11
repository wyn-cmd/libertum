//! Bulleted and numbered lists (PLAN §4.1, Phase 2).
//!
//! A list paragraph carries a [`ListItem`]: the list's style, the item's level (0 to 8, as
//! DOCX `w:ilvl`) and a *group*: paragraphs of one group are numbered as one list, even with
//! other paragraphs between them (DOCX `w:numId`, ODT `text:list` with `text:continue-list`).
//! Numbers are not stored; [`Document::list_labels`](crate::Document::list_labels) works
//! them out in one pass, like a field.

/// The built-in list styles (the Format inspector's Bullets & Lists popup).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ListStyle {
    /// •, ◦, ▪ by level.
    Bullet,
    /// 1. a. i. by level.
    Numbered,
    /// A. a. i. by level.
    Lettered,
    /// I. A. 1. a. (1) (a) i. by level (Harvard outline).
    Harvard,
}

/// Deepest list level (0-based; nine levels, as DOCX and ODT).
pub const MAX_LIST_LEVEL: u8 = 8;
/// Left indent added per list level, points (half an inch, as Word).
pub const LIST_INDENT_PT: f32 = 36.0;
/// Room for the bullet or number to the left of the text, points.
pub const LIST_HANGING_PT: f32 = 18.0;

/// A paragraph's place in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ListItem {
    pub style: ListStyle,
    /// 0 to [`MAX_LIST_LEVEL`].
    pub level: u8,
    /// Paragraphs with the same group number form one list.
    pub group: u64,
}

impl ListItem {
    /// Left indent of the item's text: one step per level, starting at one step.
    pub fn indent_left_pt(&self) -> f32 {
        LIST_INDENT_PT * f32::from(self.level.min(MAX_LIST_LEVEL) + 1)
    }
}

/// Number formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Bullet(&'static str),
    Decimal,
    DecimalParen,
    UpperLetter,
    LowerLetter,
    LowerLetterParen,
    UpperRoman,
    LowerRoman,
}

impl ListStyle {
    fn format(self, level: u8) -> Format {
        let level = usize::from(level.min(MAX_LIST_LEVEL));
        match self {
            ListStyle::Bullet => Format::Bullet(["•", "◦", "▪"][level % 3]),
            ListStyle::Numbered => {
                [Format::Decimal, Format::LowerLetter, Format::LowerRoman][level % 3]
            }
            ListStyle::Lettered => {
                [Format::UpperLetter, Format::LowerLetter, Format::LowerRoman][level % 3]
            }
            ListStyle::Harvard => [
                Format::UpperRoman,
                Format::UpperLetter,
                Format::Decimal,
                Format::LowerLetter,
                Format::DecimalParen,
                Format::LowerLetterParen,
                Format::LowerRoman,
            ][level % 7],
        }
    }

    /// The marker of item number `n` (1-based) at `level`.
    pub fn label(self, level: u8, n: u32) -> String {
        match self.format(level) {
            Format::Bullet(b) => b.to_owned(),
            Format::Decimal => format!("{n}."),
            Format::DecimalParen => format!("({n})"),
            Format::UpperLetter => format!("{}.", letters(n).to_uppercase()),
            Format::LowerLetter => format!("{}.", letters(n)),
            Format::LowerLetterParen => format!("({})", letters(n)),
            Format::UpperRoman => format!("{}.", roman(n).to_uppercase()),
            Format::LowerRoman => format!("{}.", roman(n)),
        }
    }
}

/// a, b, ..., z, aa, bb, ... (Word's lettering).
fn letters(n: u32) -> String {
    let n = n.max(1) - 1;
    let ch = char::from(b'a' + (n % 26) as u8);
    std::iter::repeat_n(ch, (n / 26 + 1) as usize).collect()
}

/// Lower-case Roman numerals; numbers past 3999 fall back to decimal.
fn roman(n: u32) -> String {
    if n == 0 || n > 3999 {
        return n.to_string();
    }
    const TABLE: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (value, digits) in TABLE {
        while n >= value {
            out.push_str(digits);
            n -= value;
        }
    }
    out
}

/// Numbers every list item in document order: per group, a counter per level; an item
/// resets the counters of the levels below it. `items` yields each paragraph's list item.
pub fn number_items<'a>(items: impl Iterator<Item = Option<&'a ListItem>>) -> Vec<Option<String>> {
    let mut counters: std::collections::HashMap<u64, [u32; MAX_LIST_LEVEL as usize + 1]> =
        std::collections::HashMap::new();
    items
        .map(|item| {
            let item = item?;
            let level = usize::from(item.level.min(MAX_LIST_LEVEL));
            let c = counters.entry(item.group).or_default();
            c[level] += 1;
            for deeper in c.iter_mut().skip(level + 1) {
                *deeper = 0;
            }
            Some(item.style.label(item.level, c[level]))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_by_style_and_level() {
        assert_eq!(ListStyle::Numbered.label(0, 3), "3.");
        assert_eq!(ListStyle::Numbered.label(1, 2), "b.");
        assert_eq!(ListStyle::Numbered.label(2, 4), "iv.");
        assert_eq!(ListStyle::Lettered.label(0, 28), "BB.");
        assert_eq!(ListStyle::Harvard.label(0, 14), "XIV.");
        assert_eq!(ListStyle::Harvard.label(4, 2), "(2)");
        assert_eq!(ListStyle::Bullet.label(1, 9), "◦");
        assert_eq!(roman(1994), "mcmxciv");
    }

    #[test]
    fn numbering_nests_and_continues_across_other_paragraphs() {
        let item = |level, group| {
            Some(ListItem {
                style: ListStyle::Numbered,
                level,
                group,
            })
        };
        let items = [
            item(0, 1),
            item(1, 1),
            item(1, 1),
            None,
            item(0, 1),
            item(1, 1),
            item(0, 2),
        ];
        let labels = number_items(items.iter().map(Option::as_ref));
        let want = ["1.", "a.", "b.", "", "2.", "a.", "1."];
        for (got, want) in labels.iter().zip(want) {
            assert_eq!(got.as_deref().unwrap_or(""), want);
        }
    }
}
