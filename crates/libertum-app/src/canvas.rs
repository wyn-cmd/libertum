//! Document canvas: a custom Iced widget that shows rasterised page bitmaps (PLAN §4.4).
//!
//! The rest of the app talks to the canvas through [`view`], [`CanvasProps`] and
//! [`CanvasEvent`]. The widget owns no document state:
//!
//! - **Scroll offset and zoom live in `App`** and are passed in. The widget turns wheel,
//!   scrollbar-drag and resize input into [`CanvasEvent`]s; `App` clamps and stores them.
//! - **Pages come from a [`PageSource`]** (`docview.rs`), which hands back a cached
//!   `image::Handle` per (page, content hash, scale), a stale stand-in, or nothing (blank page).
//!   The widget only *draws* the pages that intersect the viewport; it never renders.
//! - Cursor, selection and IME overlays (Phase 1+) will be drawn here on top of the bitmaps as
//!   Iced primitives, so caret blinking never touches the bitmap cache.
//!
//! # Units
//! All geometry is computed in **physical pixels** ([`Metrics`]) so that every page bitmap is
//! drawn 1:1 on a whole-pixel boundary (crisp text). Iced works in logical pixels, so the
//! widget divides by the window scale factor when it draws. Page bitmaps are rasterised at
//! `zoom * 96/72 * scale_factor` device pixels per point.

use std::cell::Cell;
use std::ops::Range;

use iced::advanced::image::{self, Image};
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{self, Text};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, mouse};
use iced::{
    Border, Color, Element, Event, Length, Pixels, Point, Rectangle, Size, Theme, keyboard, window,
};
use libertum_layout::{CaretRect, PageGeometry, SelectionRect};

use crate::docview::{BandImage, PageSource};
use crate::perf;
use crate::state::Zoom;

/// Gap between pages and padding around the page strip, in logical pixels.
const PAGE_GAP: f32 = 24.0;
const STRIP_PADDING: f32 = 24.0;
/// Logical pixels per wheel "line": one notch of a mouse wheel moves the page this far, the
/// distance browsers and Word use on Windows (three lines of text at the default setting).
const LINE_PX: f32 = 100.0;

/// Whether a wheel event is a notch of a mouse wheel rather than a trackpad's fine motion.
/// A wheel reports whole lines; a precision trackpad reports pixels (macOS, Linux) or small
/// fractions of a line many times a second (Windows), already smoothed by the system.
fn wheel_is_stepped(delta: &mouse::ScrollDelta) -> bool {
    match *delta {
        mouse::ScrollDelta::Lines { x, y } => {
            let whole = |v: f32| v != 0.0 && v.fract() == 0.0;
            (whole(x) || x == 0.0) && (whole(y) || y == 0.0) && (x != 0.0 || y != 0.0)
        }
        mouse::ScrollDelta::Pixels { .. } => false,
    }
}

/// Whether a wheel event carries no movement. Wayland sends an axis-source and an axis-stop
/// event around every touchpad gesture as zero-delta wheels; treating one as movement would
/// zero the fling velocity just as the fingers lift.
fn wheel_is_empty(delta: &mouse::ScrollDelta) -> bool {
    match *delta {
        mouse::ScrollDelta::Lines { x, y } => x == 0.0 && y == 0.0,
        mouse::ScrollDelta::Pixels { x, y } => x == 0.0 && y == 0.0,
    }
}
/// Logical px per point at 100% zoom (CSS 96 dpi over 72 pt per inch).
const PX_PER_PT: f32 = 96.0 / 72.0;
/// Scrollbar thickness and minimum thumb length, logical pixels.
const BAR: f32 = 10.0;
const MIN_THUMB: f32 = 28.0;
/// A second press within this window and slop counts as a double-click (third: triple).
const DOUBLE_CLICK_MAX: std::time::Duration = std::time::Duration::from_millis(400);
const CLICK_SLOP: f32 = 4.0;
/// Tint of the text Find found last (amber, translucent so the glyphs stay readable).
const FIND_MATCH_COLOUR: iced::Color = iced::Color {
    r: 1.0,
    g: 0.78,
    b: 0.0,
    a: 0.5,
};
/// Misspelling underline; readable on white paper in both themes.
const SPELL_COLOUR: iced::Color = iced::Color {
    r: 0.82,
    g: 0.16,
    b: 0.16,
    a: 1.0,
};

/// Drag-selection auto-scroll: minimum step and cap per pointer move, logical pixels.
const DRAG_SCROLL_MIN: f32 = 10.0;
const DRAG_SCROLL_MAX: f32 = 60.0;

// ---------------------------------------------------------------- metrics

/// Page strip geometry in physical pixels for one (zoom, scale factor, viewport) state.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    pub sf: f32,
    /// Device pixels per point: what the page renderer is asked for.
    pub scale: f32,
    pub page_w: f32,
    pub page_h: f32,
    pub gap: f32,
    pub pad: f32,
    pub pages: usize,
    /// Booklet view: page 1 alone, then pages 2-3, 4-5... side by side (left, right).
    pub booklet: bool,
    /// Markdown mode: no page gap or shadow, so the pages read as one continuous column.
    pub continuous: bool,
    /// Per-page sizes in physical pixels, index-aligned with the page count.
    pub sizes: Vec<(f32, f32)>,
    /// Per-page used heights in physical pixels. Only the pageless (continuous) view uses
    /// them: pages are stacked by how much of each is filled, so the blank tail of a page
    /// where a paragraph or table continued does not show as a gap.
    pub used: Vec<f32>,
}

impl Metrics {
    #[cfg(test)]
    pub fn new(
        geometry: &PageGeometry,
        zoom: Zoom,
        sf: f32,
        viewport_logical_width: f32,
        viewport_logical_height: f32,
        pages: usize,
    ) -> Self {
        Self::new_with_layout(
            geometry,
            zoom,
            sf,
            viewport_logical_width,
            viewport_logical_height,
            pages,
            false,
            false,
            &[],
            &[],
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_layout(
        geometry: &PageGeometry,
        zoom: Zoom,
        sf: f32,
        viewport_logical_width: f32,
        viewport_logical_height: f32,
        pages: usize,
        booklet: bool,
        continuous: bool,
        geometries: &[PageGeometry],
        used_heights: &[f32],
    ) -> Self {
        let sf = sf.max(0.5);
        let (max_w, max_h) = geometries.iter().fold(
            (geometry.width_pt, geometry.height_pt),
            |(w, h), geometry| (w.max(geometry.width_pt), h.max(geometry.height_pt)),
        );
        let logical_per_pt = match zoom {
            Zoom::Percent(p) => f32::from(p) / 100.0 * PX_PER_PT,
            Zoom::FitWidth => {
                let per_view = if booklet { 2.0 } else { 1.0 };
                let available =
                    (viewport_logical_width - 2.0 * STRIP_PADDING - (per_view - 1.0) * PAGE_GAP)
                        .max(100.0);
                (available / (max_w * per_view)).clamp(0.1 * PX_PER_PT, 4.0 * PX_PER_PT)
            }
            Zoom::FitPage => {
                let per_view = if booklet { 2.0 } else { 1.0 };
                let across =
                    (viewport_logical_width - 2.0 * STRIP_PADDING - (per_view - 1.0) * PAGE_GAP)
                        .max(100.0);
                let down = (viewport_logical_height - 2.0 * STRIP_PADDING).max(100.0);
                (across / (max_w * per_view))
                    .min(down / max_h)
                    .clamp(0.1 * PX_PER_PT, 4.0 * PX_PER_PT)
            }
        };
        // Round to 0.01 so a window drag in fit-width mode does not mint a new cache key for
        // every pixel of width.
        let scale = (logical_per_pt * sf * 100.0).round() / 100.0;
        let sizes: Vec<(f32, f32)> = if geometries.is_empty() {
            vec![(
                (geometry.width_pt * scale).ceil().max(1.0),
                (geometry.height_pt * scale).ceil().max(1.0),
            )]
        } else {
            geometries
                .iter()
                .map(|geometry| {
                    (
                        (geometry.width_pt * scale).ceil().max(1.0),
                        (geometry.height_pt * scale).ceil().max(1.0),
                    )
                })
                .collect()
        };
        let page_w = sizes.iter().map(|(w, _)| *w).fold(0.0, f32::max);
        let page_h = sizes.iter().map(|(_, h)| *h).fold(0.0, f32::max);
        // Used heights arrive in points (what pagination measures); the canvas works in
        // physical pixels, capped to the page they belong to.
        let used: Vec<f32> = (0..pages.max(1))
            .map(|page| {
                let full = sizes.get(page).map_or(page_h, |(_, h)| *h);
                if continuous {
                    used_heights
                        .get(page)
                        .map(|points| (points * scale).ceil().clamp(0.0, full))
                        .unwrap_or(full)
                } else {
                    full
                }
            })
            .collect();
        Self {
            sf,
            scale,
            page_w,
            page_h,
            gap: if continuous {
                0.0
            } else {
                (PAGE_GAP * sf).round()
            },
            pad: (STRIP_PADDING * sf).round(),
            pages: pages.max(1),
            booklet,
            continuous,
            sizes,
            used,
        }
    }

    /// The effective zoom as a whole percentage (what the toolbar shows, also for fit modes).
    pub fn percent(&self) -> u16 {
        (self.scale / self.sf / PX_PER_PT * 100.0)
            .round()
            .clamp(1.0, 9999.0) as u16
    }

    /// Distance between the tops of two page rows.
    pub fn pitch(&self) -> f32 {
        self.page_h + self.gap
    }

    /// Number of page rows: every page in the strip, or one page plus pairs in booklet view.
    fn rows(&self) -> usize {
        if self.booklet {
            1 + self.pages.saturating_sub(1).div_ceil(2)
        } else {
            self.pages
        }
    }

    /// Row of `page` (0-based).
    fn row_of(&self, page: usize) -> usize {
        if self.booklet {
            if page == 0 { 0 } else { page.div_ceil(2) }
        } else {
            page
        }
    }

    /// Booklet column: the lone first page sits on the right, then odd pages left, even right.
    fn column_of(&self, page: usize) -> usize {
        if !self.booklet {
            return 0;
        }
        match page {
            0 => 1,
            p if p % 2 == 1 => 0,
            _ => 1,
        }
    }

    /// Width of one row's pages, including the gap between a pair.
    fn block_width(&self) -> f32 {
        if self.booklet {
            2.0 * self.page_w + self.gap
        } else {
            self.page_w
        }
    }

    fn size(&self, page: usize) -> (f32, f32) {
        self.sizes
            .get(page)
            .copied()
            .unwrap_or((self.page_w, self.page_h))
    }

    pub fn content_height(&self) -> f32 {
        if self.booklet {
            return 2.0 * self.pad
                + self.rows() as f32 * self.page_h
                + (self.rows() - 1) as f32 * self.gap;
        }
        let pages = self.pages;
        let heights: f32 = (0..pages).map(|page| self.stack_height(page)).sum();
        2.0 * self.pad + heights + pages.saturating_sub(1) as f32 * self.gap
    }

    /// The height `page` adds to the stack: a continuous view uses only the part that has
    /// content, so pages meet where the text does rather than at the paper edge.
    fn stack_height(&self, page: usize) -> f32 {
        if self.continuous {
            self.used
                .get(page)
                .copied()
                .unwrap_or_else(|| self.size(page).1)
        } else {
            self.size(page).1
        }
    }

    pub fn content_width(&self, viewport_w: f32) -> f32 {
        viewport_w.max(self.block_width() + 2.0 * self.pad)
    }

    pub fn page_top(&self, page: usize) -> f32 {
        if self.booklet {
            return self.pad + self.row_of(page) as f32 * self.pitch();
        }
        let before: f32 = (0..page).map(|index| self.stack_height(index)).sum();
        self.pad + before + page as f32 * self.gap
    }

    /// Left edge of `page`'s rectangle in content coordinates.
    pub fn page_left(&self, page: usize, viewport_w: f32) -> f32 {
        if self.booklet {
            let x0 = ((self.content_width(viewport_w) - self.block_width()) * 0.5).floor();
            return x0 + self.column_of(page) as f32 * (self.page_w + self.gap);
        }
        let width = self.size(page).0;
        ((self.content_width(viewport_w) - width) * 0.5).floor()
    }

    /// Largest scroll offsets for a physical viewport size.
    pub fn max_scroll(&self, viewport: Size) -> (f32, f32) {
        (
            (self.content_width(viewport.width) - viewport.width).max(0.0),
            (self.content_height() - viewport.height).max(0.0),
        )
    }

    /// Pages whose rectangle intersects the viewport.
    pub fn visible_pages(&self, scroll_y: f32, viewport_h: f32) -> Range<usize> {
        if !self.booklet {
            let mut first = 0usize;
            while first + 1 < self.pages && self.page_top(first + 1) <= scroll_y {
                first += 1;
            }
            let bottom = scroll_y + viewport_h;
            let mut last = first;
            while last + 1 < self.pages && self.page_top(last + 1) < bottom {
                last += 1;
            }
            return first.min(self.pages - 1)..(last + 1).min(self.pages);
        }
        let rows = self.rows();
        let row = |y: f32| {
            (((y - self.pad) / self.pitch()).floor().max(0.0) as usize).min(rows.saturating_sub(1))
        };
        let first_row = row(scroll_y);
        let last_row = row(scroll_y + viewport_h);
        let (first, last) = if !self.booklet {
            (first_row, last_row)
        } else {
            let first = if first_row == 0 { 0 } else { 2 * first_row - 1 };
            let last = if last_row == 0 {
                0
            } else {
                (2 * last_row).min(self.pages - 1)
            };
            (first, last)
        };
        first.min(self.pages - 1)..(last + 1).min(self.pages)
    }

    /// The page under a content-space point (physical pixels).
    pub fn page_at(&self, x: f32, y: f32, viewport_w: f32) -> usize {
        if !self.booklet {
            let mut page = 0usize;
            while page + 1 < self.pages && self.page_top(page + 1) <= y {
                page += 1;
            }
            return page.min(self.pages - 1);
        }
        let rows = self.rows();
        let row =
            (((y - self.pad) / self.pitch()).floor().max(0.0) as usize).min(rows.saturating_sub(1));
        if row == 0 {
            return 0;
        }
        let left = self.page_left(2 * row - 1, viewport_w);
        let middle = left + self.page_w + self.gap * 0.5;
        let page = if x < middle { 2 * row - 1 } else { 2 * row };
        page.min(self.pages - 1)
    }

    /// The page under the middle of the viewport (0-based).
    pub fn current_page(&self, scroll_y: f32, viewport_h: f32, viewport_w: f32) -> usize {
        let mid_x = self.content_width(viewport_w) * 0.5;
        let mid_y = scroll_y + viewport_h * 0.5;
        self.page_at(mid_x, mid_y, viewport_w)
    }

    /// Scroll position of the viewport middle as a fractional page row (whole = row top).
    pub fn anchor_of(&self, scroll_y: f32, viewport_h: f32) -> f32 {
        (scroll_y + viewport_h * 0.5 - self.pad) / self.pitch()
    }

    /// Inverse of [`anchor_of`](Self::anchor_of).
    pub fn scroll_for_anchor(&self, anchor: f32, viewport_h: f32) -> f32 {
        anchor * self.pitch() + self.pad - viewport_h * 0.5
    }

    /// Page rectangle in content coordinates (physical pixels).
    fn page_rect(&self, page: usize, viewport_w: f32) -> Rectangle {
        let (width, height) = self.size(page);
        Rectangle::new(
            Point::new(self.page_left(page, viewport_w), self.page_top(page)),
            Size::new(width, height),
        )
    }
}

// ---------------------------------------------------------------- widget API

/// Input to the canvas, owned by `App`.
#[derive(Clone)]
pub struct CanvasProps {
    pub zoom: Zoom,
    pub geometry: PageGeometry,
    pub page_count: usize,
    pub scale_factor: f32,
    /// Scroll offset in physical pixels.
    pub scroll: (f32, f32),
    pub source: PageSource,
    /// Caret rectangle in page coordinates, drawn as an overlay (never baked into the bitmap).
    pub caret: Option<CaretRect>,
    /// Selection highlight rectangles in page coordinates.
    pub selection: Vec<SelectionRect>,
    /// The text Find found last, in page coordinates: drawn in a stronger colour over the
    /// selection so the match stands out exactly.
    pub find: Vec<SelectionRect>,
    /// Misspelling underline rectangles, page coordinates.
    pub spell: Vec<SelectionRect>,
    /// IME composition text, drawn inline at the caret until it is committed.
    pub preedit: Option<PreeditOverlay>,
    /// Keep requesting a redraw every frame (autoscroll test; `App` ticks on `window::frames`).
    pub animate: bool,
    /// Booklet view: page 1 alone, then pairs side by side.
    pub booklet: bool,
    /// Markdown mode: no page gap or shadow.
    pub continuous: bool,
    /// Per-page geometry, index-aligned with the page count.
    pub geometries: Vec<PageGeometry>,
    /// How much of each page is used, in points (pageless pages are stacked by this, so a
    /// paragraph or table that continues leaves no page-sized gap).
    pub used_heights: Vec<f32>,
    /// The document has keyboard focus, so the OS input method (IME) must stay enabled.
    pub ime_enabled: bool,
    /// Where the IME candidate window should sit: the caret, unaffected by blinking.
    pub ime_caret: Option<CaretRect>,
}

/// IME pre-edit text positioned in page coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct PreeditOverlay {
    pub page: usize,
    pub x_pt: f32,
    pub y_pt: f32,
    pub height_pt: f32,
    pub text: String,
}

/// What the canvas reports back to `App`.
#[derive(Debug, Clone, Copy)]
pub enum CanvasEvent {
    /// Wheel scrolling, in logical pixels (positive y scrolls down).
    /// Scroll by this many logical pixels. `stepped` is true for a mouse wheel's notches,
    /// which the app glides to; a trackpad's fine deltas (and a drag past the edge) are
    /// applied as they come, because the system already smooths them.
    Wheel { dx: f32, dy: f32, stepped: bool },
    /// Scrollbar drag: absolute offsets in physical pixels (`None` leaves an axis alone).
    ScrollTo { x: Option<f32>, y: Option<f32> },
    /// The widget's size in logical pixels changed.
    Viewport(Size),
    /// A right click on the page point (`x`/`y` are window logical pixels, where the menu
    /// goes).
    ContextMenu {
        page: usize,
        x_pt: f32,
        y_pt: f32,
        x: f32,
        y: f32,
    },
    /// A left click inside the page strip, in page points. `extend` is shift-click and
    /// `clicks` is the consecutive click count (2 = double, 3 = triple).
    Pointer {
        page: usize,
        x_pt: f32,
        y_pt: f32,
        extend: bool,
        clicks: u8,
    },
    /// The pointer moved while the left button is held: extend the selection to this point.
    PointerDrag { page: usize, x_pt: f32, y_pt: f32 },
    /// A click landed on the inspector (right of the canvas, below its top edge): document
    /// keystrokes must stop until the canvas is clicked again. iced 0.14's `text_input` has
    /// no focus event, and it captures the click, so the canvas reports it instead.
    Blur,
    /// Ctrl/Cmd + wheel (a trackpad pinch on Windows) or a Wayland pinch gesture: zoom by
    /// `factor` around a page point.
    Zoom {
        factor: f32,
        page: usize,
        x_pt: f32,
        y_pt: f32,
        /// Pointer position in physical pixels inside the canvas.
        cursor: Point,
    },
}

/// True when a press at `p` (window coordinates) is in the inspector column: to the right of
/// the canvas `bounds` and not above them (the toolbar is above and keeps document focus).
pub fn press_is_in_inspector(bounds: Rectangle, p: Point) -> bool {
    p.x >= bounds.x + bounds.width && p.y >= bounds.y
}

/// Zoom factor for one pinch update. The gesture reports its scale from the start, and winit
/// gives the change since the previous update, so the new factor is `1 + delta`.
fn pinch_zoom_factor(delta: f32) -> f32 {
    (1.0 + delta).clamp(0.8, 1.25)
}

/// Zoom factor for one wheel event used as Ctrl/Cmd + wheel (a Windows trackpad pinch):
/// up zooms in, down zooms out, and pixel deltas zoom proportionally.
fn wheel_zoom_factor(delta: &mouse::ScrollDelta) -> f32 {
    match *delta {
        mouse::ScrollDelta::Lines { y, .. } => {
            if y > 0.0 {
                1.1
            } else {
                1.0 / 1.1
            }
        }
        mouse::ScrollDelta::Pixels { y, .. } => (-y * 0.002).exp().clamp(0.8, 1.25),
    }
}

pub fn view<'a, Message: 'a>(
    props: CanvasProps,
    on_event: impl Fn(CanvasEvent) -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(DocCanvas {
        props,
        on_event: Box::new(on_event),
    })
}

struct DocCanvas<'a, Message> {
    props: CanvasProps,
    on_event: Box<dyn Fn(CanvasEvent) -> Message + 'a>,
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Vertical,
    Horizontal,
}

#[derive(Default)]
struct State {
    reported: Option<Size>,
    /// Active scrollbar drag: axis and the cursor's offset inside the thumb (logical px).
    drag: Option<(Axis, f32)>,
    /// A text selection drag is in progress (the button went down inside the page strip).
    selecting: bool,
    /// Previous press, for double- and triple-click detection.
    last_press: Option<(std::time::Instant, Point, u8)>,
    shift: bool,
    command: bool,
    /// Scroll offset at the previous draw, to tell "scrolling" frames from idle ones.
    last_drawn_scroll: Cell<(f32, f32)>,
}

struct Bar {
    track: Rectangle,
    thumb: Rectangle,
    /// Largest scroll offset on this axis, physical pixels.
    max: f32,
}

impl<Message> DocCanvas<'_, Message> {
    fn metrics(&self, bounds: Rectangle) -> Metrics {
        Metrics::new_with_layout(
            &self.props.geometry,
            self.props.zoom,
            self.props.scale_factor,
            bounds.width,
            bounds.height,
            self.props.page_count,
            self.props.booklet,
            self.props.continuous,
            &self.props.geometries,
            &self.props.used_heights,
        )
    }

    /// Scroll offset clamped to the current extent (physical pixels).
    /// The input-method request for `event`: on every redraw while the document is focused.
    fn ime_request(
        &self,
        event: &Event,
        bounds: Rectangle,
    ) -> Option<iced::advanced::InputMethod<String>> {
        let redraw = matches!(event, Event::Window(window::Event::RedrawRequested(_)));
        (self.props.ime_enabled && redraw).then(|| iced::advanced::InputMethod::Enabled {
            cursor: self.ime_cursor_area(bounds),
            purpose: iced::advanced::input_method::Purpose::Normal,
            preedit: None,
        })
    }

    /// The caret in window coordinates (logical pixels), for placing the IME candidate
    /// window. Falls back to the canvas's top-left corner when the caret is unknown.
    fn ime_cursor_area(&self, bounds: Rectangle) -> Rectangle {
        let m = self.metrics(bounds);
        let viewport = Size::new(bounds.width * m.sf, bounds.height * m.sf);
        let (sx, sy) = self.clamped_scroll(&m, viewport);
        self.props
            .ime_caret
            .and_then(|c| {
                page_point_rect(
                    &m,
                    viewport,
                    bounds,
                    sx,
                    sy,
                    c.page,
                    c.x,
                    c.y,
                    c.width.max(1.0),
                    c.height,
                )
            })
            .unwrap_or_else(|| Rectangle::new(bounds.position(), Size::new(2.0, 18.0)))
    }

    fn clamped_scroll(&self, m: &Metrics, viewport: Size) -> (f32, f32) {
        let (mx, my) = m.max_scroll(viewport);
        (
            self.props.scroll.0.clamp(0.0, mx),
            self.props.scroll.1.clamp(0.0, my),
        )
    }

    /// Maps a point inside the widget (logical pixels) to a page and page point (points).
    fn page_point(&self, bounds: Rectangle, p: Point) -> (usize, f32, f32) {
        let m = self.metrics(bounds);
        let viewport = Size::new(bounds.width * m.sf, bounds.height * m.sf);
        let (sx, sy) = self.clamped_scroll(&m, viewport);
        let cx = p.x * m.sf + sx;
        let cy = p.y * m.sf + sy;
        let page = m.page_at(cx, cy, viewport.width);
        let pr = m.page_rect(page, viewport.width);
        (page, (cx - pr.x) / m.scale, (cy - pr.y) / m.scale)
    }

    /// Scrollbars in logical coordinates relative to the widget's top-left.
    fn bars(&self, bounds: Rectangle) -> (Option<Bar>, Option<Bar>) {
        let m = self.metrics(bounds);
        let sf = m.sf;
        let viewport = Size::new(bounds.width * sf, bounds.height * sf);
        let (mx, my) = m.max_scroll(viewport);
        let (sx, sy) = self.clamped_scroll(&m, viewport);
        let h_visible = mx > 0.0;

        let v = (my > 0.0).then(|| {
            let track_h = bounds.height - if h_visible { BAR } else { 0.0 };
            let ratio = (viewport.height / m.content_height()).clamp(0.0, 1.0);
            let thumb_h = (track_h * ratio).clamp(MIN_THUMB.min(track_h), track_h);
            let y = (track_h - thumb_h) * (sy / my);
            Bar {
                track: Rectangle::new(Point::new(bounds.width - BAR, 0.0), Size::new(BAR, track_h)),
                thumb: Rectangle::new(Point::new(bounds.width - BAR, y), Size::new(BAR, thumb_h)),
                max: my,
            }
        });
        let h = h_visible.then(|| {
            let track_w = bounds.width - if v.is_some() { BAR } else { 0.0 };
            let ratio = (viewport.width / m.content_width(viewport.width)).clamp(0.0, 1.0);
            let thumb_w = (track_w * ratio).clamp(MIN_THUMB.min(track_w), track_w);
            let x = (track_w - thumb_w) * (sx / mx);
            Bar {
                track: Rectangle::new(
                    Point::new(0.0, bounds.height - BAR),
                    Size::new(track_w, BAR),
                ),
                thumb: Rectangle::new(Point::new(x, bounds.height - BAR), Size::new(thumb_w, BAR)),
                max: mx,
            }
        });
        (v, h)
    }
}

/// The part of page rectangle `page` (logical pixels) that a strip covers. Fractions of the
/// same plan meet exactly, so adjacent strips leave no seam.
fn band_rect(page: Rectangle, band: &BandImage) -> Rectangle {
    let top = page.y + band.top * page.height;
    let bottom = page.y + band.bottom * page.height;
    Rectangle::new(Point::new(page.x, top), Size::new(page.width, bottom - top))
}

fn page_image(handle: image::Handle) -> Image {
    Image::new(handle).snap(true).filter_method(page_filter())
}

/// Soft page shadow from thin translucent strips on all four sides (light from the top, so
/// the shadow is offset down by 2 px).
///
/// Not `Quad::shadow`: the tiny-skia backend does not clip quad shadows to the layer, so a page
/// below the viewport darkened the status bar (the "dark strip" of old bug 2c). Large stacked
/// translucent quads were also tried first and cost about 60 ms per frame in tiny-skia, so
/// only edge strips are drawn.
fn draw_shadow<R: iced::advanced::Renderer>(renderer: &mut R, rect: Rectangle) {
    const ALPHAS: [f32; 4] = [0.10, 0.06, 0.035, 0.015];
    for (i, alpha) in ALPHAS.into_iter().enumerate() {
        let d = i as f32;
        let colour = Color::from_rgba(0.0, 0.0, 0.0, alpha);
        let strips = [
            // right, left, bottom, top
            Rectangle::new(
                Point::new(rect.x + rect.width + d, rect.y + 2.0),
                Size::new(1.0, rect.height),
            ),
            Rectangle::new(
                Point::new(rect.x - 1.0 - d, rect.y + 2.0),
                Size::new(1.0, rect.height),
            ),
            Rectangle::new(
                Point::new(rect.x - d, rect.y + rect.height + 2.0 + d),
                Size::new(rect.width + 2.0 * d, 1.0),
            ),
            Rectangle::new(
                Point::new(rect.x - d, rect.y - 1.0 - d + 2.0),
                Size::new(rect.width + 2.0 * d, 1.0),
            ),
        ];
        for bounds in strips {
            renderer.fill_quad(
                Quad {
                    bounds,
                    ..Quad::default()
                },
                colour,
            );
        }
    }
}

/// Page bitmaps are drawn 1:1 in device pixels, so no resampling is wanted.
/// `LIBERTUM_FILTER=linear` restores bilinear for comparison runs.
fn page_filter() -> image::FilterMethod {
    use std::sync::OnceLock;
    static F: OnceLock<image::FilterMethod> = OnceLock::new();
    *F.get_or_init(|| {
        if std::env::var("LIBERTUM_FILTER").is_ok_and(|v| v == "linear") {
            image::FilterMethod::Linear
        } else {
            image::FilterMethod::Nearest
        }
    })
}

fn contains(r: Rectangle, p: Point) -> bool {
    p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height
}

/// Converts a rectangle in page points to logical window coordinates.
#[allow(clippy::too_many_arguments)]
fn page_point_rect(
    m: &Metrics,
    viewport: Size,
    bounds: Rectangle,
    sx: f32,
    sy: f32,
    page: usize,
    x_pt: f32,
    y_pt: f32,
    width_pt: f32,
    height_pt: f32,
) -> Option<Rectangle> {
    if page >= m.pages {
        return None;
    }
    let pr = m.page_rect(page, viewport.width);
    let x = pr.x + x_pt * m.scale;
    let y = pr.y + y_pt * m.scale;
    Some(Rectangle::new(
        Point::new(bounds.x + (x - sx) / m.sf, bounds.y + (y - sy) / m.sf),
        Size::new(
            (width_pt * m.scale).max(0.0) / m.sf,
            (height_pt * m.scale).max(0.0) / m.sf,
        ),
    ))
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for DocCanvas<'_, Message>
where
    Renderer: iced::advanced::Renderer
        + image::Renderer<Handle = image::Handle>
        + iced::advanced::text::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &mut self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        let bounds = layout.bounds();

        // Report the widget size to `App` (it needs it for scroll clamping and zoom anchors).
        if matches!(event, Event::Window(window::Event::RedrawRequested(_)))
            && state.reported != Some(bounds.size())
        {
            state.reported = Some(bounds.size());
            shell.publish((self.on_event)(CanvasEvent::Viewport(bounds.size())));
        }

        if self.props.animate && matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
            shell.request_redraw();
        }

        // iced_winit 0.14 applies the input-method requests of each redraw pass and *disables*
        // the IME when no widget asks (as iced's own `text_input` re-asks on every redraw while
        // focused). Asking only on click let the next redraw switch Korean, Chinese and
        // Japanese input straight off again.
        if let Some(ime) = self.ime_request(event, bounds) {
            shell.request_input_method(&ime);
        }

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
                state.shift = m.shift();
                state.command = m.command();
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                if wheel_is_empty(delta) {
                    shell.capture_event();
                    return;
                }
                // Ctrl/Cmd + wheel zooms around the pointer. Windows trackpads report a pinch
                // as Ctrl+wheel, so this is also the trackpad pinch path there; Wayland
                // pinches arrive as `mouse::Event::Pinch` below.
                if state.command
                    && let Some(p) = cursor.position_in(bounds)
                {
                    let m = self.metrics(bounds);
                    let (page, x_pt, y_pt) = self.page_point(bounds, p);
                    let factor = wheel_zoom_factor(delta);
                    shell.publish((self.on_event)(CanvasEvent::Zoom {
                        factor,
                        page,
                        x_pt,
                        y_pt,
                        cursor: Point::new(p.x * m.sf, p.y * m.sf),
                    }));
                    shell.capture_event();
                    return;
                }
                let stepped = wheel_is_stepped(delta);
                let (mut dx, mut dy) = match *delta {
                    mouse::ScrollDelta::Lines { x, y } => (-x * LINE_PX, -y * LINE_PX),
                    mouse::ScrollDelta::Pixels { x, y } => (-x, -y),
                };
                if state.shift && dx == 0.0 {
                    (dx, dy) = (dy, 0.0);
                }
                shell.publish((self.on_event)(CanvasEvent::Wheel { dx, dy, stepped }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::Pinch { delta }) if cursor.is_over(bounds) => {
                // A Wayland touchpad pinch. Each event carries the change in scale since the
                // previous update, so it zooms like Ctrl/Cmd + wheel does on other platforms.
                if let Some(p) = cursor.position_in(bounds) {
                    let m = self.metrics(bounds);
                    let (page, x_pt, y_pt) = self.page_point(bounds, p);
                    let factor = pinch_zoom_factor(*delta);
                    shell.publish((self.on_event)(CanvasEvent::Zoom {
                        factor,
                        page,
                        x_pt,
                        y_pt,
                        cursor: Point::new(p.x * m.sf, p.y * m.sf),
                    }));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let (Some(window), Some(inside)) = (cursor.position(), cursor.position_in(bounds))
                else {
                    return;
                };
                let (page, x_pt, y_pt) = self.page_point(bounds, inside);
                shell.publish((self.on_event)(CanvasEvent::ContextMenu {
                    page,
                    x_pt,
                    y_pt,
                    x: window.x,
                    y: window.y,
                }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(at) = cursor.position()
                    && press_is_in_inspector(bounds, at)
                {
                    shell.publish((self.on_event)(CanvasEvent::Blur));
                    return;
                }
                let Some(p) = cursor.position_in(bounds) else {
                    return;
                };
                let (v, h) = self.bars(bounds);
                let hit = [(Axis::Vertical, v), (Axis::Horizontal, h)]
                    .into_iter()
                    .find_map(|(axis, bar)| {
                        let bar = bar?;
                        contains(bar.thumb, p).then(|| {
                            let grab = match axis {
                                Axis::Vertical => p.y - bar.thumb.y,
                                Axis::Horizontal => p.x - bar.thumb.x,
                            };
                            (axis, grab)
                        })
                    });
                if let Some(drag) = hit {
                    state.drag = Some(drag);
                    shell.capture_event();
                } else {
                    // A click in the page strip moves the caret. A second and third click at
                    // the same spot select the word and the paragraph. Ask the OS to enable
                    // the input method so IME composition events arrive at the app.
                    let now = std::time::Instant::now();
                    let clicks = match state.last_press {
                        Some((then, last, n))
                            if now.duration_since(then) <= DOUBLE_CLICK_MAX
                                && last.distance(p) <= CLICK_SLOP =>
                        {
                            (n + 1).min(3)
                        }
                        _ => 1,
                    };
                    state.last_press = Some((now, p, clicks));
                    let (page, x_pt, y_pt) = self.page_point(bounds, p);
                    shell.publish((self.on_event)(CanvasEvent::Pointer {
                        page,
                        x_pt,
                        y_pt,
                        extend: state.shift,
                        clicks,
                    }));
                    // A single click starts a text selection drag; a double/triple click
                    // selects a word/paragraph and is not dragged.
                    state.selecting = clicks == 1;
                    shell.request_input_method(&iced::advanced::InputMethod::<String>::Enabled {
                        cursor: Rectangle::new(p, Size::new(2.0, 18.0)),
                        purpose: iced::advanced::input_method::Purpose::Normal,
                        preedit: None,
                    });
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was_selecting = std::mem::replace(&mut state.selecting, false);
                if state.drag.take().is_some() || was_selecting {
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(pos) = cursor.position() else {
                    return;
                };
                let p = Point::new(pos.x - bounds.x, pos.y - bounds.y);
                if state.selecting {
                    // Extend the selection to the pointer. Dragging past the top or bottom
                    // edge also scrolls, so a selection can run beyond one screen.
                    let (page, x_pt, y_pt) = self.page_point(bounds, p);
                    shell.publish((self.on_event)(CanvasEvent::PointerDrag {
                        page,
                        x_pt,
                        y_pt,
                    }));
                    let over = if p.y < 0.0 {
                        p.y
                    } else if p.y > bounds.height {
                        p.y - bounds.height
                    } else {
                        0.0
                    };
                    if over != 0.0 {
                        let dy =
                            over.signum() * (over.abs().min(DRAG_SCROLL_MAX) + DRAG_SCROLL_MIN);
                        shell.publish((self.on_event)(CanvasEvent::Wheel {
                            dx: 0.0,
                            dy,
                            stepped: false,
                        }));
                    }
                    shell.capture_event();
                    return;
                }
                let Some((axis, grab)) = state.drag else {
                    return;
                };
                let (v, h) = self.bars(bounds);
                let bar = match axis {
                    Axis::Vertical => v,
                    Axis::Horizontal => h,
                };
                let Some(bar) = bar else { return };
                let (along, track_len, thumb_len) = match axis {
                    Axis::Vertical => (p.y - grab, bar.track.height, bar.thumb.height),
                    Axis::Horizontal => (p.x - grab, bar.track.width, bar.thumb.width),
                };
                let travel = (track_len - thumb_len).max(1.0);
                let target = (along / travel).clamp(0.0, 1.0) * bar.max;
                shell.publish((self.on_event)(match axis {
                    Axis::Vertical => CanvasEvent::ScrollTo {
                        x: None,
                        y: Some(target),
                    },
                    Axis::Horizontal => CanvasEvent::ScrollTo {
                        x: Some(target),
                        y: None,
                    },
                }));
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let Some(p) = cursor.position_in(bounds) else {
            return mouse::Interaction::None;
        };
        let (v, h) = self.bars(bounds);
        if [v, h].iter().flatten().any(|b| contains(b.thumb, p)) {
            mouse::Interaction::Idle
        } else if contains(Rectangle::new(Point::new(0.0, 0.0), bounds.size()), p) {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::None
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let started = std::time::Instant::now();
        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let m = self.metrics(bounds);
        let sf = m.sf;
        let viewport = Size::new(bounds.width * sf, bounds.height * sf);
        let (sx, sy) = self.clamped_scroll(&m, viewport);

        let scrolling = state.last_drawn_scroll.get() != (sx, sy);
        state.last_drawn_scroll.set((sx, sy));
        perf::frame(scrolling);

        let mut blank = false;
        let mut stale = false;
        renderer.with_layer(bounds, |renderer| {
            for page in m.visible_pages(sy, viewport.height) {
                let r = m.page_rect(page, viewport.width);
                // Physical content coordinates to logical window coordinates.
                let rect = Rectangle::new(
                    Point::new(bounds.x + (r.x - sx) / sf, bounds.y + (r.y - sy) / sf),
                    Size::new(r.width / sf, r.height / sf),
                );
                if !m.continuous {
                    draw_shadow(renderer, rect);
                }
                // White paper under every page. Quads paint below images in a layer, so
                // it only shows where no strip is drawn yet.
                renderer.fill_quad(
                    Quad {
                        bounds: rect,
                        ..Quad::default()
                    },
                    Color::WHITE,
                );
                // Older strips first, current strips over them (images in one layer paint in
                // submission order), so a strip that is re-rendering after an edit keeps
                // showing its previous pixels instead of white.
                let bands = self.props.source.bands(page, m.scale);
                for band in bands.under.iter().chain(&bands.over) {
                    renderer.draw_image(
                        page_image(band.handle.clone()),
                        band_rect(rect, band),
                        bounds,
                    );
                }
                if !bands.complete {
                    if bands.under.is_empty() && bands.over.is_empty() {
                        blank = true;
                    } else {
                        stale = true;
                    }
                }
            }
        });

        // Iced records a layer's primitives in *type* order (quads, then images, then text),
        // so a quad added after an image still draws underneath it. The overlays therefore
        // need their own layer, opened after the page bitmaps: a quads-only layer cannot be
        // merged back into the layer that holds the images, so it is painted on top.
        renderer.with_layer(bounds, |renderer| {
            // Overlays (PLAN §4.4): selection, caret and IME composition are Iced primitives,
            // never baked into the page bitmap, so blinking/moving never re-rasterises.
            let palette = theme.extended_palette();
            // The accent at 28%. `primary.weak` is already mixed 40% towards the window
            // background, which made the highlight nearly invisible (it read as "selection
            // does not work" rather than "the highlight is faint").
            let selection_colour = palette.primary.base.color.scale_alpha(0.28);
            for s in &self.props.selection {
                if let Some(rect) = page_point_rect(
                    &m, viewport, bounds, sx, sy, s.page, s.x, s.y, s.width, s.height,
                ) && rect.intersects(&bounds)
                {
                    renderer.fill_quad(
                        Quad {
                            bounds: rect,
                            ..Quad::default()
                        },
                        selection_colour,
                    );
                }
            }

            // The current Find match: amber over the selection tint, so it reads as "found
            // here" rather than as an ordinary selection, on white pages in both themes.
            for s in &self.props.find {
                if let Some(rect) = page_point_rect(
                    &m, viewport, bounds, sx, sy, s.page, s.x, s.y, s.width, s.height,
                ) && rect.intersects(&bounds)
                {
                    renderer.fill_quad(
                        Quad {
                            bounds: rect,
                            ..Quad::default()
                        },
                        FIND_MATCH_COLOUR,
                    );
                }
            }

            // Misspellings: a thin red underline at the bottom of each word's line boxes.
            for s in &self.props.spell {
                if let Some(rect) = page_point_rect(
                    &m, viewport, bounds, sx, sy, s.page, s.x, s.y, s.width, s.height,
                ) && rect.intersects(&bounds)
                {
                    renderer.fill_quad(
                        Quad {
                            bounds: Rectangle::new(
                                Point::new(rect.x, rect.y + rect.height - 1.5),
                                Size::new(rect.width, 1.5),
                            ),
                            ..Quad::default()
                        },
                        SPELL_COLOUR,
                    );
                }
            }

            if let Some(caret) = self.props.caret
                && let Some(rect) = page_point_rect(
                    &m,
                    viewport,
                    bounds,
                    sx,
                    sy,
                    caret.page,
                    caret.x,
                    caret.y,
                    // A zero-width rect never intersects the viewport (`Rectangle::intersects`
                    // needs a positive overlap), which used to hide the caret completely.
                    caret.width.max(0.75),
                    caret.height,
                )
                && rect.intersects(&bounds)
            {
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle::new(rect.position(), Size::new(1.5, rect.height)),
                        ..Quad::default()
                    },
                    palette.primary.strong.color,
                );
            }

            if let Some(pre) = &self.props.preedit
                && let Some(rect) = page_point_rect(
                    &m,
                    viewport,
                    bounds,
                    sx,
                    sy,
                    pre.page,
                    pre.x_pt,
                    pre.y_pt,
                    // Not zero: `Rectangle::intersects` requires a positive overlap.
                    1.0,
                    pre.height_pt,
                )
                && rect.intersects(&bounds)
            {
                let size = (pre.height_pt * m.scale / m.sf).max(6.0);
                let colour = palette.primary.strong.color;
                renderer.fill_text(
                    Text {
                        content: pre.text.clone(),
                        bounds: Size::new(bounds.width, size * 1.4),
                        size: Pixels(size),
                        line_height: text::LineHeight::Relative(1.0),
                        font: renderer.default_font(),
                        align_x: text::Alignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: text::Shaping::Advanced,
                        wrapping: text::Wrapping::None,
                    },
                    rect.position(),
                    colour,
                    bounds,
                );
                renderer.fill_quad(
                    Quad {
                        bounds: Rectangle::new(
                            Point::new(rect.x, rect.y + rect.height),
                            Size::new(rect.width.max(2.0), 1.0),
                        ),
                        ..Quad::default()
                    },
                    colour,
                );
            }

            // Scrollbars (overlay, so they never reflow the page strip).
            let (v, h) = self.bars(bounds);
            let hover = cursor.position_in(bounds);
            for (axis, bar) in [(Axis::Vertical, v), (Axis::Horizontal, h)] {
                let Some(bar) = bar else { continue };
                let active = state.drag.is_some_and(|(a, _)| a == axis)
                    || hover.is_some_and(|p| contains(bar.thumb, p));
                let alpha = if active { 0.65 } else { 0.35 };
                let thumb = Rectangle::new(
                    Point::new(bounds.x + bar.thumb.x + 2.0, bounds.y + bar.thumb.y + 2.0),
                    Size::new(bar.thumb.width - 4.0, bar.thumb.height - 4.0),
                );
                renderer.fill_quad(
                    Quad {
                        bounds: thumb,
                        border: Border {
                            radius: 3.0.into(),
                            ..Border::default()
                        },
                        ..Quad::default()
                    },
                    palette.background.strongest.color.scale_alpha(alpha),
                );
            }
        });
        perf::draw_done(started.elapsed(), blank, stale);
        perf::event("draw", started.elapsed().as_secs_f64() * 1000.0, || {
            format!(
                "scroll y {sy:.0}{}{}",
                if blank { ", blank page" } else { "" },
                if stale { ", stale strips" } else { "" }
            )
        });
        if !blank && !stale {
            perf::screen_current();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(zoom: Zoom, sf: f32, vw: f32, pages: usize) -> Metrics {
        Metrics::new(&PageGeometry::a4(), zoom, sf, vw, 700.0, pages)
    }

    fn booklet(vw: f32, pages: usize) -> Metrics {
        Metrics::new_with_layout(
            &PageGeometry::a4(),
            Zoom::FitWidth,
            1.0,
            vw,
            700.0,
            pages,
            true,
            false,
            &[],
            &[],
        )
    }

    #[test]
    fn booklet_puts_page_one_alone_then_pairs_left_and_right() {
        let m = booklet(1400.0, 5);
        let w = m.page_w;
        let gap = m.gap;
        // Page 0 is alone on row 0, in the right column.
        assert_eq!(m.page_top(0), m.pad);
        assert_eq!(m.page_top(1), m.page_top(2), "pages 2 and 3 share a row");
        let r0 = m.page_rect(0, 1400.0);
        let r1 = m.page_rect(1, 1400.0);
        let r2 = m.page_rect(2, 1400.0);
        assert!(r0.x > r1.x, "the lone first page sits on the right");
        assert!(
            (r2.x - r1.x - (w + gap)).abs() < 1.0,
            "pair is left to right"
        );
        // Hit testing maps a point to the page under it.
        assert_eq!(m.page_at(r1.x + 10.0, r1.y + 10.0, 1400.0), 1);
        assert_eq!(m.page_at(r2.x + 10.0, r2.y + 10.0, 1400.0), 2);
        assert_eq!(m.visible_pages(m.page_top(0), 700.0).start, 0);
    }

    #[test]
    fn wheel_with_command_maps_to_zoom() {
        let up = mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 };
        let down = mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 };
        assert!(wheel_zoom_factor(&up) > 1.0);
        assert!(wheel_zoom_factor(&down) < 1.0);
        // Pixel deltas are proportional and clamped per event.
        let pixels = |y| wheel_zoom_factor(&mouse::ScrollDelta::Pixels { x: 0.0, y });
        assert!(pixels(10.0) < 1.0, "pixels down zoom out");
        assert!((pixels(10.0) - (-10.0f32 * 0.002).exp()).abs() < 1e-6);
        assert!((pixels(10_000.0) - 0.8).abs() < 1e-6, "clamped");
    }

    #[test]
    fn a4_aspect_ratio_is_kept() {
        let m = m(Zoom::Percent(100), 1.0, 1000.0, 3);
        assert!((m.page_h / m.page_w - 841.89 / 595.276).abs() < 2e-3);
    }

    #[test]
    fn hidpi_scales_the_bitmap_not_the_layout() {
        let a = m(Zoom::Percent(100), 1.0, 1000.0, 3);
        let b = m(Zoom::Percent(100), 1.25, 1000.0, 3);
        // Scale is rounded to 0.01 (cache-key stability), worth up to ~0.005 x page width.
        assert!((b.page_w / b.sf - a.page_w).abs() <= 4.0);
        assert!((b.scale / a.scale - 1.25).abs() < 0.01);
    }

    #[test]
    fn ime_is_requested_on_every_redraw_while_the_document_is_focused() {
        use crate::docview::DocView;
        let (view, _rx) = DocView::open(libertum_layout::spike::generate_model_document(1, 1));
        let caret = view.caret_rect();
        let props = |ime_enabled| CanvasProps {
            zoom: Zoom::Percent(100),
            geometry: view.geometry,
            page_count: view.page_total(),
            scale_factor: 1.0,
            scroll: (0.0, 0.0),
            source: view.source.clone(),
            caret,
            selection: Vec::new(),
            find: Vec::new(),
            spell: Vec::new(),
            preedit: None,
            animate: false,
            booklet: false,
            continuous: false,
            geometries: Vec::new(),
            used_heights: Vec::new(),
            ime_enabled,
            ime_caret: caret,
        };
        let bounds = Rectangle::new(Point::new(0.0, 52.0), Size::new(900.0, 700.0));
        let redraw = Event::Window(window::Event::RedrawRequested(std::time::Instant::now()));
        let focused = DocCanvas {
            props: props(true),
            on_event: Box::new(|e: CanvasEvent| e),
        };
        let Some(iced::advanced::InputMethod::Enabled { cursor, .. }) =
            focused.ime_request(&redraw, bounds)
        else {
            panic!("a focused document must keep the IME enabled on every redraw");
        };
        // The candidate window sits at the caret, inside the canvas.
        assert!(bounds.contains(cursor.position()));
        assert!(cursor.height > 4.0);
        // Not on other events, and not when the inspector has the keyboard.
        let other = Event::Keyboard(keyboard::Event::ModifiersChanged(
            keyboard::Modifiers::default(),
        ));
        assert!(focused.ime_request(&other, bounds).is_none());
        let blurred = DocCanvas {
            props: props(false),
            on_event: Box::new(|e: CanvasEvent| e),
        };
        assert!(blurred.ime_request(&redraw, bounds).is_none());
    }

    #[test]
    fn only_presses_in_the_inspector_column_blur_the_document() {
        let canvas = Rectangle::new(Point::new(0.0, 52.0), Size::new(900.0, 700.0));
        assert!(press_is_in_inspector(canvas, Point::new(950.0, 300.0)));
        assert!(
            !press_is_in_inspector(canvas, Point::new(400.0, 300.0)),
            "the canvas"
        );
        assert!(
            !press_is_in_inspector(canvas, Point::new(950.0, 20.0)),
            "the toolbar"
        );
    }

    #[test]
    fn fit_width_fills_viewport() {
        let m = m(Zoom::FitWidth, 1.0, 1000.0, 3);
        assert!((m.page_w + 2.0 * m.pad - 1000.0).abs() < 8.0);
    }

    #[test]
    fn visible_range_covers_the_viewport() {
        let m = m(Zoom::Percent(100), 1.0, 1000.0, 100);
        assert_eq!(m.visible_pages(0.0, 600.0), 0..1);
        let range = m.visible_pages(m.page_top(10) - 10.0, 600.0);
        assert!(range.contains(&9) && range.contains(&10));
        let last = m.visible_pages(m.content_height(), 600.0);
        assert_eq!(last.end, 100);
    }

    #[test]
    fn anchor_round_trips_across_zoom() {
        let a = m(Zoom::Percent(100), 1.0, 1000.0, 100);
        let b = m(Zoom::Percent(200), 1.0, 1000.0, 100);
        let scroll = a.page_top(20) + 300.0;
        let anchor = a.anchor_of(scroll, 700.0);
        let back = a.scroll_for_anchor(anchor, 700.0);
        assert!((back - scroll).abs() < 0.01);
        // The same page stays at the viewport middle after zooming.
        let z = b.scroll_for_anchor(anchor, 700.0);
        assert_eq!(
            b.current_page(z, 700.0, 1000.0),
            a.current_page(scroll, 700.0, 1000.0)
        );
    }

    #[test]
    fn current_page_is_clamped() {
        let m = m(Zoom::Percent(100), 1.0, 1000.0, 5);
        assert_eq!(m.current_page(0.0, 600.0, 1000.0), 0);
        assert_eq!(m.current_page(1.0e9, 600.0, 1000.0), 4);
    }
}

#[cfg(test)]
mod variable_page_tests {
    use super::*;

    /// A pageless column is one endless column: a page is followed by the next at the point
    /// where its text ended, not at the paper edge, so a paragraph or table that continues
    /// across the page boundary leaves no blank stretch.
    #[test]
    fn a_continuous_column_stacks_pages_by_the_text_they_use() {
        let geometry = PageGeometry::a4();
        let m0 = Metrics::new_with_layout(
            &geometry,
            Zoom::Percent(100),
            1.0,
            1000.0,
            700.0,
            3,
            false,
            true,
            &[geometry; 3],
            &[300.0, 500.0, 120.0],
        );
        let scale = m0.scale;
        let full = (geometry.height_pt * scale).ceil();
        let expect = [
            (300.0 * scale).ceil(),
            (500.0 * scale).ceil(),
            (120.0 * scale).ceil(),
        ];
        let m = Metrics::new_with_layout(
            &geometry,
            Zoom::Percent(100),
            1.0,
            1000.0,
            700.0,
            3,
            false,
            true,
            &[geometry; 3],
            &[300.0, 500.0, 120.0],
        );
        assert!(
            (m.used[0] - expect[0]).abs() < 1.0,
            "used 0 = {}",
            m.used[0]
        );
        assert!(
            (m.used[1] - expect[1]).abs() < 1.0,
            "used 1 = {}",
            m.used[1]
        );
        assert!(
            (m.page_top(1) - (m.page_top(0) + m.used[0])).abs() < 0.01,
            "page 1 follows page 0's text, not its paper"
        );
        assert!(
            (m.page_top(2) - (m.page_top(0) + m.used[0] + m.used[1])).abs() < 0.01,
            "page 2 follows page 1's text"
        );
        // The scroll extent is the text's, not three whole sheets of paper.
        assert!(
            (m.content_height() - (2.0 * m.pad + expect.iter().sum::<f32>())).abs() < 2.0,
            "content height {}",
            m.content_height()
        );
        // Without a pageless view the same figures leave whole pages: a page has its full
        // height even when little is on it.
        let paged = Metrics::new_with_layout(
            &geometry,
            Zoom::Percent(100),
            1.0,
            1000.0,
            700.0,
            3,
            false,
            false,
            &[geometry; 3],
            &[300.0, 500.0, 120.0],
        );
        assert!(
            (paged.used[0] - full).abs() < 1.01,
            "paged uses whole pages"
        );
        assert!(paged.page_top(1) >= paged.page_top(0) + full - 1.0);
        let _ = m0;
    }

    #[test]
    fn pages_can_have_different_sizes_in_one_column() {
        let portrait = PageGeometry::a4();
        let mut landscape = portrait;
        landscape.width_pt = portrait.height_pt;
        landscape.height_pt = portrait.width_pt;
        let m = Metrics::new_with_layout(
            &portrait,
            Zoom::Percent(100),
            1.0,
            1000.0,
            700.0,
            2,
            false,
            false,
            &[portrait, landscape],
            &[],
        );
        assert!(m.page_rect(1, 1000.0).width > m.page_rect(0, 1000.0).width);
        assert!((m.page_top(1) - (m.page_rect(0, 1000.0).height + m.gap + m.pad)).abs() < 0.01);
        assert_eq!(m.visible_pages(0.0, 2000.0), 0..2);
    }
}

#[cfg(test)]
mod wheel_tests {
    use super::*;

    #[test]
    fn whole_lines_are_wheel_notches_and_fine_deltas_are_a_trackpad() {
        let lines = |x, y| mouse::ScrollDelta::Lines { x, y };
        assert!(wheel_is_stepped(&lines(0.0, 1.0)));
        assert!(wheel_is_stepped(&lines(0.0, -3.0)));
        assert!(wheel_is_stepped(&lines(1.0, 0.0)));
        assert!(
            !wheel_is_stepped(&lines(0.0, 0.25)),
            "a precision trackpad on Windows"
        );
        assert!(!wheel_is_stepped(&lines(0.0, -1.5)));
        assert!(!wheel_is_stepped(&lines(0.0, 0.0)));
        assert!(!wheel_is_stepped(&mouse::ScrollDelta::Pixels {
            x: 0.0,
            y: 40.0
        }));
    }

    #[test]
    fn zero_wheel_events_are_not_movement() {
        assert!(wheel_is_empty(&mouse::ScrollDelta::Lines {
            x: 0.0,
            y: 0.0
        }));
        assert!(wheel_is_empty(&mouse::ScrollDelta::Pixels {
            x: 0.0,
            y: 0.0
        }));
        assert!(!wheel_is_empty(&mouse::ScrollDelta::Pixels {
            x: 0.0,
            y: 0.5
        }));
    }

    #[test]
    fn pinch_up_zooms_in_and_down_zooms_out() {
        assert!(pinch_zoom_factor(0.05) > 1.0);
        assert!(pinch_zoom_factor(-0.05) < 1.0);
        assert_eq!(pinch_zoom_factor(0.0), 1.0);
        // One update cannot jump the zoom; the same limits apply as for pixel wheel deltas.
        assert_eq!(pinch_zoom_factor(10.0), 1.25);
        assert_eq!(pinch_zoom_factor(-10.0), 0.8);
    }
}
