//! HTML import and export (AO3 functions plan, Phase 12 stage A).
//!
//! Reading uses html5ever's parser with a small tree of our own ([`dom`]), so any HTML a
//! browser would open reads without a panic. The reader maps paragraphs, headings, emphasis,
//! inline CSS, block quotes, lists, tables, images, footnotes and bookmarks onto the model
//! ([`read`]). A file downloaded from AO3 is recognised and read by the rules in [`ao3`]:
//! the story and the author's summaries and notes are kept, the preface's metadata and the
//! afterword's links are not. [`import_epub`] reads an EPUB's chapters with the same reader.
//!
//! Writing has two flavours ([`HtmlFlavour`]): a full page that keeps the look of the text
//! and reads back with little loss, and plain semantic HTML. [`loss_report`] lists what each
//! drops.
//!
//! The network is never used: a remote image is shown as its alt text, with a warning.

mod ao3;
mod base64;
mod css;
mod dom;
mod epub;
mod loss;
mod read;
mod style;
mod util;
mod write;

use std::path::Path;

use libertum_model::Document;

pub use epub::{import_epub, import_epub_bytes};
pub use loss::{Loss, LossKind, loss_report};

#[derive(Debug, thiserror::Error)]
pub enum HtmlError {
    #[error("could not read or write the file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the EPUB could not be read: {0}")]
    Epub(String),
}

/// How much a save keeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum HtmlFlavour {
    /// A standalone page with the formatting as CSS; reopens with little loss.
    #[default]
    Full,
    /// Semantic tags only: no fonts, colours, spacing or page colour.
    Plain,
}

/// A document read from, or to be written as, HTML.
#[derive(Clone)]
pub struct Html {
    document: Document,
    warnings: Vec<String>,
    flavour: HtmlFlavour,
    ao3: bool,
}

impl Html {
    /// Wraps a document for a new HTML file (full flavour).
    pub fn new(document: Document) -> Self {
        Self {
            document,
            warnings: Vec::new(),
            flavour: HtmlFlavour::Full,
            ao3: false,
        }
    }

    /// Parses HTML text. Local images are loaded relative to `base_dir`; without one, only
    /// images embedded as `data:` URIs load.
    pub fn parse(source: &str, base_dir: Option<&Path>) -> Self {
        let source = source.strip_prefix('\u{feff}').unwrap_or(source);
        let dom = dom::Dom::parse(source);
        let resolve = |src: &str| load_local(base_dir, src);
        let result = if base_dir.is_some() {
            read::read(&dom, Some(&resolve), true)
        } else {
            read::read(&dom, None, true)
        };
        // Libertum's plain flavour is the only HTML it writes without a style block.
        let flavour = if source.contains("<meta name=\"generator\" content=\"Libertum\">")
            && !source.contains("<style>")
        {
            HtmlFlavour::Plain
        } else {
            HtmlFlavour::Full
        };
        Self {
            document: result.document,
            warnings: result.warnings,
            flavour,
            ao3: result.ao3,
        }
    }

    /// Parses HTML bytes: UTF-8 (a byte-order mark is skipped). Bytes that are not UTF-8
    /// are read with replacement characters and a warning.
    pub fn parse_bytes(bytes: &[u8], base_dir: Option<&Path>) -> Self {
        match std::str::from_utf8(bytes) {
            Ok(text) => Self::parse(text, base_dir),
            Err(_) => {
                let text = String::from_utf8_lossy(bytes);
                let mut html = Self::parse(&text, base_dir);
                html.warnings.insert(
                    0,
                    "the file is not UTF-8; characters that could not be read are shown as \u{fffd}"
                        .to_owned(),
                );
                html
            }
        }
    }

    /// Reads an HTML file. Images it links to are loaded from beside it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, HtmlError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)?;
        let base_dir = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            _ => Path::new("."),
        };
        Ok(Self::parse_bytes(&bytes, Some(base_dir)))
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn set_document(&mut self, document: Document) {
        self.document = document;
    }

    pub fn into_document(self) -> Document {
        self.document
    }

    /// Non-fatal things noticed while reading (images not loaded, encoding).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// True when the file was recognised as an AO3 download and read by the AO3 rules.
    pub fn is_ao3(&self) -> bool {
        self.ao3
    }

    pub fn flavour(&self) -> HtmlFlavour {
        self.flavour
    }

    pub fn set_flavour(&mut self, flavour: HtmlFlavour) {
        self.flavour = flavour;
    }

    /// The document as HTML in the current flavour.
    pub fn to_bytes(&self) -> Vec<u8> {
        write::write(&self.document, self.flavour).into_bytes()
    }

    /// Writes the document to `path`.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), HtmlError> {
        std::fs::write(path, self.to_bytes())?;
        Ok(())
    }
}

/// Reads a chapter's HTML as pasted (an AO3 chapter, or any fragment) into a document.
/// No images are loaded except embedded ones, and the AO3 download rules do not apply.
pub fn parse_fragment(html: &str) -> Document {
    let dom = dom::Dom::parse(html);
    read::read(&dom, None, false).document
}

/// Loads a local image for the reader. Remote sources never reach here.
fn load_local(base_dir: Option<&Path>, src: &str) -> Result<Vec<u8>, String> {
    let base_dir = base_dir.ok_or_else(|| "there is no folder to look in".to_owned())?;
    let src = src.split(['#', '?']).next().unwrap_or("");
    let src = src.strip_prefix("file://").unwrap_or(src);
    let decoded = util::percent_decode(src);
    for candidate in [src, decoded.as_str()] {
        let path = Path::new(candidate);
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        };
        if full.is_file() {
            let size = std::fs::metadata(&full)
                .map_err(|e| format!("the file could not be read ({e})"))?
                .len();
            if size > read::MAX_IMAGE_BYTES as u64 {
                return Err("the file is larger than 32 MiB".to_owned());
            }
            return std::fs::read(&full).map_err(|e| format!("the file could not be read ({e})"));
        }
    }
    Err("the file was not found".to_owned())
}
