//! CPU page rasterisation for Libertum: parley glyph runs -> swash glyph bitmaps -> tiny-skia
//! pixmaps, with a glyph bitmap cache and a byte-capped page bitmap cache (PLAN §4.4).

mod backend;
mod glyphs;
pub mod images;
mod page_cache;
mod renderer;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use backend::{PageBackend, PageOutput};
pub use page_cache::{DEFAULT_PAGE_CACHE_BYTES, PageCache, PageKey};
pub use renderer::{PageRenderer, RenderStats, page_pixel_size, render_page};
pub use tiny_skia::Pixmap;
