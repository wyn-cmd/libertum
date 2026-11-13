//! Glyph bitmap cache on top of swash.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use swash::FontRef;
use swash::scale::{Render, ScaleContext, Scaler, Source, image::Content};
use swash::zeno::{Angle, Format, Transform, Vector};

/// Alpha-mask bitmap of one glyph at one size and sub-pixel offset.
pub(crate) struct GlyphBitmap {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub alpha: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct GlyphKey {
    font_id: u64,
    font_index: u32,
    glyph: u32,
    size_bits: u32,
    /// Horizontal sub-pixel position in quarter pixels (0..=3).
    subpx: u8,
    coords_hash: u64,
    embolden: bool,
    skew: i8,
    smooth: bool,
}

pub(crate) struct GlyphRequest<'a> {
    pub font_data: &'a [u8],
    pub font_id: u64,
    pub font_index: u32,
    pub glyph: u32,
    pub size_px: f32,
    pub subpx: u8,
    pub coords: &'a [i16],
    pub embolden: bool,
    pub skew_degrees: Option<f32>,
    /// Antialiased edges; false thresholds coverage to hard pixels.
    pub smooth: bool,
}

struct Entry {
    bitmap: Option<Arc<GlyphBitmap>>,
    tick: u64,
    bytes: usize,
}

pub(crate) struct GlyphCache {
    scale_cx: ScaleContext,
    map: HashMap<GlyphKey, Entry>,
    bytes: usize,
    cap_bytes: usize,
    tick: u64,
    pub hits: u64,
    pub misses: u64,
}

impl GlyphCache {
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            scale_cx: ScaleContext::new(),
            map: HashMap::new(),
            bytes: 0,
            cap_bytes,
            tick: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Returns the cached bitmap (or renders it). `None` for glyphs without an outline
    /// (spaces, missing glyphs).
    pub fn get(&mut self, req: &GlyphRequest<'_>) -> Option<Arc<GlyphBitmap>> {
        self.tick += 1;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        req.coords.hash(&mut h);
        let key = GlyphKey {
            font_id: req.font_id,
            font_index: req.font_index,
            glyph: req.glyph,
            size_bits: req.size_px.to_bits(),
            subpx: req.subpx,
            coords_hash: h.finish(),
            embolden: req.embolden,
            skew: req.skew_degrees.map_or(0, |s| s as i8),
            smooth: req.smooth,
        };
        if let Some(e) = self.map.get_mut(&key) {
            e.tick = self.tick;
            self.hits += 1;
            return e.bitmap.clone();
        }
        self.misses += 1;
        let bitmap = self.rasterise(req).map(Arc::new);
        let bytes = bitmap.as_ref().map_or(0, |b| b.alpha.len()) + 64;
        self.bytes += bytes;
        self.map.insert(
            key,
            Entry {
                bitmap: bitmap.clone(),
                tick: self.tick,
                bytes,
            },
        );
        if self.bytes > self.cap_bytes {
            self.evict();
        }
        bitmap
    }

    fn rasterise(&mut self, req: &GlyphRequest<'_>) -> Option<GlyphBitmap> {
        let font = FontRef::from_index(req.font_data, req.font_index as usize)?;
        let mut scaler: Scaler<'_> = self
            .scale_cx
            .builder(font)
            .size(req.size_px)
            .hint(false)
            .normalized_coords(req.coords)
            .build();
        let sources = [Source::Outline];
        let mut render = Render::new(&sources);
        render
            .format(Format::Alpha)
            .offset(Vector::new(f32::from(req.subpx) / 4.0, 0.0));
        if req.embolden {
            render.embolden(req.size_px / 40.0);
        }
        if let Some(deg) = req.skew_degrees {
            render.transform(Some(Transform::skew(
                Angle::from_degrees(deg),
                Angle::from_degrees(0.0),
            )));
        }
        let img = render.render(&mut scaler, req.glyph as u16)?;
        if img.placement.width == 0 || img.placement.height == 0 {
            return None;
        }
        if img.content != Content::Mask {
            return None;
        }
        let mut alpha = img.data;
        if !req.smooth {
            for a in &mut alpha {
                *a = if *a >= 128 { 255 } else { 0 };
            }
        }
        Some(GlyphBitmap {
            left: img.placement.left,
            top: img.placement.top,
            width: img.placement.width,
            height: img.placement.height,
            alpha,
        })
    }

    /// Drops the least recently used half of the entries.
    fn evict(&mut self) {
        let mut ticks: Vec<u64> = self.map.values().map(|e| e.tick).collect();
        if ticks.is_empty() {
            return;
        }
        let mid = ticks.len() / 2;
        let (_, nth, _) = ticks.select_nth_unstable(mid);
        let cutoff = *nth;
        let mut freed = 0usize;
        self.map.retain(|_, e| {
            let keep = e.tick > cutoff;
            if !keep {
                freed += e.bytes;
            }
            keep
        });
        self.bytes = self.bytes.saturating_sub(freed);
    }
}
