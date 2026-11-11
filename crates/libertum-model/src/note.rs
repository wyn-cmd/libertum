//! Footnotes and endnotes attached to a paragraph.
//!
//! A note is anchored at a byte offset in its paragraph and carries its own plain text.
//! Numbers are computed in document order; they are never stored.

/// A note kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum NoteKind {
    Footnote,
    Endnote,
}

/// One note anchored in a paragraph.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Note {
    pub kind: NoteKind,
    /// Byte offset in the paragraph's text where the reference sits.
    pub offset: usize,
    pub text: String,
}
