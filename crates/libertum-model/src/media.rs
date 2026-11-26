//! Inline images and the image bytes they show (PLAN §4.1, Phase 6).
//!
//! An image is anchored at a byte offset in its paragraph, like a note or a bookmark: it adds
//! no character to the text, so offsets and the plain-text clipboard are unaffected. Layout
//! places it as an inline box at that offset.
//!
//! The bytes are held in an [`Arc<Media>`](Media) on the image itself rather than in a
//! document-wide store: cloning a paragraph (undo, snapshots) then shares the bytes, and an
//! operation's inverse carries everything needed to restore a deleted image exactly.
//!
//! Only PNG and JPEG bytes are stored, because those are the two encodings DOCX, ODT and RTF
//! can all embed. Other formats are converted to PNG before they reach the model (see
//! `libertum_formats_common::media`). No decoding happens here: the pixel size is read from
//! the file header.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Smallest and largest side of a displayed image, points.
pub const MIN_IMAGE_PT: f32 = 4.0;
pub const MAX_IMAGE_PT: f32 = 4_000.0;

/// The encodings the model stores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    /// File extension without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpeg",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ImageFormat::Png => "image/png",
            ImageFormat::Jpeg => "image/jpeg",
        }
    }
}

/// Why image bytes were not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MediaError {
    #[error("the image is not a PNG or JPEG file")]
    UnknownFormat,
    #[error("the image header is damaged or incomplete")]
    BadHeader,
    #[error("the image has no pixels")]
    Empty,
}

/// Encoded image bytes with the facts read from their header.
#[derive(Clone, Debug)]
pub struct Media {
    bytes: Vec<u8>,
    format: ImageFormat,
    width_px: u32,
    height_px: u32,
    hash: u64,
}

impl Media {
    /// Wraps PNG or JPEG bytes. The format and pixel size come from the header; the pixels
    /// are not decoded.
    pub fn new(bytes: Vec<u8>) -> Result<Self, MediaError> {
        let (format, width_px, height_px) = sniff(&bytes)?;
        if width_px == 0 || height_px == 0 {
            return Err(MediaError::Empty);
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        Ok(Self {
            hash: hasher.finish(),
            bytes,
            format,
            width_px,
            height_px,
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn format(&self) -> ImageFormat {
        self.format
    }

    pub fn width_px(&self) -> u32 {
        self.width_px
    }

    pub fn height_px(&self) -> u32 {
        self.height_px
    }

    /// Hash of the bytes: a cheap identity for decode caches and package part names.
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Natural size in points at 96 pixels per inch (what Word and LibreOffice assume for an
    /// image without a resolution).
    pub fn natural_size_pt(&self) -> (f32, f32) {
        (self.width_px as f32 * 0.75, self.height_px as f32 * 0.75)
    }
}

impl PartialEq for Media {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.bytes == other.bytes
    }
}

impl Media {
    /// An image reference without its bytes. The native format keeps image data in the
    /// container's media entries, so reading a document builds these first and the reader
    /// fills the bytes before the document is used.
    pub fn placeholder(format: ImageFormat, width_px: u32, height_px: u32, hash: u64) -> Self {
        Self {
            bytes: Vec::new(),
            format,
            width_px,
            height_px,
            hash,
        }
    }

    /// Whether this image still needs its bytes (it came from a file being read).
    pub fn is_placeholder(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// The hash, format and pixel size go into the document; the bytes go into the container.
impl serde::Serialize for Media {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Media", 4)?;
        state.serialize_field("hash", &format!("{:016x}", self.hash))?;
        state.serialize_field("format", &self.format)?;
        state.serialize_field("width_px", &self.width_px)?;
        state.serialize_field("height_px", &self.height_px)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Media {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Parts {
            hash: String,
            format: ImageFormat,
            width_px: u32,
            height_px: u32,
        }
        let parts = Parts::deserialize(deserializer)?;
        let hash = u64::from_str_radix(parts.hash.trim_start_matches("0x"), 16)
            .map_err(serde::de::Error::custom)?;
        Ok(Media::placeholder(
            parts.format,
            parts.width_px,
            parts.height_px,
            hash,
        ))
    }
}

/// An image anchored in a paragraph.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InlineImage {
    /// Byte offset in the paragraph's text where the image sits.
    pub offset: usize,
    pub media: Arc<Media>,
    /// Displayed size, points.
    pub width_pt: f32,
    pub height_pt: f32,
    /// Alternative text (DOCX `wp:docPr/@descr`, ODT `svg:desc`, Markdown alt text).
    pub alt: String,
    /// File name or relative path the image came from (Markdown link target, package part
    /// name hint). May be empty.
    pub name: String,
}

/// What a linked picture or video is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Image,
    Video,
    /// A music player: shown in the document as a player bar, written to HTML as an
    /// `<audio>` element.
    Audio,
}

/// An online picture or video an inline image stands for: the address AO3's HTML will point
/// at. Libertum never fetches it by itself; the image shown is a preview the writer chose to
/// download (kept in the document) or a placeholder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaLink {
    pub url: String,
    pub kind: MediaKind,
}

/// A video link is stored in the image's `name` as this prefix and the address.
const VIDEO_PREFIX: &str = "video:";
/// An audio link is stored the same way, after `audio:`.
const AUDIO_PREFIX: &str = "audio:";

impl MediaLink {
    /// The text kept in [`InlineImage::name`]: the address itself for a picture, or the
    /// address after `video:`.
    pub fn name(&self) -> String {
        match self.kind {
            MediaKind::Image => self.url.clone(),
            MediaKind::Video => format!("{VIDEO_PREFIX}{}", self.url),
            MediaKind::Audio => format!("{AUDIO_PREFIX}{}", self.url),
        }
    }

    /// Reads a link back out of an image name (`None` for an ordinary file name).
    pub fn from_name(name: &str) -> Option<Self> {
        let (kind, url) = if let Some(rest) = name.strip_prefix(VIDEO_PREFIX) {
            (MediaKind::Video, rest)
        } else if let Some(rest) = name.strip_prefix(AUDIO_PREFIX) {
            (MediaKind::Audio, rest)
        } else {
            (MediaKind::Image, name)
        };
        let lower = url.to_ascii_lowercase();
        (lower.starts_with("https://") || lower.starts_with("http://")).then(|| Self {
            url: url.to_owned(),
            kind,
        })
    }
}

impl InlineImage {
    /// The online picture or video this image stands for, if it is a link. The link lives in
    /// `name`, which every format already keeps, so no file format needed a new field.
    pub fn link(&self) -> Option<MediaLink> {
        MediaLink::from_name(&self.name)
    }

    /// An image at its natural size, scaled down to fit `max_width_pt` if it is wider.
    pub fn fitted(offset: usize, media: Arc<Media>, max_width_pt: f32) -> Self {
        let (mut width, mut height) = media.natural_size_pt();
        if max_width_pt > 0.0 && width > max_width_pt {
            height *= max_width_pt / width;
            width = max_width_pt;
        }
        let mut image = Self {
            offset,
            media,
            width_pt: width,
            height_pt: height,
            alt: String::new(),
            name: String::new(),
        };
        image.clamp_size();
        image
    }

    /// Keeps the displayed size finite and within [`MIN_IMAGE_PT`]..=[`MAX_IMAGE_PT`].
    pub fn clamp_size(&mut self) {
        let fix = |v: f32| {
            if v.is_finite() {
                v.clamp(MIN_IMAGE_PT, MAX_IMAGE_PT)
            } else {
                MIN_IMAGE_PT
            }
        };
        self.width_pt = fix(self.width_pt);
        self.height_pt = fix(self.height_pt);
    }

    /// Height over width of the pixels (for resizing in proportion).
    pub fn aspect(&self) -> f32 {
        self.media.height_px() as f32 / self.media.width_px().max(1) as f32
    }
}

/// What a cross-reference shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CrossRefKind {
    /// The text of the paragraph that holds the bookmark (DOCX `REF`, ODT
    /// `text:reference-format="text"`).
    Text,
    /// The number of the page the bookmark is on (DOCX `PAGEREF`, ODT
    /// `text:reference-format="page"`).
    Page,
}

/// A cross-reference to a bookmark: a stretch of the paragraph's own text that shows the
/// reference's last computed result (the formats call this the field's cached result).
///
/// The text is ordinary text for layout, selection and export. Refreshing a reference
/// replaces the text in `range`; nothing else distinguishes it from typed text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct CrossRef {
    /// Byte range of the shown text. Never empty: a reference whose text is deleted goes
    /// with it.
    pub range: std::ops::Range<usize>,
    /// Name of the bookmark referred to (matched case-insensitively, as Word does).
    pub target: String,
    pub kind: CrossRefKind,
}

/// Format and pixel size from a PNG or JPEG header.
fn sniff(bytes: &[u8]) -> Result<(ImageFormat, u32, u32), MediaError> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG) {
        // Signature (8), IHDR length (4), "IHDR" (4), width (4), height (4).
        let field = |at: usize| -> Result<u32, MediaError> {
            let b = bytes.get(at..at + 4).ok_or(MediaError::BadHeader)?;
            Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        };
        if bytes.get(12..16) != Some(b"IHDR") {
            return Err(MediaError::BadHeader);
        }
        return Ok((ImageFormat::Png, field(16)?, field(20)?));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        // Walk the segments to the first start-of-frame marker.
        let mut at = 2usize;
        loop {
            // Skip fill bytes before a marker.
            while bytes.get(at) == Some(&0xFF) && bytes.get(at + 1) == Some(&0xFF) {
                at += 1;
            }
            if bytes.get(at) != Some(&0xFF) {
                return Err(MediaError::BadHeader);
            }
            let marker = *bytes.get(at + 1).ok_or(MediaError::BadHeader)?;
            at += 2;
            // Markers without a length.
            if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
                continue;
            }
            if marker == 0xD9 || marker == 0xDA {
                return Err(MediaError::BadHeader);
            }
            let len = bytes.get(at..at + 2).ok_or(MediaError::BadHeader)?;
            let len = usize::from(u16::from_be_bytes([len[0], len[1]]));
            if len < 2 {
                return Err(MediaError::BadHeader);
            }
            let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
            if is_sof {
                // Length (2), precision (1), height (2), width (2).
                let b = bytes.get(at + 3..at + 7).ok_or(MediaError::BadHeader)?;
                let height = u32::from(u16::from_be_bytes([b[0], b[1]]));
                let width = u32::from(u16::from_be_bytes([b[2], b[3]]));
                return Ok((ImageFormat::Jpeg, width, height));
            }
            at += len;
        }
    }
    Err(MediaError::UnknownFormat)
}

/// A tiny valid PNG (`width` x `height`, one grey level) for tests in this and other crates.
/// The image data chunk is not real pixel data, so it can be sniffed but not decoded; use
/// an encoder where the pixels matter.
pub fn test_png_header(width: u32, height: u32) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend_from_slice(&13u32.to_be_bytes());
    out.extend_from_slice(b"IHDR");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&[8, 0, 0, 0, 0]);
    out.extend_from_slice(&[0, 0, 0, 0]); // CRC (not checked by the sniffer)
    out
}
