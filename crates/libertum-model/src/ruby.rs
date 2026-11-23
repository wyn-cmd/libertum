//! Ruby: a reading set beside a stretch of text, such as furigana over kanji or pinyin over
//! hanzi. Above the base text in horizontal lines and to its right in vertical columns.

use std::ops::Range;

/// A reading over part of a paragraph's text.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ruby {
    /// The bytes of the paragraph's text the reading belongs to (the base).
    pub range: Range<usize>,
    /// The reading, shown at a smaller size beside the base.
    pub text: String,
}

impl Ruby {
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }
}
