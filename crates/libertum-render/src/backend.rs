//! Renderer abstraction (PLAN §4.5): "render page N of a `LayoutState` at scale S".
//!
//! The CPU rasteriser ([`PageRenderer`]) and, with the `gpu` feature, the vello renderer
//! (`VelloPageRenderer`) both implement [`PageBackend`], so layout and editor code never know
//! which one is active.
//!
//! Intended Iced integration:
//! - CPU mode: [`PageOutput::Bitmap`] is uploaded as an `iced::widget::image::Handle` (or drawn
//!   by a canvas), cached by the backend's own page cache.
//! - GPU mode: [`PageOutput::Texture`] is a `wgpu::Texture` created on the *same* device Iced
//!   uses. It is drawn by an `iced::widget::shader` primitive, which gets `&Device`/`&Queue` in
//!   `prepare` and a render pass in `render`. See docs/spikes.md, Spike D.

use std::sync::Arc;

use libertum_layout::LayoutState;
use tiny_skia::Pixmap;

use crate::renderer::PageRenderer;

/// What a backend produced for one page.
pub enum PageOutput {
    /// Opaque RGBA8 bitmap (premultiplied, but the page is opaque so it is also straight).
    Bitmap(Arc<Pixmap>),
    /// GPU texture (`Rgba8Unorm`, linear storage format, `TEXTURE_BINDING | COPY_SRC`).
    #[cfg(feature = "gpu")]
    Texture(Arc<vello::wgpu::Texture>),
}

/// Renders pages of a laid-out document.
pub trait PageBackend {
    /// Short human-readable name ("cpu" or "vello").
    fn name(&self) -> &'static str;

    /// Renders (or fetches from the backend's cache) page `page_index` at `scale` (device
    /// pixels per point). `None` if the page does not exist or rendering failed.
    fn render(&mut self, state: &LayoutState, page_index: usize, scale: f32) -> Option<PageOutput>;
}

impl PageBackend for PageRenderer {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn render(&mut self, state: &LayoutState, page_index: usize, scale: f32) -> Option<PageOutput> {
        self.cached_page(state, page_index, scale)
            .map(PageOutput::Bitmap)
    }
}
