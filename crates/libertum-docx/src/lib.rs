//! DOCX import and export (PLAN §6.2).
//!
//! [`Docx::parse`] reads a package into a [`Document`] and keeps the package it came from, so
//! [`Docx::to_bytes`] writes back every part Libertum does not model. Body content it cannot
//! carry (tables, drawings, fields, tracked changes) makes the import fail rather than
//! silently dropping it on the next save.

mod package;
mod read;
mod write;
mod xml;

use std::path::Path;

use libertum_model::Document;

#[derive(Debug, thiserror::Error)]
pub enum DocxError {
    #[error("not a DOCX package: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("the package has no word/document.xml")]
    NoDocument,
    #[error("invalid XML in {part}: {message}")]
    Xml { part: String, message: String },
    #[error("unexpected content in {part}: {message}")]
    Content { part: String, message: String },
    #[error("this document contains {what}, which Libertum cannot save yet")]
    Unsupported { part: String, what: String },
    #[error("could not read or write the file: {0}")]
    Io(#[from] std::io::Error),
}

/// Section-level XML Libertum does not model (header/footer references, page-number format,
/// grid) kept as element name plus attributes, and written back unchanged.
#[derive(Clone, Debug, Default)]
pub(crate) struct SectionExtras {
    pub elements: Vec<(String, Vec<(String, String)>)>,
}

pub(crate) type Extras = Vec<SectionExtras>;

/// A DOCX document together with the package it was read from.
#[derive(Clone)]
pub struct Docx {
    document: Document,
    extras: Extras,
    origin: Option<package::Package>,
    warnings: Vec<String>,
}

impl Docx {
    /// Reads a package from memory.
    pub fn parse(bytes: &[u8]) -> Result<Self, DocxError> {
        let parsed = read::read(bytes)?;
        Ok(Self {
            document: parsed.document,
            extras: parsed.extras,
            origin: Some(parsed.package),
            warnings: parsed.warnings,
        })
    }

    /// Reads a file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocxError> {
        let bytes = std::fs::read(path)?;
        Self::parse(&bytes)
    }

    /// Wraps a document for export. The first save writes a complete minimal package.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            extras: Vec::new(),
            origin: None,
            warnings: Vec::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Replaces the model document, keeping the origin package (so saved files keep their
    /// unknown parts). Section extras are kept by index.
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

    /// Writes the document as a DOCX package.
    pub fn to_bytes(&self) -> Result<Vec<u8>, DocxError> {
        let package = write::build(&self.document, &self.extras, self.origin.as_ref())?;
        package.write()
    }

    /// Writes the document to a file.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DocxError> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }
}
