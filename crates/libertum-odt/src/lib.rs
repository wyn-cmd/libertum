//! ODT import and export (PLAN §6.3).
//!
//! [`Odt::parse`] reads an OpenDocument Text package into a [`Document`] and keeps the
//! package it came from, so [`Odt::to_bytes`] writes back every part Libertum does not model
//! and rewrites only `content.xml` and `styles.xml`. The `mimetype` entry is always written
//! first and stored uncompressed, as ODF packaging requires.
//!
//! Body content the model cannot carry (footnotes and endnotes, text sections, comments,
//! tracked changes, fields outside a page number, drawings and frames) makes the import fail
//! rather than being silently dropped on the next save.

mod package;
mod read;
mod style;
mod write;
mod xml;

use std::path::Path;

use libertum_model::Document;

/// The ODF media type of a text document, the contents of the `mimetype` entry.
pub const MIME_TEXT: &str = "application/vnd.oasis.opendocument.text";

/// The ODF version Libertum writes. 1.3 files open in LibreOffice, Word and Pages; there is
/// no feature here that needs a newer one.
pub const ODF_VERSION: &str = "1.3";

#[derive(Debug, thiserror::Error)]
pub enum OdtError {
    #[error("not an ODT package: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("this is not an OpenDocument Text file")]
    NotOdt,
    #[error("the package has no content.xml")]
    NoContent,
    #[error("invalid XML in {part}: {message}")]
    Xml { part: String, message: String },
    #[error("unexpected content in {part}: {message}")]
    Content { part: String, message: String },
    #[error("this document contains {what}, which Libertum cannot save yet")]
    Unsupported { part: String, what: String },
    #[error("could not read or write the file: {0}")]
    Io(#[from] std::io::Error),
}

/// An ODT document together with the package it was read from.
#[derive(Clone)]
pub struct Odt {
    document: Document,
    origin: Option<package::Package>,
    warnings: Vec<String>,
}

impl Odt {
    /// Reads a package from memory.
    pub fn parse(bytes: &[u8]) -> Result<Self, OdtError> {
        let parsed = read::read(bytes)?;
        Ok(Self {
            document: parsed.document,
            origin: Some(parsed.package),
            warnings: parsed.warnings,
        })
    }

    /// Reads a file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OdtError> {
        let bytes = std::fs::read(path)?;
        Self::parse(&bytes)
    }

    /// Wraps a document for export. The first save writes a complete minimal package.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            origin: None,
            warnings: Vec::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Replaces the model document, keeping the origin package (so saved files keep their
    /// unknown parts, pictures and meta data).
    pub fn set_document(&mut self, document: Document) {
        self.document = document;
    }

    pub fn into_document(self) -> Document {
        self.document
    }

    /// Non-fatal things noticed while reading (flattened hyperlinks, unknown page sizes).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Writes the document as an ODT package.
    pub fn to_bytes(&self) -> Result<Vec<u8>, OdtError> {
        let package = write::build(&self.document, self.origin.as_ref())?;
        package.write_odf()
    }

    /// Writes the document to a file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), OdtError> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }
}
