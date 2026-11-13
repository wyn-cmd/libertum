//! Decoding and scaling of inline images for the page renderers.
//!
//! An image is decoded the first time a page that shows it is rendered (never on open or
//! while typing) and kept as straight RGBA, downsampled so its longer side is at most
//! [`MAX_DECODED_SIDE`] pixels: a 24-megapixel photograph then costs about 12 MB instead of
//! 96 MB. A second cache holds the image scaled to the exact device size it is drawn at, so
//! shrinking uses a proper filter once instead of point-sampling every strip.
//!
//! Both caches are bounded by bytes and simply cleared when full: decoding again is cheap
//! next to the cost of holding hundreds of megabytes on a 4 GB machine.

use std::collections::HashMap;
use std::sync::Arc;

use libertum_model::{ImageFormat, Media};
use tiny_skia::{FilterQuality, IntSize, Pixmap, PixmapPaint, Transform};

/// Longest side kept after decoding, pixels.
pub const MAX_DECODED_SIDE: u32 = 2048;
/// Bound on decoded images held, bytes.
const MAX_DECODED_BYTES: usize = 96 * 1024 * 1024;
/// Bound on device-size copies held, bytes.
const MAX_SCALED_BYTES: usize = 48 * 1024 * 1024;

/// Straight (non-premultiplied) RGBA pixels.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes `media` to straight RGBA, downsampled to at most [`MAX_DECODED_SIDE`] on its
/// longer side. `None` when the bytes do not decode (a damaged file): the caller draws a
/// placeholder instead.
pub fn decode(media: &Media) -> Option<Decoded> {
    let format = match media.format() {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
    };
    let decoded = image::load_from_memory_with_format(media.bytes(), format).ok()?;
    let longest = decoded.width().max(decoded.height());
    let decoded = if longest > MAX_DECODED_SIDE {
        decoded.thumbnail(MAX_DECODED_SIDE, MAX_DECODED_SIDE)
    } else {
        decoded
    };
    let rgba = decoded.into_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    if width == 0 || height == 0 {
        return None;
    }
    Some(Decoded {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

/// Straight RGBA to the premultiplied pixmap tiny-skia draws.
fn to_pixmap(width: u32, height: u32, mut rgba: Vec<u8>) -> Option<Pixmap> {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(rgba, IntSize::from_wh(width, height)?)
}

/// Decode and device-size caches for the CPU renderer.
#[derive(Default)]
pub struct ImageCache {
    /// By `Media::hash`; `None` records a failed decode so it is not retried every frame.
    decoded: HashMap<u64, Option<Arc<Decoded>>>,
    decoded_bytes: usize,
    scaled: HashMap<(u64, u32, u32), Arc<Pixmap>>,
    scaled_bytes: usize,
    /// Decodes done since creation (tests and benchmarks).
    pub decodes: u64,
}

impl ImageCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bytes held by both caches.
    pub fn bytes(&self) -> usize {
        self.decoded_bytes + self.scaled_bytes
    }

    pub fn clear(&mut self) {
        self.decoded.clear();
        self.scaled.clear();
        self.decoded_bytes = 0;
        self.scaled_bytes = 0;
    }

    fn decoded(&mut self, media: &Media) -> Option<Arc<Decoded>> {
        if let Some(entry) = self.decoded.get(&media.hash()) {
            return entry.clone();
        }
        self.decodes += 1;
        let entry = decode(media).map(Arc::new);
        let bytes = entry.as_ref().map_or(0, |d| d.rgba.len());
        if self.decoded_bytes + bytes > MAX_DECODED_BYTES {
            self.decoded.clear();
            self.decoded_bytes = 0;
        }
        self.decoded_bytes += bytes;
        self.decoded.insert(media.hash(), entry.clone());
        entry
    }

    /// The image as a pixmap `width` x `height` device pixels, when that is no larger than
    /// the decoded image (shrinking, the common case). `None` asks the caller to scale the
    /// decoded pixels up while drawing instead.
    fn scaled(&mut self, media: &Media, width: u32, height: u32) -> Option<Arc<Pixmap>> {
        let key = (media.hash(), width, height);
        if let Some(pm) = self.scaled.get(&key) {
            return Some(pm.clone());
        }
        let decoded = self.decoded(media)?;
        if width > decoded.width || height > decoded.height {
            return None;
        }
        let rgba = if (width, height) == (decoded.width, decoded.height) {
            decoded.rgba.clone()
        } else {
            let source =
                image::RgbaImage::from_raw(decoded.width, decoded.height, decoded.rgba.clone())?;
            image::imageops::resize(
                &source,
                width,
                height,
                image::imageops::FilterType::Triangle,
            )
            .into_raw()
        };
        let pm = Arc::new(to_pixmap(width, height, rgba)?);
        let bytes = pm.data().len();
        if self.scaled_bytes + bytes > MAX_SCALED_BYTES {
            self.scaled.clear();
            self.scaled_bytes = 0;
        }
        self.scaled_bytes += bytes;
        self.scaled.insert(key, pm.clone());
        Some(pm)
    }

    /// Draws `media` into `target` over the device rectangle (`x`, `y`, `width`, `height`),
    /// in pixels. Returns false when the image could not be decoded (the caller draws a
    /// placeholder).
    pub fn draw(
        &mut self,
        target: &mut Pixmap,
        media: &Media,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> bool {
        if width == 0 || height == 0 {
            return true;
        }
        if let Some(pm) = self.scaled(media, width, height) {
            target.draw_pixmap(
                x,
                y,
                pm.as_ref().as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            );
            return true;
        }
        // Enlarging: scale the decoded pixels while drawing.
        let Some(decoded) = self.decoded(media) else {
            return false;
        };
        let Some(source) = to_pixmap(decoded.width, decoded.height, decoded.rgba.clone()) else {
            return false;
        };
        let paint = PixmapPaint {
            quality: FilterQuality::Bilinear,
            ..PixmapPaint::default()
        };
        let transform = Transform::from_row(
            width as f32 / decoded.width as f32,
            0.0,
            0.0,
            height as f32 / decoded.height as f32,
            x as f32,
            y as f32,
        );
        target.draw_pixmap(0, 0, source.as_ref(), &paint, transform, None);
        true
    }
}
