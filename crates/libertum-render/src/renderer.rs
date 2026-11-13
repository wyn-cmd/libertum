//! Page rasterisation.

use std::sync::Arc;

use libertum_layout::parley::PositionedLayoutItem;
use libertum_layout::{Decoration, INK_OVERHANG, LEADER_PITCH_PT, LayoutState, ParagraphLayout};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

use crate::glyphs::{GlyphCache, GlyphRequest};
use crate::images::ImageCache;
use crate::page_cache::{DEFAULT_PAGE_CACHE_BYTES, PageCache, PageKey};

/// Counters for benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub glyph_cache_entries: usize,
    pub glyph_cache_bytes: usize,
    pub glyph_hits: u64,
    pub glyph_misses: u64,
    pub page_cache_pages: usize,
    pub page_cache_bytes: usize,
    /// Bytes of decoded and scaled inline images held.
    pub image_cache_bytes: usize,
    /// Image decodes since creation.
    pub image_decodes: u64,
}

/// Stateful page renderer: owns the swash scale context, the glyph bitmap cache and the page
/// bitmap cache. Keep one per document view.
pub struct PageRenderer {
    glyphs: GlyphCache,
    pages: PageCache,
    images: ImageCache,
    /// The watermark text rasterised last, by a hash of what it shows and at what size.
    watermark: Option<(u64, Arc<Pixmap>, f32)>,
    /// Smooth (antialiased) text, dots and turned images; false gives hard pixel edges.
    antialias: bool,
}

impl Default for PageRenderer {
    fn default() -> Self {
        Self::new(DEFAULT_PAGE_CACHE_BYTES)
    }
}

impl PageRenderer {
    /// `page_cache_bytes` caps the page bitmap cache (default 64 MB).
    pub fn new(page_cache_bytes: usize) -> Self {
        Self {
            glyphs: GlyphCache::new(32 * 1024 * 1024),
            pages: PageCache::new(page_cache_bytes),
            images: ImageCache::new(),
            watermark: None,
            antialias: true,
        }
    }

    /// Turns antialiasing of text, leader dots and turned images on or off (on by default).
    /// The setting is part of the page cache key, so cached pages are never served stale.
    pub fn set_antialias(&mut self, on: bool) {
        self.antialias = on;
    }

    pub fn antialias(&self) -> bool {
        self.antialias
    }

    pub fn stats(&self) -> RenderStats {
        RenderStats {
            glyph_cache_entries: self.glyphs.len(),
            glyph_cache_bytes: self.glyphs.bytes(),
            glyph_hits: self.glyphs.hits,
            glyph_misses: self.glyphs.misses,
            page_cache_pages: self.pages.len(),
            page_cache_bytes: self.pages.bytes(),
            image_cache_bytes: self.images.bytes(),
            image_decodes: self.images.decodes,
        }
    }

    pub fn page_cache(&mut self) -> &mut PageCache {
        &mut self.pages
    }

    /// Returns the page bitmap, from the page cache when the page content, index and scale
    /// match, otherwise rendering and caching it.
    pub fn cached_page(
        &mut self,
        state: &LayoutState,
        page_index: usize,
        scale: f32,
    ) -> Option<Arc<Pixmap>> {
        let key = PageKey {
            page: page_index,
            content_hash: state.page_content_hash(page_index)?
                ^ if self.antialias { 0 } else { HARD_EDGE_SALT },
            scale_bits: scale.to_bits(),
        };
        if let Some(p) = self.pages.get(&key) {
            return Some(p);
        }
        let pm = Arc::new(self.render_page(state, page_index, scale)?);
        self.pages.insert(key, pm.clone());
        Some(pm)
    }

    /// Rasterises one page at `scale` (device pixels per point: 1.0 = 72 dpi, 2.0 = HiDPI or
    /// 200% zoom). Returns `None` if `page_index` is out of range.
    pub fn render_page(
        &mut self,
        state: &LayoutState,
        page_index: usize,
        scale: f32,
    ) -> Option<Pixmap> {
        let (_, h) = page_pixel_size(state, page_index, scale);
        self.render_band(state, page_index, scale, 0, h)
    }

    /// Rasterises rows `y0..y0 + rows` of a page's bitmap at `scale`, full width. The result
    /// matches the same rows of [`render_page`](Self::render_page); lines entirely outside the
    /// band are skipped. Returns `None` if the page does not exist or the band is empty.
    pub fn render_band(
        &mut self,
        state: &LayoutState,
        page_index: usize,
        scale: f32,
        y0: u32,
        rows: u32,
    ) -> Option<Pixmap> {
        let page = state.pages().get(page_index)?;
        let geo = state.page_geometry(page_index);
        let (w, h) = page_pixel_size(state, page_index, scale);
        let rows = rows.min(h.saturating_sub(y0));
        let mut pm = Pixmap::new(w, rows)?;
        pm.fill(Color::WHITE);
        // The band in page points, and the shift from page points to band pixels.
        let shift = y0 as f32 / scale;
        let band_end = (y0 + rows) as f32 / scale;

        // The watermark lies under everything else.
        if let Some(watermark) = state.page_watermark(page_index) {
            self.draw_watermark(&mut pm, &watermark, shift, scale);
        }

        // Table cell fills and borders sit under the text.
        for table in &page.tables {
            let top = geo.margins.top + table.y - shift;
            if top + table.height < 0.0 || top > band_end {
                continue;
            }
            for cell in &table.cells {
                if let Some(fill) = cell.fill {
                    fill_rect(
                        &mut pm,
                        geo.margins.left + cell.x,
                        top,
                        cell.width,
                        table.height,
                        fill,
                        scale,
                    );
                }
            }
            if table.borders.is_visible() {
                let colour = table.borders.colour;
                let line = table.borders.width_pt.max(0.5);
                let x = geo.margins.left + table.x;
                // The top line, cell by cell: a cell that continues the one above has none.
                for cell in table.cells.iter().filter(|cell| cell.top) {
                    fill_rect(
                        &mut pm,
                        geo.margins.left + cell.x,
                        top,
                        cell.width,
                        line,
                        colour,
                        scale,
                    );
                }
                if table.bottom {
                    fill_rect(
                        &mut pm,
                        x,
                        top + table.height - line,
                        table.width,
                        line,
                        colour,
                        scale,
                    );
                }
                if table.sides {
                    fill_rect(&mut pm, x, top, line, table.height, colour, scale);
                    fill_rect(
                        &mut pm,
                        x + table.width - line,
                        top,
                        line,
                        table.height,
                        colour,
                        scale,
                    );
                }
                for cell in table.cells.iter().filter(|cell| cell.left) {
                    fill_rect(
                        &mut pm,
                        geo.margins.left + cell.x,
                        top,
                        line,
                        table.height,
                        colour,
                        scale,
                    );
                }
            }
        }
        for slice in &page.slices {
            let Some(pl) = state.paragraph_layout(slice.para) else {
                continue;
            };
            let Some(first) = pl.lines.get(slice.line_start) else {
                continue; // synthetic line of an empty paragraph has no glyphs anyway
            };
            let dy = geo.margins.top + slice.y - first.top;
            let dx = geo.margins.left + slice.x + pl.x_offset;
            for li in slice.line_start..slice.line_end {
                // Only lines whose padded box reaches into the band.
                let inside = pl.lines.get(li).is_some_and(|l| {
                    let pad = l.height * INK_OVERHANG;
                    dy + l.top + l.height + pad >= shift && dy + l.top - pad <= band_end
                });
                if inside {
                    self.draw_lines(&mut pm, &pl, li..li + 1, dx, dy - shift, scale);
                }
            }
        }
        for decoration in state.page_decorations(page_index) {
            match decoration {
                Decoration::Text { layout, x, y, .. } => {
                    let lines = 0..layout.lines.len();
                    self.draw_lines(&mut pm, &layout, lines, x, y - shift, scale);
                }
                Decoration::Leader { x0, x1, baseline } => {
                    draw_leader(&mut pm, x0, x1, baseline - shift, scale, self.antialias);
                }
                Decoration::Rect {
                    x,
                    y,
                    width,
                    height,
                    colour,
                } => {
                    fill_rect(&mut pm, x, y - shift, width, height, colour, scale);
                }
                Decoration::Image {
                    media,
                    x,
                    y,
                    width,
                    height,
                } => {
                    // Skip images wholly outside the band; they are never decoded for it.
                    if y + height < shift || y > band_end {
                        continue;
                    }
                    let left = (x * scale).round();
                    let top = ((y - shift) * scale).round();
                    let w = (width * scale).round().max(1.0) as u32;
                    let h = (height * scale).round().max(1.0) as u32;
                    if !self
                        .images
                        .draw(&mut pm, &media, left as i32, top as i32, w, h)
                    {
                        // A damaged image: a grey box where it would be.
                        fill_rect(
                            &mut pm,
                            x,
                            y - shift,
                            width,
                            height,
                            IMAGE_PLACEHOLDER,
                            scale,
                        );
                    }
                }
            }
        }
        Some(pm)
    }

    /// Draws a watermark into a band whose top is `shift` page points down the page.
    ///
    /// The text is rasterised once, upright, into a translucent bitmap (kept until the text,
    /// its size or the scale changes) and then drawn turned about the page's centre, so
    /// every strip of every page reuses the same bitmap.
    fn draw_watermark(
        &mut self,
        pm: &mut Pixmap,
        watermark: &libertum_layout::WatermarkDraw,
        shift: f32,
        scale: f32,
    ) {
        use std::hash::{Hash, Hasher};
        use tiny_skia::{FilterQuality, PixmapPaint};
        // Rasterise at no more than 2 px per point: beyond that the turn-and-scale below
        // enlarges it, which keeps the bitmap a few megabytes at any zoom.
        let raster_scale = scale.clamp(0.05, WATERMARK_MAX_RASTER_SCALE);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        watermark.text.hash(&mut hasher);
        watermark.layout.height.to_bits().hash(&mut hasher);
        watermark.colour.hash(&mut hasher);
        raster_scale.to_bits().hash(&mut hasher);
        self.antialias.hash(&mut hasher);
        let key = hasher.finish();
        let cached = self
            .watermark
            .as_ref()
            .filter(|(k, _, _)| *k == key)
            .map(|(_, bitmap, s)| (bitmap.clone(), *s));
        let (bitmap, bitmap_scale) = match cached {
            Some(found) => found,
            None => {
                let layout = &watermark.layout;
                // Room around the text for ink that reaches past the line box.
                let pad = layout.height * INK_OVERHANG;
                let w = ((layout.layout.width() + 2.0 * pad) * raster_scale)
                    .ceil()
                    .max(1.0) as u32;
                let h = ((layout.height + 2.0 * pad) * raster_scale).ceil().max(1.0) as u32;
                let Some(mut text) = Pixmap::new(w, h) else {
                    return;
                };
                // Black text on white, then turned into coverage: the text renderer blends
                // onto an opaque page, so the alpha is recovered from how dark each pixel is.
                text.fill(Color::WHITE);
                let lines = 0..layout.lines.len();
                self.draw_lines(&mut text, layout, lines, pad, pad, raster_scale);
                let [r, g, b, a] = watermark.colour.map(u32::from);
                for px in text.data_mut().as_chunks_mut::<4>().0 {
                    let coverage = 255 - u32::from(px[0]);
                    let alpha = (coverage * a + 127) / 255;
                    px[0] = ((r * alpha + 127) / 255) as u8;
                    px[1] = ((g * alpha + 127) / 255) as u8;
                    px[2] = ((b * alpha + 127) / 255) as u8;
                    px[3] = alpha as u8;
                }
                let bitmap = Arc::new(text);
                self.watermark = Some((key, bitmap.clone(), raster_scale));
                (bitmap, raster_scale)
            }
        };
        // Bitmap pixels to band pixels: centre on the origin, enlarge to the page scale,
        // turn, then move to the page's centre in this band.
        let enlarge = scale / bitmap_scale;
        let transform = Transform::from_translate(
            -(bitmap.width() as f32) * 0.5,
            -(bitmap.height() as f32) * 0.5,
        )
        .post_scale(enlarge, enlarge)
        .post_rotate(watermark.angle_deg)
        .post_translate(
            watermark.centre_x * scale,
            (watermark.centre_y - shift) * scale,
        );
        let paint = PixmapPaint {
            quality: if self.antialias {
                FilterQuality::Bilinear
            } else {
                FilterQuality::Nearest
            },
            ..PixmapPaint::default()
        };
        pm.draw_pixmap(0, 0, bitmap.as_ref().as_ref(), &paint, transform, None);
    }

    /// Draws lines `lines` of `pl` with the layout's origin at (`dx`, `dy`) page points.
    fn draw_lines(
        &mut self,
        pm: &mut Pixmap,
        pl: &ParagraphLayout,
        lines: std::ops::Range<usize>,
        dx: f32,
        dy: f32,
        scale: f32,
    ) {
        for li in lines {
            let Some(line) = pl.layout.get(li) else {
                continue;
            };
            // Highlights first, under the glyphs: the run's advance across the line box.
            if let Some(lb) = pl.lines.get(li) {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item
                        && let Some(hl) = run.style().brush.highlight
                    {
                        fill_rect(
                            pm,
                            dx + run.offset(),
                            dy + lb.top,
                            run.advance(),
                            lb.height,
                            hl,
                            scale,
                        );
                    }
                }
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue; // inline boxes (tabs, images): images are page decorations
                };
                let brush = run.style().brush;
                let color = brush.color;
                // Superscript and subscript glyphs sit above or below the line's baseline.
                let rise = brush.rise_pt();
                let r = run.run();
                let font = r.font();
                let size_px = r.font_size() * scale;
                let synth = r.synthesis();
                // parley 0.12 wraps coordinates in `NormalizedCoord(i16)`; swash wants i16.
                let coords: Vec<i16> = r.normalized_coords().iter().map(|c| c.to_bits()).collect();
                for g in run.positioned_glyphs() {
                    let x = (dx + g.x) * scale;
                    // Hard edges also snap each glyph to a whole pixel.
                    let x = if self.antialias { x } else { x.round() };
                    let y = ((dy + g.y - rise) * scale).round();
                    let xf = x.floor();
                    let subpx = (((x - xf) * 4.0) as u8).min(3);
                    let req = GlyphRequest {
                        font_data: font.data.data(),
                        font_id: font.data.id(),
                        font_index: font.index,
                        glyph: g.id,
                        size_px,
                        subpx,
                        coords: &coords,
                        embolden: synth.embolden(),
                        skew_degrees: synth.skew(),
                        smooth: self.antialias,
                    };
                    if let Some(bm) = self.glyphs.get(&req) {
                        blit_alpha(
                            pm,
                            xf as i32 + bm.left,
                            y as i32 - bm.top,
                            bm.width,
                            bm.height,
                            &bm.alpha,
                            color,
                        );
                    }
                }
                // Underline and strikethrough over the glyphs, at the font's own offsets.
                let style = run.style();
                let metrics = r.font_metrics();
                let lines = [
                    (
                        &style.underline,
                        metrics.underline_offset,
                        metrics.underline_size,
                    ),
                    (
                        &style.strikethrough,
                        metrics.strikethrough_offset,
                        metrics.strikethrough_size,
                    ),
                ];
                for (decoration, offset, size) in lines {
                    if let Some(d) = decoration {
                        let offset = d.offset.unwrap_or(offset);
                        let size = d.size.unwrap_or(size).max(0.5);
                        let top = dy + run.baseline() - rise - offset - size * 0.5;
                        fill_rect(
                            pm,
                            dx + run.offset(),
                            top,
                            run.advance(),
                            size,
                            d.brush.color,
                            scale,
                        );
                    }
                }
            }
        }
    }
}

/// Fills a rectangle given in points (opaque or translucent colour, straight alpha).
fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: [u8; 4], scale: f32) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = false;
    // At least one device pixel tall, so a thin underline never vanishes.
    let height = (h * scale).max(1.0);
    if let Some(rect) = Rect::from_xywh(x * scale, y * scale, w * scale, height) {
        pm.fill_rect(rect, &paint, Transform::identity(), None);
    }
}

/// Mixed into a page's cache key when antialiasing is off.
const HARD_EDGE_SALT: u64 = 0x9E37_79B9_7F4A_7C15;

/// Most device pixels per point a watermark is rasterised at; larger scales enlarge it.
const WATERMARK_MAX_RASTER_SCALE: f32 = 2.0;

/// Fill of the box drawn in place of an image that does not decode.
pub const IMAGE_PLACEHOLDER: [u8; 4] = [214, 214, 214, 255];

/// Size of a leader dot, points.
const LEADER_DOT_PT: f32 = 0.9;

/// A dot leader: small squares every [`LEADER_PITCH_PT`], right-aligned to `x1` so the dots
/// line up from entry to entry, sitting on the baseline.
fn draw_leader(pm: &mut Pixmap, x0: f32, x1: f32, baseline: f32, scale: f32, smooth: bool) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = smooth;
    let size = (LEADER_DOT_PT * scale).max(1.0);
    let mut x = x1 - LEADER_DOT_PT;
    while x >= x0 {
        if let Some(rect) =
            Rect::from_xywh(x * scale, (baseline - LEADER_DOT_PT) * scale, size, size)
        {
            pm.fill_rect(rect, &paint, Transform::identity(), None);
        }
        x -= LEADER_PITCH_PT;
    }
}

/// Pixel size of a page bitmap at `scale` (what [`PageRenderer::render_page`] returns).
pub fn page_pixel_size(state: &LayoutState, page: usize, scale: f32) -> (u32, u32) {
    let geo = state.page_geometry(page);
    (
        (geo.width_pt * scale).ceil().max(1.0) as u32,
        (geo.height_pt * scale).ceil().max(1.0) as u32,
    )
}

/// One-shot convenience wrapper: renders without reusing any caches. Prefer
/// [`PageRenderer`] in anything performance sensitive.
pub fn render_page(state: &LayoutState, page_index: usize, scale: f32) -> Option<Pixmap> {
    PageRenderer::default().render_page(state, page_index, scale)
}

/// Source-over blend of an 8-bit coverage mask tinted with `color` onto an opaque pixmap.
fn blit_alpha(
    pm: &mut Pixmap,
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    alpha: &[u8],
    color: [u8; 4],
) {
    let pw = pm.width() as i32;
    let ph = pm.height() as i32;
    let x0 = left.max(0);
    let y0 = top.max(0);
    let x1 = (left + width as i32).min(pw);
    let y1 = (top + height as i32).min(ph);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let data = pm.data_mut();
    let ca = u32::from(color[3]);
    for y in y0..y1 {
        let src_row = ((y - top) as usize) * width as usize;
        let dst_row = (y as usize) * pw as usize;
        for x in x0..x1 {
            let a = u32::from(alpha[src_row + (x - left) as usize]);
            if a == 0 {
                continue;
            }
            let a = (a * ca + 127) / 255;
            let i = (dst_row + x as usize) * 4;
            for c in 0..3 {
                let d = u32::from(data[i + c]);
                let s = u32::from(color[c]);
                // The page is opaque, so premultiplied == straight for the destination.
                data[i + c] = ((d * (255 - a) + s * a + 127) / 255) as u8;
            }
        }
    }
}
