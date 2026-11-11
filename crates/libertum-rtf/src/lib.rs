//! RTF import and export (PLAN §6.5).
//!
//! RTF is a plain-text interchange format: one stream of groups (`{ … }`), control words
//! (`\word` with an optional numeric parameter), control symbols, `\'hh` bytes and text. It
//! needs no zip or XML stack, and every word processor can read and write it.
//!
//! [`Rtf::parse`] reads a stream through a byte-level tokeniser into a [`Document`] and keeps
//! the raw bytes of everything it does not model, so [`Rtf::to_bytes`] writes them back.
//! [`Rtf::new`] wraps a model document for export and builds a fresh RTF 1.9.1 stream.
//!
//! # What is mapped
//!
//! - **Text and runs:** `\par`, `\line` (a space), `\tab`, `\page`, bold, italic, underline,
//!   strike-through, super/subscript, `\fs` half-points, `\f` through the font table, `\cf`
//!   and `\highlight` through the colour table, and `\expnd` letter spacing.
//! - **Paragraphs:** `\ql`/`\qc`/`\qr`/`\qj`, `\li`/`\ri`/`\fi`, `\sb`/`\sa`, `\sl`/`\slmult`,
//!   `\keep`/`\keepn`, `\widctlpar`, `\pagebb`, explicit tab stops, and `\s` paragraph
//!   styles resolved through `\stylesheet`. Built-in names (Normal/Body, Title, Heading 1 to
//!   3, Quote) map to the model's styles; other styles are appended to the model style sheet.
//! - **Lists:** `\listtable`/`\listoverridetable`, `\ls`, `\ilvl`, and `\listtext`/`\pntext`
//!   markers (which are skipped, because the model regenerates markers). A list's level
//!   format chooses one of the model's four list styles; the `\ls` id becomes the model group,
//!   so numbering continues per list instance.
//! - **Tables:** `\trowd`, `\cellx`, `\cell`, `\row`, `\trhdr`, cell fills, horizontal
//!   merges (`\clmgf`/`\clmrg`) and vertical merges (`\clvmgf`/`\clvmrg`). Every cell gets at
//!   least one paragraph. Nested tables are refused with [`RtfError::Unsupported`] rather
//!   than flattened.
//! - **Sections and page setup:** `\sectd`, `\paperw`, `\paperh`, `\landscape`, `\portrait`,
//!   `\margl`/`\margr`/`\margt`/`\margb`, `\cols`/`\colsx`, `\headery`/`\footery`,
//!   `\titlepg`, `\pgnstart` and `\sect`. The model's sections are master-page-like, so they
//!   map one to one.
//! - **Headers and footers:** `\header`, `\footer`, `\headerf`, `\footerf`, `\headerl`,
//!   `\footerl` (and the right-hand variants) map to the default, first-page and even-page
//!   variants. `PAGE` and `NUMPAGES` fields become the model's `{page}` and `{pages}` tokens.
//!   The model's furniture is one plain-text paragraph, so several header paragraphs are
//!   joined with a space (with a warning).
//! - **Fields:** in a header or footer, `PAGE` and `NUMPAGES` become tokens on import and are
//!   regenerated as `\field` groups with a cached `\fldrslt` on export. In body text a
//!   `PAGE`/`NUMPAGES` field becomes its cached result as plain text (with a warning), and a
//!   `TOC` or other unknown field is refused with [`RtfError::Unsupported`], because the model
//!   has no field node that a save could preserve without duplicating the cached text.
//!   `REF` and `PAGEREF` (with or without switches) become cross-references (kind Text or
//!   Page) over their cached result and are written back as `ield` groups.
//! - **Pictures:** `\pict` with `\pngblip` or `\jpegblip` (hex or `\binN`) becomes an inline
//!   image at its byte offset, sized from `\picwgoal` times scale, else pixels. They are
//!   written in the bare `{\pict ...}` form, which Word, WordPad, LibreOffice and Pages read,
//!   with alt text and name in `\*\picprop`. Of a `\shppict`/`\nonshppict` pair only one is read.
//! - **Table styles:** the style name is carried in `{\*\libertumtblstyle Name}` and the
//!   resolved fills are written into each cell, so other readers see the same colours. On
//!   import a built-in name sets the style and clears the fills it resolves to.
//! - **Rows:** `\trkeep` maps to `TableRow::keep_together`; absent means the row may split.
//! - **Notes:** `{\footnote …}` and `{\endnote …}` destinations are read as plain-text notes
//!   anchored at the offset where the reference appeared, and written back as destination
//!   groups at each note's byte offset in the paragraph. Several notes of both kinds may sit
//!   on one paragraph. Word's `{\footnote\ftnalt …}` form is read as an endnote and is what export writes.
//! - **Encoding:** `\uN` is decoded directly (surrogate pairs included), `\ucN` fallback bytes
//!   are skipped, and raw bytes are decoded through `encoding_rs` under `\ansicpgN`. CP1252
//!   and UTF-8 are always available; other Windows and ANSI pages use `encoding_rs`'s labels,
//!   and an unknown page falls back to CP1252 with a warning. Export writes ASCII only and
//!   encodes every non-ASCII character as `\uN?`.
//!
//! # What is preserved
//!
//! There is no package to keep, so preservation works on the token stream:
//!
//! - An unknown top-level group or destination (for example `{\info …}`, `{\*\generator …}`,
//!   `{\*\themedata …}`) is copied byte for byte into a **preamble** and written back after the
//!   font, colour and style tables.
//! - An unknown group or control word met while reading a paragraph, run or table cell is
//!   copied byte for byte and attached to that paragraph, at the byte offset in the paragraph
//!   where it appeared. It is written back at the same offset. If the paragraph is deleted,
//!   its snippets go with it. Unknown field groups, revision marks and unknown destinations
//!   survive this way; bookmarks (`\*\bkmkstart`/`\*\bkmkend`) are read into the model and
//!   written back at their offsets.
//! - An unknown group inside a header or footer is kept per section, header/footer kind and
//!   variant, and written back inside the same furniture group.
//! - Font, colour and style tables are **regenerated** from the model, like the DOCX and ODT
//!   writers. A preserved snippet that refers to an old font, colour or style index may
//!   therefore resolve differently after an edit that changes those tables; the bytes survive,
//!   but their table references are not remapped.
//!
//! # What is refused
//!
//! Metafile and other non-PNG/JPEG pictures, cropped pictures, pictures in headers and notes,
//! drawing shapes (`\shp`), objects (`\object`), annotations/comments and nested tables
//! produce [`RtfError::Unsupported`], matching the DOCX and ODT importers' rule that content
//! the model cannot carry makes the open fail rather than disappearing on the next save. A body
//! field other than `PAGE`/`NUMPAGES`/`REF`/`PAGEREF` is refused for the same reason.
//!
//! # Known approximations
//!
//! - RTF has no separate document-default character and paragraph formatting: Word's Normal
//!   style *is* the default. Export folds [`StyleSheet::defaults`] and [`StyleSheet::para_defaults`]
//!   into the Normal style; import puts that formatting back into the defaults and leaves the
//!   Body style empty. Resolved formatting is unchanged, but a Body style that carried its own
//!   direct formatting is normalised into the document defaults.
//! - Lengths are written in twips (1/20 pt), font sizes in half-points and weights as
//!   bold/regular, as RTF requires, so a value that is not on those grids is rounded.
//! - Line spacing: RTF can express a multiple of the font's line height, an exact height and
//!   (with a negative `\sl`) a minimum height. [`LineSpacing::Leading`] is written as an exact
//!   height (1.2 × the default font size plus the extra) and comes back as `Exactly`.
//! - Paragraph borders, ligatures and the model's separately drawn page numbers have no RTF
//!   mapping here; they are not written and do not survive a round trip. (Header/footer page
//!   fields and `{page}`/`{pages}` tokens do.) Tab stops and per-paragraph hyphenation are
//!   written as `\tx`/`\hyphpar` and read back.
//! - Notes are plain text. Formatting inside a footnote or endnote is flattened on import
//!   (with a warning) and is not written back, so the note body is not a styled paragraph.
//!   Paragraph breaks inside a note become newlines (`\line` on export). The reference is
//!   written exactly at the note's byte offset, snapped back to a character boundary; an
//!   offset past the end of the paragraph puts the group at the paragraph end, and an offset
//!   that falls inside a multi-byte character is snapped to the start of that character. Notes
//!   in a header or footer are dropped with a warning, because the model's furniture is plain
//!   text with no notes. RTF itself encodes an endnote as `\footnote\ftnalt`; export uses that Word form.

mod encoding;
mod read;
mod token;
mod write;

use std::collections::HashMap;
use std::path::Path;

use libertum_model::{BlockId, Document, HfKind, HfVariant};

/// The RTF version this writer emits in its `\rtf1` header (RTF 1.9.1, Word 2007 and later).
pub const RTF_VERSION: u32 = 1;

/// Errors from reading or writing RTF. Parsing never panics, and no I/O path unwraps.
#[derive(Debug, thiserror::Error)]
pub enum RtfError {
    #[error("invalid RTF at byte {offset}: {message}")]
    Parse { offset: usize, message: String },
    #[error("this document contains {what}, which Libertum cannot save yet")]
    Unsupported { what: String },
    #[error("could not read or write the file: {0}")]
    Io(#[from] std::io::Error),
}

impl RtfError {
    pub(crate) fn parse(offset: usize, message: impl Into<String>) -> Self {
        Self::Parse {
            offset,
            message: message.into(),
        }
    }

    pub(crate) fn unsupported(what: impl Into<String>) -> Self {
        Self::Unsupported { what: what.into() }
    }
}

/// A raw RTF fragment kept at a byte offset in one paragraph's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Snippet {
    pub(crate) offset: usize,
    pub(crate) raw: Vec<u8>,
}

/// Identity of a preserved header or footer fragment.
pub(crate) type HeaderKey = (usize, HfKind, HfVariant);

/// An RTF document: the model document plus the raw fragments Libertum does not model.
#[derive(Clone, Debug)]
pub struct Rtf {
    document: Document,
    /// Unknown top-level groups/destinations, written back after the generated tables.
    preamble: Vec<Vec<u8>>,
    /// Raw fragments attached to paragraphs, written back at their original byte offsets.
    snippets: HashMap<BlockId, Vec<Snippet>>,
    /// Raw fragments attached to a section's header or footer variant.
    header_snippets: HashMap<HeaderKey, Vec<Vec<u8>>>,
    /// Character and table style groups from the source style sheet, kept verbatim and put
    /// back inside the regenerated style sheet.
    stylesheet_extras: Vec<Vec<u8>>,
    warnings: Vec<String>,
    /// The source stream, kept as the "original" for callers that want to know it existed.
    source: Option<Vec<u8>>,
}

impl Rtf {
    /// Reads an RTF stream from memory.
    pub fn parse(bytes: &[u8]) -> Result<Self, RtfError> {
        let parsed = read::read(bytes)?;
        Ok(Self {
            document: parsed.document,
            preamble: parsed.preamble,
            snippets: parsed.snippets,
            header_snippets: parsed.header_snippets,
            stylesheet_extras: parsed.stylesheet_extras,
            warnings: parsed.warnings,
            source: Some(bytes.to_vec()),
        })
    }

    /// Reads an RTF file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RtfError> {
        let bytes = std::fs::read(path)?;
        Self::parse(&bytes)
    }

    /// Wraps a document for export. The first save writes a complete RTF stream.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            preamble: Vec::new(),
            snippets: HashMap::new(),
            header_snippets: HashMap::new(),
            stylesheet_extras: Vec::new(),
            warnings: Vec::new(),
            source: None,
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Replaces the model document. Preserved fragments stay attached to paragraph and section
    /// ids, so a paragraph or section that survives keeps its raw content.
    pub fn set_document(&mut self, document: Document) {
        self.document = document;
    }

    pub fn into_document(self) -> Document {
        self.document
    }

    /// Non-fatal things noticed while reading (code-page fallbacks, flattened lines, cached
    /// fields and the like).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The original source stream, when this value was parsed from one.
    pub fn source(&self) -> Option<&[u8]> {
        self.source.as_deref()
    }

    /// Writes the document as an RTF 1.9.1 stream.
    pub fn to_bytes(&self) -> Result<Vec<u8>, RtfError> {
        write::build(
            &self.document,
            &self.preamble,
            &self.snippets,
            &self.header_snippets,
            &self.stylesheet_extras,
        )
    }

    /// Writes the document to a file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), RtfError> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_stream_parses_to_one_empty_paragraph() {
        let rtf = Rtf::parse(br"{\rtf1\ansi hello\par}").expect("parse");
        let doc = rtf.document();
        assert_eq!(doc.len(), 1);
        assert_eq!(doc.paragraph_at(0).expect("paragraph").text(), "hello");
    }

    #[test]
    fn a_bad_stream_is_a_typed_error_not_a_panic() {
        assert!(matches!(
            Rtf::parse(br"{\rtf1 unterminated"),
            Err(RtfError::Parse { .. })
        ));
    }
}
