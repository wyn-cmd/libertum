//! Named anchors inside a paragraph (DOCX `w:bookmarkStart`, ODT `text:bookmark`, RTF
//! `\*\bkmkstart`).
//!
//! A bookmark is a zero-width point: a name and the byte offset in its paragraph. Ranges
//! are not modelled; the formats' start/end pairs collapse to their start, because Libertum
//! only needs a target to navigate to and to resolve cross-references against.

/// A named point in a paragraph.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    /// The name as the document stores it. Word's hidden names begin with `_`.
    pub name: String,
    /// Byte offset in the paragraph's text.
    pub offset: usize,
}
