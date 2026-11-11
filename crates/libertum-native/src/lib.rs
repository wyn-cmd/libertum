//! The native Libertum document format (`.lbtd`), as specified in
//! `docs/format-libertum-native.md`.
//!
//! A ZIP container:
//!
//! ```text
//! mimetype                 stored, first entry
//! manifest.json            format, version, generator, feature list, part list
//! document.json            the model, one-to-one
//! view.json                pageless and zoom (the interchange formats have nowhere for it)
//! media/<hash>.<ext>       one entry per distinct image, named by the model's media hash
//! extensions/<name>        opaque entries a newer build wrote, kept byte for byte
//! ```
//!
//! Saving is deterministic: a fixed entry order, a zeroed timestamp on every entry and maps
//! written in key order, so the same document always saves to the same bytes and a fixture
//! can be compared by hash. Image bytes stay out of `document.json`: the model stores an
//! image's hash there and the bytes live in `media/`, so an edit never rewrites image data.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::sync::Arc;

use libertum_model::{Document, Media};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

/// What the `mimetype` entry says.
pub const MIMETYPE: &str = "application/vnd.libertum.document";
/// The manifest's `format` field.
pub const FORMAT: &str = "libertum-document";
/// The container version this build writes and the newest it reads.
pub const VERSION: u32 = 1;
/// File extension, without the dot.
pub const EXTENSION: &str = "lbtd";
/// What the writer lists in the manifest: the features this build actually carries.
pub const FEATURES: [&str; 7] = [
    "pageless",
    "images",
    "notes",
    "bookmarks",
    "cross-references",
    "table-styles",
    "view-state",
];

#[derive(Debug, thiserror::Error)]
pub enum NativeError {
    #[error("this is not a Libertum document")]
    NotNative,
    #[error(
        "this document was written by a newer Libertum (format {found}, this build reads {VERSION})"
    )]
    TooNew { found: u32 },
    #[error("the document is damaged: {0}")]
    Damaged(String),
    #[error("the image {hash:016x} is missing from the document")]
    MissingMedia { hash: u64 },
    #[error("could not read or write the document: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not read the package: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("could not read the document's content: {0}")]
    Json(#[from] serde_json::Error),
}

/// The view state a document remembers. Optional in the container: a build that does not
/// understand a key keeps the rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewState {
    /// The document was last read as one endless column.
    pub pageless: bool,
    /// Last zoom, as a percentage (100 is actual size).
    pub zoom_percent: u16,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            pageless: false,
            zoom_percent: 100,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    generator: String,
    #[serde(default)]
    features: Vec<String>,
    #[serde(default)]
    parts: Vec<String>,
}

/// An open native document: the model, its view state, and anything a newer build added.
#[derive(Debug, Clone)]
pub struct Native {
    document: Document,
    view: ViewState,
    /// Entries under `extensions/`, kept byte for byte so a save never drops them.
    extensions: BTreeMap<String, Vec<u8>>,
}

impl Native {
    /// A document with the default view state.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            view: ViewState::default(),
            extensions: BTreeMap::new(),
        }
    }

    pub fn with_view(document: Document, view: ViewState) -> Self {
        Self {
            document,
            view,
            extensions: BTreeMap::new(),
        }
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    pub fn view(&self) -> &ViewState {
        &self.view
    }

    pub fn set_view(&mut self, view: ViewState) {
        self.view = view;
    }

    /// Keeps an opaque entry a newer build wrote (also the hook a test uses).
    pub fn insert_extension(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        self.extensions.insert(name.into(), bytes);
    }

    pub fn extensions(&self) -> impl Iterator<Item = (&str, &[u8])> {
        self.extensions
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, NativeError> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), NativeError> {
        std::fs::write(path, self.to_bytes()?)?;
        Ok(())
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, NativeError> {
        // One entry per distinct image, in hash order.
        let mut media: BTreeMap<u64, Arc<Media>> = BTreeMap::new();
        for image in self
            .document
            .paragraphs()
            .flat_map(|paragraph| paragraph.images())
        {
            media
                .entry(image.media.hash())
                .or_insert_with(|| image.media.clone());
        }
        let mut parts = vec![
            "document.json".to_owned(),
            "view.json".to_owned(),
            "manifest.json".to_owned(),
        ];
        parts.extend(
            media
                .values()
                .map(|media| format!("media/{:016x}.{}", media.hash(), media.format().extension())),
        );
        parts.extend(
            self.extensions
                .keys()
                .map(|name| format!("extensions/{name}")),
        );
        let manifest = Manifest {
            format: FORMAT.to_owned(),
            version: VERSION,
            generator: format!("libertum {}", env!("CARGO_PKG_VERSION")),
            features: FEATURES
                .iter()
                .map(|feature| (*feature).to_owned())
                .collect(),
            parts,
        };

        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            // A zeroed timestamp and fixed order make the archive byte-identical each save.
            let stored = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(DateTime::default());
            let deflated = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(DateTime::default());
            zip.start_file("mimetype", stored)?;
            zip.write_all(MIMETYPE.as_bytes())?;
            zip.start_file("manifest.json", deflated)?;
            zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
            zip.start_file("document.json", deflated)?;
            zip.write_all(&serde_json::to_vec(&self.document)?)?;
            zip.start_file("view.json", deflated)?;
            zip.write_all(&serde_json::to_vec(&self.view)?)?;
            for media in media.values() {
                zip.start_file(
                    format!("media/{:016x}.{}", media.hash(), media.format().extension()),
                    deflated,
                )?;
                zip.write_all(media.bytes())?;
            }
            for (name, bytes) in &self.extensions {
                zip.start_file(format!("extensions/{name}"), deflated)?;
                zip.write_all(bytes)?;
            }
            zip.finish()?;
        }
        Ok(buffer.into_inner())
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, NativeError> {
        let mut archive = ZipArchive::new(Cursor::new(bytes))?;
        if read_entry(&mut archive, "mimetype")?.as_slice() != MIMETYPE.as_bytes() {
            return Err(NativeError::NotNative);
        }
        let manifest: Manifest =
            serde_json::from_slice(&read_entry(&mut archive, "manifest.json")?)?;
        if manifest.format != FORMAT {
            return Err(NativeError::NotNative);
        }
        if manifest.version > VERSION {
            return Err(NativeError::TooNew {
                found: manifest.version,
            });
        }
        let mut document: Document =
            serde_json::from_slice(&read_entry(&mut archive, "document.json")?)?;
        let view = match read_entry(&mut archive, "view.json") {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            // Optional: the document is still perfectly readable without it.
            Err(NativeError::Zip(zip::result::ZipError::FileNotFound)) => ViewState::default(),
            Err(error) => return Err(error),
        };

        let mut media: BTreeMap<u64, Arc<Media>> = BTreeMap::new();
        let mut extensions: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index)?;
            let name = file.name()?.to_string();
            if let Some(rest) = name.strip_prefix("media/") {
                let hash = rest
                    .split('.')
                    .next()
                    .and_then(|hex| u64::from_str_radix(hex, 16).ok())
                    .ok_or_else(|| NativeError::Damaged(format!("bad media name {name}")))?;
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                let image = Media::new(bytes)
                    .map_err(|error| NativeError::Damaged(format!("image {hash:016x}: {error}")))?;
                if image.hash() != hash {
                    return Err(NativeError::Damaged(format!(
                        "image {hash:016x} does not match its name"
                    )));
                }
                media.insert(hash, Arc::new(image));
            } else if let Some(rest) = name.strip_prefix("extensions/") {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                extensions.insert(rest.to_owned(), bytes);
            }
        }

        // The model holds image references by hash; the bytes are here.
        document
            .resolve_media(|placeholder| media.get(&placeholder.hash()).cloned())
            .map_err(|error| match error {
                libertum_model::ModelError::MissingMedia { hash } => {
                    NativeError::MissingMedia { hash }
                }
                other => NativeError::Damaged(other.to_string()),
            })?;

        Ok(Self {
            document,
            view,
            extensions,
        })
    }
}

fn read_entry(archive: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>, NativeError> {
    let mut file = archive.by_name(name)?;
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
