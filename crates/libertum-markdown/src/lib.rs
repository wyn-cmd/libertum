//! Markdown import and export (PLAN §7).
//!
//! Libertum edits Markdown in the same WYSIWYG editor as DOCX and ODT. The hard part is
//! round-tripping the user's own file: regenerating Markdown from the model would change
//! emphasis markers, re-wrap lines and renumber lists. So the importer keeps the original
//! source and remembers every top-level block's byte slice; a block that has not been edited
//! is written back exactly as it arrived. Only edited blocks are re-serialised, following the
//! conventions detected in the file (bullet character, fence character, ATX/Setext headings,
//! ordered-list delimiter).
//!
//! The parser is `comrak` with the GFM extensions (tables, strikethrough, task lists,
//! autolinks, footnotes) and source positions. Front matter and raw HTML blocks are kept as
//! text; clean ones are re-emitted byte for byte.
//!
//! ## Images
//!
//! A file opened with [`Markdown::open`] (or [`Markdown::parse_with_base_dir`]) loads each
//! `![alt](target)` whose target is a local file that exists and decodes: the model gets an
//! inline image (`name` is the target as written, `alt` the alt text) and the image syntax is
//! not also left in the text. A URL, a missing file or an undecodable one is shown as its alt
//! text and noted in [`Markdown::warnings`]; the network is never used. [`Markdown::parse`]
//! has no folder to look in, so it loads no image.
//!
//! An edited block writes `![alt](name "title")` at each image's position, and
//! `[![alt](name)](target)` when the image was wrapped in a link. An image with no `name`
//! (inserted in the app) is written beside the document as `<document-stem>-image-<n>.<ext>`
//! when the save has a path ([`Markdown::to_bytes_for`], [`Markdown::save_with_report`]);
//! otherwise its alt text is written and a warning is returned. An image's displayed size is
//! not kept.
//!
//! ## Formatting Markdown has no syntax for
//!
//! Underline, superscript, subscript and text colour are written as inline HTML
//! (`<u>`, `<sup>`, `<sub>`, `<span style="color:#rrggbb">`), and those tags are read back as
//! formatting on open. Other inline HTML is still shown as text.
//!
//! ## What a Markdown save drops
//!
//! [`loss_report`] lists the formatting of a document that Markdown cannot hold.

mod loss;
mod parse;
mod write;

use std::path::Path;

use libertum_model::{Block, BlockId, Document};

pub use loss::{Loss, LossKind, loss_report};
pub use parse::Conventions;
pub(crate) use parse::Origin;

#[derive(Debug, thiserror::Error)]
pub enum MarkdownError {
    #[error("the file is not valid UTF-8 Markdown: {0}")]
    Encoding(String),
    #[error("could not read or write the file: {0}")]
    Io(#[from] std::io::Error),
}

/// An image file written beside the Markdown file (an image that had no file of its own).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownAsset {
    /// File name in the document's folder, `<document-stem>-image-<n>.<ext>`.
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// The result of writing a document as Markdown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedMarkdown {
    /// The Markdown text.
    pub bytes: Vec<u8>,
    /// Image files that must be written next to the document for its links to work.
    pub assets: Vec<MarkdownAsset>,
    /// Things that could not be written as they were (an image with nowhere to be saved).
    pub warnings: Vec<String>,
}

/// A Markdown document together with the source it came from.
#[derive(Clone)]
pub struct Markdown {
    document: Document,
    origin: Option<Origin>,
    original: Vec<u8>,
    warnings: Vec<String>,
}

impl Markdown {
    /// Parses Markdown text. The bytes are kept as they arrived so a clean save is identical.
    /// With no folder to resolve relative targets against, no image is loaded (each is shown
    /// as its alt text); use [`parse_with_base_dir`](Self::parse_with_base_dir) for that.
    pub fn parse(bytes: &[u8]) -> Result<Self, MarkdownError> {
        Self::parse_with_base_dir(bytes, None)
    }

    /// Parses Markdown text, loading local images relative to `base_dir` (absolute targets
    /// are used as they are).
    pub fn parse_with_base_dir(
        bytes: &[u8],
        base_dir: Option<&Path>,
    ) -> Result<Self, MarkdownError> {
        let (document, origin, warnings) = parse::parse(bytes, base_dir)?;
        Ok(Self {
            document,
            origin: Some(origin),
            original: bytes.to_vec(),
            warnings,
        })
    }

    /// Reads a Markdown file from disk. Images it links to are loaded from beside it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, MarkdownError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let base_dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        Self::parse_with_base_dir(&bytes, Some(base_dir))
    }

    /// Wraps a document for export as new Markdown.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            origin: None,
            original: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn set_document(&mut self, document: Document) {
        self.document = document;
    }

    pub fn into_document(self) -> Document {
        self.document
    }

    /// Non-fatal things noticed while reading (flattened links and images, line breaks).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Writes the document as Markdown. A document that has not been edited is returned
    /// byte for byte; otherwise unchanged blocks keep their original source and edited
    /// blocks are re-serialised with the file's own conventions.
    ///
    /// There is no path here, so an image without a file name is written as its alt text;
    /// use [`to_bytes_for`](Self::to_bytes_for) to have it saved beside the document.
    pub fn to_bytes(&self) -> Result<Vec<u8>, MarkdownError> {
        Ok(self.to_bytes_for(None)?.bytes)
    }

    /// Like [`to_bytes`](Self::to_bytes), for a document that will be written to
    /// `document_path`: images with no file name are returned as [`MarkdownAsset`]s named
    /// `<document-stem>-image-<n>.<ext>` (the next free `n`; an existing file with the same
    /// bytes is reused, one with other bytes is never overwritten), and the text links to
    /// them. Without a path those images are written as alt text, with a warning.
    pub fn to_bytes_for(
        &self,
        document_path: Option<&Path>,
    ) -> Result<SavedMarkdown, MarkdownError> {
        let mut sink = write::Sink::new(document_path);
        let bytes = self.render(&mut sink);
        Ok(SavedMarkdown {
            bytes,
            assets: sink.assets,
            warnings: sink.warnings,
        })
    }

    fn render(&self, sink: &mut write::Sink) -> Vec<u8> {
        let Some(origin) = &self.origin else {
            return write::serialise_document(
                &self.document,
                &parse::Hints::default(),
                Conventions::default(),
                sink,
            )
            .into_bytes();
        };
        if write::fingerprint_document(&self.document) == origin.document_fingerprint {
            return self.original.clone();
        }

        let blocks: Vec<&Block> = self.document.body().iter().collect();
        let ids: Vec<BlockId> = blocks.iter().map(|block| block.id()).collect();
        let mut source_ids = Vec::new();
        for group in &origin.groups {
            source_ids.extend(group.ids.iter().copied());
        }
        if ids != source_ids {
            // The block structure changed (a paragraph was added, removed or moved), so the
            // per-block slices no longer line up. Serialise the whole document.
            return write::serialise_document(
                &self.document,
                &origin.hints,
                origin.conventions,
                sink,
            )
            .into_bytes();
        }

        let mut out = origin.prefix.clone();
        let mut index = 0usize;
        for group in &origin.groups {
            let end = index + group.ids.len();
            let current = &blocks[index..end];
            if write::fingerprint_blocks(current) == group.fingerprint {
                out.push_str(&group.source);
            } else {
                let mut piece = write::serialise_group(
                    &self.document,
                    current,
                    &origin.hints,
                    origin.conventions,
                    sink,
                );
                if !piece.ends_with('\n') {
                    piece.push('\n');
                }
                // Keep the spacing the block had: one that was not followed by a blank line
                // (the last block, or one packed against the next) does not gain one.
                let newlines = group
                    .source
                    .chars()
                    .rev()
                    .take_while(|c| matches!(c, '\n' | '\r'))
                    .filter(|c| *c == '\n')
                    .count();
                if newlines <= 1 {
                    while piece.ends_with("\n\n") {
                        piece.pop();
                    }
                }
                out.push_str(&piece);
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            index = end;
        }
        out.into_bytes()
    }

    /// Writes the document to a file. Image files that belong beside it are written too (see
    /// [`to_bytes_for`](Self::to_bytes_for)).
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), MarkdownError> {
        self.save_with_report(path).map(|_| ())
    }

    /// Like [`save`](Self::save), and returns the warnings about anything that could not be
    /// written as it was. The text is written last, so a failure while writing an image
    /// leaves the existing file untouched.
    pub fn save_with_report(&self, path: impl AsRef<Path>) -> Result<Vec<String>, MarkdownError> {
        let path = path.as_ref();
        let saved = self.to_bytes_for(Some(path))?;
        let dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => std::path::PathBuf::from("."),
        };
        for asset in &saved.assets {
            std::fs::write(dir.join(&asset.file_name), &asset.bytes)?;
        }
        std::fs::write(path, &saved.bytes)?;
        Ok(saved.warnings)
    }
}
