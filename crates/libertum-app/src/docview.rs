//! The document behind the canvas: layout state, background pagination, the page render
//! worker and the bitmap handle cache.
//!
//! Threading model (Phase 0):
//! - the UI thread never locks the layout state. It reads page hashes from [`PageMap`] and
//!   page bitmaps from the handle cache, both behind short-lived mutexes;
//! - a *paginator* thread paginates the rest of the document in small chunks, taking the
//!   layout write lock per chunk and publishing page hashes to the [`PageMap`];
//! - a *render worker* thread rasterises the pages the UI asked for (closest to the viewport
//!   first). A page bitmap is cut into horizontal **strips** (bands) of under 2 MiB each and
//!   every strip is cached under a hash of what it shows (see [`BandKey`]): after an edit only
//!   the strips whose lines changed or moved are rasterised again, and the rest are reused.
//!   The UI is told with [`DocEvent::PageReady`] and draws the cached handles, so a handle is
//!   created once per strip content and never per frame.
//! - strips stay **under iced_wgpu's 2 MiB synchronous-upload limit**. Larger images are
//!   uploaded on a background thread and draw *nothing* until the upload lands, which flashed
//!   the canvas background after every edit and delayed each keystroke by a frame or more;
//!   a small image is uploaded in the frame that first draws it. While a changed strip is
//!   rendering, the page's previous strips stay visible underneath, so nothing flashes.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use iced::advanced::image::Handle;
use iced::futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use libertum_editor::{DocPosition, Editor, EditorError};
use libertum_layout::spike::{CHARS_PER_PAGE, generate_model_document};
use libertum_layout::{
    CaretRect, LayoutOptions, LayoutState, PageGeometry, ParagraphSnapshot, SelectionRect,
    table_snapshots,
};
use libertum_model::{BlockId, ChangeSet, Document};
use libertum_render::{PageRenderer, page_pixel_size};

use crate::perf;

/// Pages the paginator thread adds per write-lock hold (about 12 ms of work on the dev box):
/// the longest another thread waits for the layout, under one frame.
const PAGINATE_CHUNK: usize = 4;
/// Pages paginated synchronously on open and after a page-setup change, so the first
/// screenful can be drawn before the background pass catches up.
const FIRST_SCREEN_PAGES: usize = 4;
/// A change to more paragraphs than this is not repaginated on the UI thread: the new
/// snapshots go in at once and the background pass rebuilds the pages (about 20 pages' worth).
const BULK_PARAGRAPHS: usize = 200;
/// Pending render requests from the previous frame are kept when they are this many pages
/// from the new visible range, so fast scrolling has a backlog the worker can chew through.
const PENDING_MARGIN: usize = 8;
/// Cap on the merged render queue (bounds how far ahead the worker can run).
const PENDING_MAX: usize = 32;
/// Pages on either side of the viewport whose strip plans are kept. A plan is small, and
/// keeping a margin means scrolling back finds its strips in the handle cache (the plan's
/// keys still match); plans further away are forgotten so the map cannot grow with every
/// page ever visited. Looking up a page with no current plan has to scan the map.
const PLAN_MARGIN: usize = 128;
/// Cap on cached strip bitmaps (bytes of RGBA). A 600 MB working-set ceiling
/// (2026-11-09); this leaves room for the layout caches and iced's GPU atlas.
const HANDLE_CACHE_BYTES: usize = 160 * 1024 * 1024;
/// iced_wgpu uploads an image synchronously (in the frame that draws it) only when it is
/// smaller than this (`MAX_SYNC_SIZE` in `iced_wgpu::image::cache`, 0.14).
const SYNC_UPLOAD_BYTES: u32 = 2 * 1024 * 1024;
/// Upper bound on strip height, device pixels (keeps re-rendering after an edit small).
const MAX_BAND_ROWS: u32 = 256;
/// `BandKey::page` of strips with nothing on them: identical on every page, so shared.
const BLANK_BAND_PAGE: usize = usize::MAX;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A page-count guess for the scroll extent before pagination completes. [`CHARS_PER_PAGE`]
/// is calibrated for A4 with 1-inch margins, so the guess scales with the content area.
fn estimated_pages_for(chars: usize, geometry: &PageGeometry) -> usize {
    let a4 = PageGeometry::a4();
    let reference_area = a4.content_width() * a4.content_height();
    let area = geometry.content_width() * geometry.content_height();
    let ratio = (reference_area / area).clamp(0.25, 4.0);
    ((chars as f32 * ratio / CHARS_PER_PAGE as f32).ceil() as usize).max(1)
}

/// The column a pageless view uses for `document`: its own paper width (A4 for an A4
/// document) and side margins, with no top or bottom margin, so consecutive pages meet
/// without a visible seam.
pub fn pageless_geometry(document: &Document) -> PageGeometry {
    let mut geometry = PageGeometry::from_setup(&document.page_setup());
    geometry.margins.top = 0.0;
    geometry.margins.bottom = 0.0;
    // Very wide side margins must still leave a usable column.
    let side = (geometry.width_pt - 72.0).max(0.0) / 2.0;
    geometry.margins.left = geometry.margins.left.min(side);
    geometry.margins.right = geometry.margins.right.min(side);
    geometry
}

/// Layout options for a document opened in the app. `LIBERTUM_LAYOUT_THREADS=<n>` sets how
/// many threads shape paragraphs ahead of pagination (0 or unset: one per core, up to 4; 1:
/// none), for measuring.
fn layout_options() -> LayoutOptions {
    let threads = std::env::var("LIBERTUM_LAYOUT_THREADS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    LayoutOptions {
        threads,
        ..LayoutOptions::default()
    }
}

/// Events from the worker threads to the UI.
#[derive(Debug, Clone, Copy)]
pub enum DocEvent {
    /// The paginator finished a chunk.
    Pagination { pages: usize, complete: bool },
    /// A page bitmap was added to the cache.
    PageReady,
}

// ---------------------------------------------------------------- page map

/// Page content hashes published by the paginator.
#[derive(Default)]
struct PageMapInner {
    hashes: Vec<u64>,
    complete: bool,
    /// Bumped on every publish. The canvas caches the per-page geometry vectors against it,
    /// so they are rebuilt only when the layout actually changes, not on every frame.
    generation: u64,
}

/// What the UI knows about pagination, without touching the layout lock.
#[derive(Clone, Default)]
pub struct PageMap(Arc<Mutex<PageMapInner>>);

impl PageMap {
    pub fn known_pages(&self) -> usize {
        lock(&self.0).hashes.len()
    }
    pub fn is_complete(&self) -> bool {
        lock(&self.0).complete
    }
    pub fn hash(&self, page: usize) -> Option<u64> {
        lock(&self.0).hashes.get(page).copied()
    }

    /// Changes whenever [`refresh`](Self::refresh) publishes a new layout state.
    pub fn generation(&self) -> u64 {
        lock(&self.0).generation
    }

    /// Recomputes every known page hash from the layout and republishes completeness.
    pub fn refresh(&self, state: &LayoutState) {
        let mut m = lock(&self.0);
        m.hashes.clear();
        m.hashes
            .extend((0..state.page_count()).filter_map(|i| state.page_content_hash(i)));
        m.complete = state.is_complete();
        m.generation = m.generation.wrapping_add(1);
    }
}

// ---------------------------------------------------------------- strips

static ANTIALIAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether page text and shapes are drawn antialiased (the Settings choice). Every page
/// renderer follows it, and it is part of each strip's identity, so a change redraws the
/// pages without clearing anything.
pub fn antialias() -> bool {
    ANTIALIAS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_antialias(on: bool) {
    ANTIALIAS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Rows per strip for a page bitmap `width` pixels wide: as tall as possible below the
/// synchronous-upload limit, capped at [`MAX_BAND_ROWS`].
pub fn band_rows(width: u32) -> u32 {
    let row_bytes = width.max(1).saturating_mul(4);
    ((SYNC_UPLOAD_BYTES - 1) / row_bytes).clamp(1, MAX_BAND_ROWS)
}

/// Identity of one cached strip bitmap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BandKey {
    /// Page index, or [`BLANK_BAND_PAGE`] for an empty strip.
    pub page: usize,
    pub band: u32,
    /// Hash of every page item overlapping the strip, plus its size and scale.
    pub hash: u64,
    pub scale_bits: u32,
}

/// How one page is cut into strips at one scale, for one version of its content.
#[derive(Clone, Debug, PartialEq)]
pub struct BandPlan {
    /// The page content hash this plan was computed for.
    pub page_hash: u64,
    /// Bitmap height in pixels.
    pub height: u32,
    pub rows: u32,
    pub keys: Vec<BandKey>,
}

impl BandPlan {
    /// Strip `i`'s pixel rows.
    fn span(&self, i: usize) -> (u32, u32) {
        let y0 = i as u32 * self.rows;
        (y0, self.rows.min(self.height.saturating_sub(y0)))
    }

    /// Strip `i` as a fraction of the page height.
    fn fraction(&self, i: usize) -> (f32, f32) {
        let (y0, rows) = self.span(i);
        let h = self.height.max(1) as f32;
        (y0 as f32 / h, (y0 + rows) as f32 / h)
    }
}

/// Cuts page `page` into strips at `scale` and keys each strip by the items on it.
pub fn plan_bands(state: &LayoutState, page: usize, scale: f32) -> Option<BandPlan> {
    let page_hash = state.page_content_hash(page)?;
    let (width, height) = page_pixel_size(state, page, scale);
    let rows = band_rows(width);
    let items = state.page_items(page);
    let count = height.div_ceil(rows) as usize;
    let mut keys = Vec::with_capacity(count);
    for i in 0..count {
        let y0 = i as u32 * rows;
        let r = rows.min(height - y0);
        let (top, bottom) = (y0 as f32 / scale, (y0 + r) as f32 / scale);
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (width, r, scale.to_bits(), antialias()).hash(&mut h);
        let mut empty = true;
        for item in items
            .iter()
            .filter(|it| it.bottom >= top && it.top <= bottom)
        {
            item.key.hash(&mut h);
            empty = false;
        }
        keys.push(BandKey {
            page: if empty { BLANK_BAND_PAGE } else { page },
            band: if empty { 0 } else { i as u32 },
            hash: h.finish(),
            scale_bits: scale.to_bits(),
        });
    }
    Some(BandPlan {
        page_hash,
        height,
        rows,
        keys,
    })
}

// ---------------------------------------------------------------- handle cache

struct Entry {
    handle: Handle,
    bytes: usize,
    tick: u64,
}

/// LRU cache of strip image handles (PLAN §4.4 bitmap cache, per strip).
pub struct HandleCache {
    map: HashMap<BandKey, Entry>,
    bytes: usize,
    cap: usize,
    tick: u64,
}

impl HandleCache {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            cap,
            tick: 0,
        }
    }

    pub fn contains(&self, key: &BandKey) -> bool {
        self.map.contains_key(key)
    }

    /// Marks the entry as recently used.
    pub fn get(&mut self, key: &BandKey) -> Option<Handle> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(key).map(|e| {
            e.tick = t;
            e.handle.clone()
        })
    }

    fn insert(&mut self, key: BandKey, handle: Handle, bytes: usize) {
        self.tick += 1;
        if let Some(old) = self.map.insert(
            key,
            Entry {
                handle,
                bytes,
                tick: self.tick,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        while self.bytes > self.cap && self.map.len() > 1 {
            let oldest = self
                .map
                .iter()
                .filter(|(k, _)| **k != key)
                .min_by_key(|(_, e)| e.tick)
                .map(|(k, _)| *k);
            let Some(k) = oldest else { break };
            if let Some(e) = self.map.remove(&k) {
                self.bytes -= e.bytes;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

/// Strip plans per (page, scale bits): the current plan first, then the one it replaced
/// (whose strips stand in while the current one renders).
#[derive(Default)]
struct Plans(HashMap<(usize, u32), Vec<BandPlan>>);

impl Plans {
    fn current(&self, page: usize, scale: f32, page_hash: u64) -> Option<&BandPlan> {
        self.0
            .get(&(page, scale.to_bits()))?
            .first()
            .filter(|p| p.page_hash == page_hash)
    }

    fn set(&mut self, page: usize, scale: f32, plan: BandPlan) {
        let slot = self.0.entry((page, scale.to_bits())).or_default();
        if slot.first() == Some(&plan) {
            return;
        }
        slot.insert(0, plan);
        slot.truncate(2);
    }

    /// Forgets the plans of pages outside `lo..=hi`, at every scale. Their strips stay in
    /// the handle cache and are reused if the worker plans the same content again.
    fn retain_pages(&mut self, lo: usize, hi: usize) {
        self.0.retain(|(page, _), _| (lo..=hi).contains(page));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

// ---------------------------------------------------------------- render queue

#[derive(Default)]
struct Queue {
    /// (page index, device pixels per point), most urgent first.
    wanted: Vec<(usize, f32)>,
    shutdown: bool,
}

#[derive(Default)]
struct RenderQueue {
    inner: Mutex<Queue>,
    wake: Condvar,
}

// ---------------------------------------------------------------- DocView

/// Everything the canvas needs to show a document. Cheap to clone (all `Arc`s).
#[derive(Clone)]
pub struct PageSource {
    pub pages: PageMap,
    cache: Arc<Mutex<HandleCache>>,
    plans: Arc<Mutex<Plans>>,
    queue: Arc<RenderQueue>,
}

/// One strip to draw: its image and its vertical extent as fractions of the page height.
pub struct BandImage {
    pub handle: Handle,
    pub top: f32,
    pub bottom: f32,
}

/// What the canvas draws for one page, bottom layer first.
#[derive(Default)]
pub struct PageBands {
    /// Older strips (previous content, or another scale) shown until the current ones exist.
    pub under: Vec<BandImage>,
    /// Current strips.
    pub over: Vec<BandImage>,
    /// Every current strip is present.
    pub complete: bool,
}

impl PageSource {
    /// The strips to draw for `page` at `scale`.
    pub fn bands(&self, page: usize, scale: f32) -> PageBands {
        let mut out = PageBands::default();
        let Some(hash) = self.pages.hash(page) else {
            return out;
        };
        let plans = lock(&self.plans);
        let mut cache = lock(&self.cache);
        let current = plans.current(page, scale, hash);
        if let Some(plan) = current {
            for (i, key) in plan.keys.iter().enumerate() {
                if let Some(handle) = cache.get(key) {
                    let (top, bottom) = plan.fraction(i);
                    out.over.push(BandImage {
                        handle,
                        top,
                        bottom,
                    });
                }
            }
            out.complete = out.over.len() == plan.keys.len();
        }
        if out.complete {
            return out;
        }
        // Stand-ins: the previous plan at this scale, else the newest plan at another.
        let fallback = plans
            .0
            .get(&(page, scale.to_bits()))
            .and_then(|v| v.iter().find(|p| Some(*p) != current))
            .or_else(|| {
                plans
                    .0
                    .iter()
                    .filter(|((p, s), _)| *p == page && *s != scale.to_bits())
                    .find_map(|(_, v)| v.first())
            });
        if let Some(plan) = fallback {
            for (i, key) in plan.keys.iter().enumerate() {
                if let Some(handle) = cache.get(key) {
                    let (top, bottom) = plan.fraction(i);
                    out.under.push(BandImage {
                        handle,
                        top,
                        bottom,
                    });
                }
            }
        }
        out
    }

    /// True when every strip of the page's current content is cached at `scale`.
    fn has_exact(&self, page: usize, scale: f32) -> bool {
        let Some(hash) = self.pages.hash(page) else {
            return false;
        };
        let plans = lock(&self.plans);
        let Some(plan) = plans.current(page, scale, hash) else {
            return false;
        };
        let cache = lock(&self.cache);
        plan.keys.iter().all(|k| cache.contains(k))
    }

    /// Updates the render worker's to-do list. `pages` is most urgent first. Pages that are
    /// already cached at `scale` are dropped.
    ///
    /// Pages still pending from the previous request are *merged* (within
    /// [`PENDING_MARGIN`] pages of the new request) instead of being thrown away: while the
    /// user scrolls fast the visible range moves every frame, and restarting the queue each
    /// time meant the worker rendered roughly one page per frame and the pages entering the
    /// viewport stayed blank (the white "flicker" of fast scrolling).
    pub fn request(&self, pages: &[usize], scale: f32) {
        if pages.is_empty() {
            return;
        }
        let wanted: Vec<(usize, f32)> = pages
            .iter()
            .copied()
            .filter(|&p| self.pages.hash(p).is_some() && !self.has_exact(p, scale))
            .map(|p| (p, scale))
            .collect();
        let mut q = lock(&self.queue.inner);
        if q.wanted == wanted {
            return;
        }
        let (lo, hi) = pages
            .iter()
            .copied()
            .fold((usize::MAX, 0), |(lo, hi), p| (lo.min(p), hi.max(p)));
        let lo = lo.saturating_sub(PENDING_MARGIN);
        let hi = hi.saturating_add(PENDING_MARGIN);
        // Drop plans for pages far from the viewport. Done before the queue is updated, so
        // the map never holds every page ever visited.
        lock(&self.plans).retain_pages(
            lo.saturating_sub(PLAN_MARGIN),
            hi.saturating_add(PLAN_MARGIN),
        );
        let mut merged = wanted;
        for &(page, s) in &q.wanted {
            if merged.len() >= PENDING_MAX {
                break;
            }
            if s == scale && (lo..=hi).contains(&page) && !merged.iter().any(|&(p, _)| p == page) {
                merged.push((page, s));
            }
        }
        q.wanted = merged;
        self.queue.wake.notify_one();
    }

    /// (cached strips, cached bytes), for logging.
    pub fn cache_stats(&self) -> (usize, usize) {
        let c = lock(&self.cache);
        (c.len(), c.bytes())
    }
}

/// Hands out [`DocView::id`]s: never reused within a run.
static NEXT_DOC_ID: AtomicU64 = AtomicU64::new(1);

/// The open document: the model, the editor, paginated layout and the render pipeline.
pub struct DocView {
    /// Identifies this open document to MCP clients; a different document gets a different id.
    id: u64,
    pub source: PageSource,
    /// The page geometry currently laid out, derived from the document's page setup.
    pub geometry: PageGeometry,
    /// Pageless view: one endless column, drawn without page gaps or shadows.
    pub pageless: bool,
    /// Overrides the document's page setup while `continuous` is on.
    fixed_geometry: Option<PageGeometry>,
    word_count: usize,
    /// Characters in the body, for the page count estimate when the geometry changes.
    chars: usize,
    /// Page count guess used for the scroll extent until pagination completes.
    pub estimated_pages: usize,
    queue: Arc<RenderQueue>,
    /// The model document and editor state. Owned by the UI thread.
    editor: Editor,
    /// Shared with the paginator and render worker.
    layout: Arc<SharedLayout>,
    /// Where worker threads report; kept so a page-setup change can restart the paginator.
    events: UnboundedSender<DocEvent>,
    /// Bumped to retire a running paginator before a new one starts.
    paginate_epoch: Arc<AtomicU64>,
    /// Bumped whenever a paragraph style changes. Folded into snapshot versions, so layouts
    /// cached under a paragraph's old formatting are never reused (PLAN §4.2 `style_epoch`).
    style_epoch: u64,
    /// Renders page thumbnails on the UI thread (a few at a time, only those in view).
    thumbs: libertum_render::PageRenderer,
    /// Per-page geometry and used heights, shared with the canvas and rebuilt only when the
    /// page map publishes a new layout state (see [`PageMap::generation`]). Without this the
    /// whole document's geometry was rebuilt several times per scroll event.
    page_views: Mutex<Option<PageViews>>,
}

struct PageViews {
    generation: u64,
    pages: usize,
    geometries: Arc<[PageGeometry]>,
    used: Arc<[f32]>,
}

/// Bits of a snapshot version above the paragraph's own edit count that hold the style
/// epoch (a paragraph would need 2^40 edits to reach them).
const STYLE_EPOCH_SHIFT: u32 = 40;

impl Drop for DocView {
    fn drop(&mut self) {
        lock(&self.queue.inner).shutdown = true;
        self.queue.wake.notify_all();
    }
}

impl DocView {
    /// Builds the 500-page demo document, paginates the first screenful synchronously and
    /// starts the background threads. Returns the receiver for [`DocEvent`]s.
    /// The generated demo document: 500 pages, or `LIBERTUM_DEMO_PAGES` (1 to 5,000; the
    /// Phase 1 acceptance run uses 1,000). `LIBERTUM_HEADERS=1` gives it a running header and
    /// footer, for screenshots of the header and footer feature.
    pub fn open_demo() -> (Self, UnboundedReceiver<DocEvent>) {
        let pages = std::env::var("LIBERTUM_DEMO_PAGES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|p| (1..=5_000).contains(p))
            .unwrap_or(500);
        let mut document = generate_model_document(pages, 42);
        if std::env::var("LIBERTUM_HEADERS").is_ok_and(|v| v == "1")
            && let Some(section) = document.sections.first_mut()
        {
            use libertum_model::{HeaderFooter, HeadersFooters};
            section.headers = HeadersFooters {
                default: Some(HeaderFooter::new("The Libertum Demo \u{2014} {page}")),
                first: Some(HeaderFooter::new("The Libertum Demo \u{2014} first page")),
                even: Some(HeaderFooter::new("The Libertum Demo \u{2014} even page")),
                different_first: true,
                different_even_odd: true,
            };
            section.footers = HeadersFooters::with_default("{page} of {pages}");
        }
        Self::open(document)
    }

    /// Opens `document` on the page setup its sections carry, with the default layout options.
    pub fn open(document: Document) -> (Self, UnboundedReceiver<DocEvent>) {
        Self::open_with(document, layout_options())
    }

    /// Opens `document` with layout options (from a template). The page geometry comes from
    /// the document's page setup (section 0; every section shares it for now).
    pub fn open_with(
        document: Document,
        options: LayoutOptions,
    ) -> (Self, UnboundedReceiver<DocEvent>) {
        Self::open_with_mode(document, options, None)
    }

    /// Opens a Markdown document in the pageless view: a reading-width column, drawn without
    /// page gaps or shadows by the canvas.
    pub fn open_markdown(document: Document) -> (Self, UnboundedReceiver<DocEvent>) {
        let geometry = PageGeometry {
            width_pt: 612.0,
            height_pt: 792.0,
            margins: libertum_layout::Margins {
                top: 0.0,
                right: 120.0,
                bottom: 0.0,
                left: 120.0,
            },
        };
        Self::open_with_mode(document, layout_options(), Some(geometry))
    }

    /// Opens a document in the pageless view: one endless column as wide as its own paper
    /// (A4 for an A4 document), with its side margins and no top or bottom margin, so the
    /// pages read as one long sheet.
    pub fn open_pageless(document: Document) -> (Self, UnboundedReceiver<DocEvent>) {
        let geometry = pageless_geometry(&document);
        Self::open_with_mode(document, layout_options(), Some(geometry))
    }

    fn open_with_mode(
        document: Document,
        options: LayoutOptions,
        fixed_geometry: Option<PageGeometry>,
    ) -> (Self, UnboundedReceiver<DocEvent>) {
        let started = Instant::now();
        let geometry =
            fixed_geometry.unwrap_or_else(|| PageGeometry::from_setup(&document.page_setup()));
        let chars: usize = document
            .paragraphs()
            .map(|p| p.text().chars().count())
            .sum();
        let word_count = document.word_count();
        let estimated_pages = estimated_pages_for(chars, &geometry);

        let mut state = LayoutState::from_document(&document, geometry, options);
        if fixed_geometry.is_some() {
            // A reading view owns its geometry; the sections' page margins must not push the
            // body back in (their page setup is still the document's own for export).
            state.set_ignore_section_margins(true);
            // One endless column: no page numbers, headers or footers between the pages.
            state.set_pageless(true);
        }
        // First screenful: enough pages to fill a tall window at 50% zoom.
        state.paginate_until(FIRST_SCREEN_PAGES);
        tracing::info!(
            first_pages = state.page_count(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            words = word_count,
            "document opened; first screenful paginated"
        );

        let page_map = PageMap::default();
        {
            let mut m = lock(&page_map.0);
            m.hashes = (0..state.page_count())
                .filter_map(|i| state.page_content_hash(i))
                .collect();
            m.complete = state.is_complete();
        }
        let layout = Arc::new(SharedLayout::new(state));
        let cache = Arc::new(Mutex::new(HandleCache::new(HANDLE_CACHE_BYTES)));
        let plans = Arc::new(Mutex::new(Plans::default()));
        let queue = Arc::new(RenderQueue::default());
        let paginate_epoch = Arc::new(AtomicU64::new(0));
        let (tx, rx) = unbounded();

        spawn_paginator(
            layout.clone(),
            page_map.clone(),
            tx.clone(),
            paginate_epoch.clone(),
        );
        spawn_render_worker(
            layout.clone(),
            page_map.clone(),
            cache.clone(),
            plans.clone(),
            queue.clone(),
            tx.clone(),
        );

        let source = PageSource {
            pages: page_map,
            cache,
            plans: plans.clone(),
            queue: queue.clone(),
        };
        (
            Self {
                id: NEXT_DOC_ID.fetch_add(1, Ordering::Relaxed),
                source,
                geometry,
                pageless: fixed_geometry.is_some(),
                fixed_geometry,
                word_count,
                chars,
                estimated_pages,
                queue,
                editor: Editor::new(document),
                layout,
                events: tx,
                paginate_epoch,
                style_epoch: 0,
                thumbs: libertum_render::PageRenderer::new(0),
                page_views: Mutex::new(None),
            },
            rx,
        )
    }

    /// Pages to lay the scroll area out for: the real count once pagination is complete,
    /// otherwise the larger of the pages found so far and the estimate.
    pub fn page_total(&self) -> usize {
        let known = self.source.pages.known_pages();
        if self.source.pages.is_complete() {
            known.max(1)
        } else {
            known.max(self.estimated_pages)
        }
    }

    /// One geometry per page for the canvas. Pages not laid out yet use section 0's setup.
    pub fn page_geometries(&self) -> Arc<[PageGeometry]> {
        self.page_views().0
    }

    /// How much of each page is used, in points (the canvas stacks pageless pages by this so
    /// no blank page tail opens between the lines of a split paragraph or table).
    pub fn page_used_heights(&self) -> Arc<[f32]> {
        self.page_views().1
    }

    /// Both per-page vectors, rebuilt when the page map publishes a new generation and shared
    /// from then on. Clones are cheap `Arc` clones, so a draw or a scroll event does not
    /// allocate the whole document's geometry.
    fn page_views(&self) -> (Arc<[PageGeometry]>, Arc<[f32]>) {
        let generation = self.source.pages.generation();
        let total = self.page_total();
        let mut slot = self
            .page_views
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(views) = slot.as_ref()
            && views.generation == generation
            && views.pages == total
        {
            return (views.geometries.clone(), views.used.clone());
        }
        let layout = self
            .layout
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let geometries: Arc<[PageGeometry]> = (0..total)
            .map(|page| layout.page_geometry(page))
            .collect::<Vec<_>>()
            .into();
        let used: Arc<[f32]> = (0..total)
            .map(|page| layout.page_used_height(page))
            .collect::<Vec<_>>()
            .into();
        drop(layout);
        *slot = Some(PageViews {
            generation,
            pages: total,
            geometries: geometries.clone(),
            used: used.clone(),
        });
        (geometries, used)
    }

    /// Hash of what page `page` shows, once it is paginated (thumbnail cache key).
    pub fn page_content_hash(&self, page: usize) -> Option<u64> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout.page_content_hash(page)
    }

    /// Page `page` rendered `width_px` device pixels wide, as straight RGBA (the page is
    /// opaque, so premultiplied and straight are the same). `None` until it is paginated.
    pub fn render_thumbnail(&mut self, page: usize, width_px: u32) -> Option<(u32, u32, Vec<u8>)> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        if page >= layout.page_count() {
            return None;
        }
        let geometry = layout.page_geometry(page);
        let scale = width_px.max(8) as f32 / geometry.width_pt.max(1.0);
        self.thumbs.set_antialias(antialias());
        let pixmap = self.thumbs.render_page(&layout, page, scale)?;
        Some((pixmap.width(), pixmap.height(), pixmap.data().to_vec()))
    }

    /// The displayed page number of a block, once pagination has reached it.
    /// 0-based index of the physical page the caret is on, once that page is paginated.
    /// "Use Current Page" for page numbering starts from this, not from the viewport middle.
    pub fn caret_page_index(&self) -> Option<usize> {
        let block = self.editor.caret().block;
        let index = self.document().index_of(block)?;
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        (index < layout.paragraph_count()).then(|| layout.page_of_para(index))
    }

    pub fn page_number_of_block(&self, block: libertum_model::BlockId) -> Option<u32> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout.page_number_of_block(block.0)
    }

    /// Calls back into the page map once the background pass has finished: `{pages}` fields
    /// (and the total in the page-number pill) are only final then, so pages whose furniture
    /// shows them are repainted with the exact count.
    pub fn pagination_finished(&mut self) {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        self.source.pages.refresh(&layout);
    }

    /// Read access to the editor (selection, formatting summary). Changes go through
    /// [`edit`](Self::edit) so layout stays in sync.
    /// This open document's id for MCP clients.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The model document, for saving.
    pub fn document(&self) -> &Document {
        self.editor.document()
    }

    /// Turns the pageless view on or off. In the pageless view the document is one endless
    /// column as wide as its own paper; off, it is pages as the document's page setup says.
    ///
    /// The column width changes, so this is a whole-document action like a page-setup change:
    /// the first screenful is laid out here and the rest in the background.
    pub fn set_pageless(&mut self, on: bool) {
        if on == self.pageless {
            return;
        }
        self.pageless = on;
        self.fixed_geometry = if on {
            Some(pageless_geometry(self.document()))
        } else {
            None
        };
        let geometry = self
            .fixed_geometry
            .unwrap_or_else(|| PageGeometry::from_setup(&self.document().page_setup()));
        {
            let mut layout = self.layout.write().unwrap_or_else(|e| e.into_inner());
            // Order matters: dropping the section margins first, then the furniture, then the
            // geometry, so nothing re-reserves room for a header the view no longer draws.
            layout.set_ignore_section_margins(on);
            layout.set_pageless(on);
            layout.set_geometry(geometry);
            layout.paginate_until(FIRST_SCREEN_PAGES);
            self.source.pages.refresh(&layout);
        }
        self.geometry = geometry;
        self.estimated_pages = estimated_pages_for(self.chars, &geometry);
        self.restart_paginator();
    }

    /// Paginates the whole document now (tests: the app does it on a worker).
    #[cfg(test)]
    pub fn paginate_everything(&mut self) {
        let mut layout = self.layout.write().unwrap_or_else(|e| e.into_inner());
        layout.paginate_all();
        // Publish the result to the page map, as the background paginator does, so that
        // `page_total()` is exact and cannot race that thread (it was an intermittent failure).
        self.source.pages.refresh(&layout);
    }

    /// List markers drawn on page `page` (tests).
    #[cfg(test)]
    pub fn markers_on(&self, page: usize) -> Vec<String> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout
            .page_decorations(page)
            .into_iter()
            .filter_map(|d| match d {
                libertum_layout::Decoration::Text { text, .. } => Some(text),
                libertum_layout::Decoration::Leader { .. }
                | libertum_layout::Decoration::Rect { .. }
                | libertum_layout::Decoration::Image { .. } => None,
            })
            .collect()
    }

    /// The font families installed on this machine, from the layout engine's collection.
    pub fn font_families(&self) -> Vec<std::sync::Arc<str>> {
        self.layout
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .font_families()
    }

    pub fn word_count(&self) -> usize {
        self.word_count
    }

    /// Runs an editor command and synchronises layout, page hashes and word count.
    pub fn edit(
        &mut self,
        command: impl FnOnce(&mut Editor) -> Result<ChangeSet, EditorError>,
    ) -> Result<(), EditorError> {
        let started = Instant::now();
        let change = command(&mut self.editor)?;
        self.sync(change);
        perf::edit_synced(started);
        Ok(())
    }

    /// Applies a change set to the layout incrementally. Text-only edits replace the dirty
    /// paragraphs; structural edits reconcile the paragraph list with the model in order.
    fn sync(&mut self, change: ChangeSet) {
        if change.is_empty() {
            return;
        }
        if !change.styles.is_empty() || change.all_styles {
            self.style_epoch += 1;
        }
        let epoch = self.style_epoch;
        let doc = self.editor.document();
        let snap = |p: &libertum_model::Paragraph| {
            let mut s = ParagraphSnapshot::from_model(p, &doc.styles);
            s.version = s.version.wrapping_add(epoch << STYLE_EPOCH_SHIFT);
            s
        };
        let mut relayout = false;
        {
            let mut layout = self.layout.write().unwrap_or_else(|e| e.into_inner());
            // Structural edits can renumber lists after them.
            let mut relabel = change.structural;
            if change.structural {
                // Drop blocks the document no longer has, from the back.
                let live: std::collections::HashSet<u64> =
                    doc.paragraph_ids().map(|id| id.0).collect();
                let mut i = layout.paragraph_count();
                while i > 0 {
                    i -= 1;
                    let id = layout.paragraphs()[i].id;
                    if !live.contains(&id) {
                        layout.remove_paragraph(i);
                    }
                }
                // Walk the document in order (table cell paragraphs included): replace dirty
                // paragraphs, insert new blocks.
                for (index, p) in doc.paragraphs().enumerate() {
                    let mut snapshot = snap(p);
                    if let Some(old) = layout.paragraphs().get(index)
                        && old.id == snapshot.id
                    {
                        relabel |= old.list != snapshot.list;
                        snapshot.list_label = old.list_label.clone();
                    }
                    let same = layout.paragraphs().get(index).map(|s| s.id) == Some(snapshot.id);
                    if same {
                        if change.dirty.contains(&p.id) {
                            layout.replace_paragraph(index, snapshot);
                        }
                    } else {
                        layout.insert_paragraph(index, snapshot);
                    }
                }
                // A table's structure may have changed (insert, delete, replace). Rebuilding
                // the snapshots is O(paragraphs) and only happens on structural edits.
                let tables = table_snapshots(doc);
                if tables != layout.tables() {
                    layout.set_tables(tables);
                    relayout = true;
                }
            } else if change.dirty.len() > BULK_PARAGRAPHS {
                // Select All then Bold, and the like: swap every snapshot in and leave the
                // pagination to the background pass instead of repaginating once per paragraph
                // on this thread (1.6 s at 500 pages, measured).
                let mut items = Vec::with_capacity(change.dirty.len());
                for id in &change.dirty {
                    if let (Some(index), Some(p)) = (doc.index_of(*id), doc.paragraph(*id)) {
                        let mut snapshot = snap(p);
                        if let Some(old) = layout.paragraphs().get(index) {
                            relabel |= old.list != snapshot.list;
                            snapshot.list_label = old.list_label.clone();
                        }
                        items.push((index, snapshot));
                    }
                }
                relayout |= layout.replace_paragraphs_deferred(items);
            } else {
                for id in &change.dirty {
                    if let (Some(index), Some(p)) = (doc.index_of(*id), doc.paragraph(*id)) {
                        let mut snapshot = snap(p);
                        if let Some(old) = layout.paragraphs().get(index) {
                            // Typing in a list item keeps its number; a list change renumbers.
                            relabel |= old.list != snapshot.list;
                            snapshot.list_label = old.list_label.clone();
                        }
                        layout.replace_paragraph(index, snapshot);
                    }
                }
            }
            if change.all_styles {
                // The document defaults changed: every paragraph re-resolves.
                let all: Vec<(usize, ParagraphSnapshot)> = doc
                    .paragraphs()
                    .enumerate()
                    .map(|(i, p)| {
                        let mut s = snap(p);
                        s.list_label = layout
                            .paragraphs()
                            .get(i)
                            .and_then(|old| old.list_label.clone());
                        (i, s)
                    })
                    .collect();
                relayout |= layout.replace_paragraphs_deferred(all);
            } else if !change.styles.is_empty() {
                // Every paragraph using a changed style (or one based on it) re-resolves.
                // O(paragraphs), but only for an explicit style change, never for typing.
                let styles = &doc.styles;
                let restyled: Vec<(usize, ParagraphSnapshot)> = doc
                    .paragraphs()
                    .enumerate()
                    .filter(|(_, p)| {
                        change
                            .styles
                            .iter()
                            .any(|s| styles.inherits_from(p.props.style, *s))
                    })
                    .map(|(i, p)| (i, snap(p)))
                    .collect();
                if restyled.len() > BULK_PARAGRAPHS {
                    relayout |= layout.replace_paragraphs_deferred(restyled);
                } else {
                    layout.replace_paragraphs(restyled);
                }
            }
            if relabel {
                // One O(paragraphs) numbering pass, only after a structural or list change.
                let labels = doc.list_labels();
                let changed: Vec<(usize, ParagraphSnapshot)> = layout
                    .paragraphs()
                    .iter()
                    .zip(labels)
                    .enumerate()
                    .filter(|(_, (s, label))| s.list_label != *label)
                    .map(|(i, (s, label))| {
                        let mut s = s.clone();
                        s.list_label = label;
                        (i, s)
                    })
                    .collect();
                if !changed.is_empty() {
                    layout.replace_paragraphs(changed);
                }
            }
            if change.all_styles {
                // The document defaults changed: header and footer text follows them.
                let style = doc.styles.defaults.clone();
                relayout |= layout.set_furniture_style(&style);
            }
            if change.sections {
                // The model owns page setup now; a change rebuilds pagination from scratch
                // (explicit action, so O(document) is allowed) and the first screenful is
                // laid out here so the canvas has something to draw straight away.
                let geometry = self
                    .fixed_geometry
                    .unwrap_or_else(|| PageGeometry::from_setup(&doc.page_setup()));
                if geometry != self.geometry {
                    layout.set_geometry(geometry);
                    self.geometry = geometry;
                    self.estimated_pages = estimated_pages_for(self.chars, &geometry);
                    relayout = true;
                }
                // A header or footer taller than the margins re-paginates too.
                relayout |= layout.set_sections(doc.sections.clone());
                // Whole-document page numbering: only numbers and decorations change.
                layout.set_numbering(doc.page_numbering());
            }
            if relayout {
                // The first screenful is laid out here so the canvas has something to draw
                // straight away; the background pass catches up.
                layout.paginate_until(FIRST_SCREEN_PAGES);
            }
            self.source.pages.refresh(&layout);
        }
        if relayout {
            self.restart_paginator();
        }
        self.word_count = doc.word_count();
    }

    /// Retires the running paginator and starts a fresh one on the current layout state. A
    /// page-setup change drops every page, so the background pass must run again from page 0.
    fn restart_paginator(&self) {
        self.paginate_epoch.fetch_add(1, Ordering::AcqRel);
        spawn_paginator(
            self.layout.clone(),
            self.source.pages.clone(),
            self.events.clone(),
            self.paginate_epoch.clone(),
        );
    }

    pub fn undo(&mut self) -> Result<bool, EditorError> {
        match self.editor.undo()? {
            Some(change) => {
                self.sync(change);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn redo(&mut self) -> Result<bool, EditorError> {
        match self.editor.redo()? {
            Some(change) => {
                self.sync(change);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Plain-text copy of the selection.
    pub fn copy(&self) -> Option<String> {
        self.editor.copy()
    }

    /// Plain-text cut of the selection.
    pub fn cut(&mut self) -> Result<Option<String>, EditorError> {
        let Some(text) = self.editor.copy() else {
            return Ok(None);
        };
        let change = self.editor.delete_forward()?;
        self.sync(change);
        Ok(Some(text))
    }

    /// Maps a page point to the model block under it.
    pub fn hit_test(&self, page: usize, x_pt: f32, y_pt: f32) -> Option<(BlockId, usize)> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        let hit = layout.hit_test(page, x_pt, y_pt)?;
        let id = layout.paragraphs().get(hit.para)?.id;
        Some((BlockId(id), hit.byte_offset))
    }

    /// The rectangles of the pictures drawn on `page`, in page points, in drawing order
    /// (tests aim the pointer with these).
    #[cfg(test)]
    pub fn image_rects(&self, page: usize) -> Vec<(f32, f32, f32, f32)> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout
            .page_decorations(page)
            .into_iter()
            .filter_map(|decoration| match decoration {
                libertum_layout::Decoration::Image {
                    x,
                    y,
                    width,
                    height,
                    ..
                } => Some((x, y, width, height)),
                _ => None,
            })
            .collect()
    }

    /// The paragraphs that have a line on `pages`, plus the caret's paragraph. Spelling
    /// checks these only, so a 500-page document never pays for the whole text on a keystroke.
    pub fn visible_paragraphs(&self, pages: std::ops::Range<usize>) -> Vec<(BlockId, String)> {
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut push = |index: usize, seen: &mut std::collections::HashSet<usize>| {
            if seen.insert(index)
                && let Some(snapshot) = layout.paragraphs().get(index)
            {
                out.push((BlockId(snapshot.id), snapshot.text.clone()));
            }
        };
        for page in pages {
            let Some(page) = layout.pages().get(page) else {
                continue;
            };
            push(page.start.para, &mut seen);
            for slice in &page.slices {
                push(slice.para, &mut seen);
            }
        }
        let caret = self.editor.caret().block;
        if let Some(index) = self.editor.document().index_of(caret) {
            push(index, &mut seen);
        }
        out
    }

    /// The line rectangles covered by a byte range in one paragraph, for overlays such as
    /// spelling underlines.
    pub fn range_rects(&self, block: BlockId, start: usize, end: usize) -> Vec<SelectionRect> {
        let Some(index) = self.editor.document().index_of(block) else {
            return Vec::new();
        };
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout.selection_rects(index, start..end)
    }

    /// The image under a page point, with the paragraph that holds it and its index there:
    /// for the context menu, so rights-clicking a picture acts on that picture rather than
    /// on the first one in the paragraph.
    pub fn image_at(&self, page: usize, x_pt: f32, y_pt: f32) -> Option<(BlockId, usize)> {
        let (block, _) = self.hit_test(page, x_pt, y_pt)?;
        let paragraph = self.editor.document().paragraph(block)?;
        if paragraph.images().is_empty() {
            return None;
        }
        let hash = {
            let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
            layout
                .page_decorations(page)
                .into_iter()
                .find_map(|decoration| match decoration {
                    libertum_layout::Decoration::Image {
                        media,
                        x,
                        y,
                        width,
                        height,
                    } if x_pt >= x && x_pt <= x + width && y_pt >= y && y_pt <= y + height => {
                        Some(media.hash())
                    }
                    _ => None,
                })
        };
        // Match the decoration back to the paragraph's images by image bytes. Two uses of the
        // same image in one paragraph pick the first, which is the best the model can say.
        let index = hash.and_then(|hash| {
            paragraph
                .images()
                .iter()
                .position(|image| image.media.hash() == hash)
        })?;
        Some((block, index))
    }

    /// Caret rectangle in page coordinates, for the canvas overlay.
    pub fn caret_rect(&self) -> Option<CaretRect> {
        let caret = self.editor.caret();
        let index = self.editor.document().index_of(caret.block)?;
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout.caret_rect(index, caret.offset, 1.0)
    }

    /// The line box the caret is on, in page coordinates (see `LayoutState::line_rect`).
    fn caret_line_rect(&self) -> Option<CaretRect> {
        let caret = self.editor.caret();
        let index = self.editor.document().index_of(caret.block)?;
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        layout.line_rect(index, caret.offset)
    }

    /// Selection highlight rectangles in page coordinates, one entry per visual line.
    pub fn selection_rects(&self) -> Vec<SelectionRect> {
        self.selection_rects_within(None)
    }

    /// The highlight rectangles of the part of the selection on `pages` only. The canvas
    /// asks for this every frame; with the whole of a long document selected, walking every
    /// selected paragraph took a layout per paragraph each time (and starved the paginator).
    pub fn selection_rects_on(&self, pages: std::ops::Range<usize>) -> Vec<SelectionRect> {
        self.selection_rects_within(Some(pages))
    }

    fn selection_rects_within(&self, pages: Option<std::ops::Range<usize>>) -> Vec<SelectionRect> {
        let Some((start, end)) = self.editor.ordered_selection() else {
            return Vec::new();
        };
        if start == end {
            return Vec::new();
        }
        let doc = self.editor.document();
        let (Some(si), Some(ei)) = (doc.index_of(start.block), doc.index_of(end.block)) else {
            return Vec::new();
        };
        let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
        let (mut first, mut last) = (si, ei);
        if let Some(pages) = pages {
            // Only the paragraphs that have a line on these pages (paginated so far).
            let laid_out = layout.pages();
            let (from, to) = (
                pages.start.min(laid_out.len()),
                pages.end.min(laid_out.len()),
            );
            if from >= to {
                return Vec::new();
            }
            let lowest = laid_out[from]
                .slices
                .iter()
                .map(|slice| slice.para)
                .chain(std::iter::once(laid_out[from].start.para))
                .min()
                .unwrap_or(0);
            let highest = laid_out[to - 1]
                .slices
                .iter()
                .map(|slice| slice.para)
                .chain(std::iter::once(laid_out[to - 1].start.para))
                .max()
                .unwrap_or(0);
            first = first.max(lowest);
            last = last.min(highest);
        }
        let mut out = Vec::new();
        for i in first..=last {
            let Some(snapshot) = layout.paragraphs().get(i) else {
                break;
            };
            let from = if i == si { start.offset } else { 0 };
            let to = if i == ei {
                end.offset
            } else {
                snapshot.text.len()
            };
            out.extend(layout.selection_rects(i, from..to));
        }
        out
    }

    /// Places the caret (or extends the selection) from a click in page coordinates.
    pub fn set_caret_from_pointer(
        &mut self,
        page: usize,
        x_pt: f32,
        y_pt: f32,
        extend: bool,
    ) -> bool {
        let Some((block, offset)) = self.hit_test(page, x_pt, y_pt) else {
            return false;
        };
        self.editor.set_caret(DocPosition { block, offset }, extend);
        true
    }

    /// Selects the word under a page point (double-click) and places the caret at its end.
    pub fn select_word_from_pointer(&mut self, page: usize, x_pt: f32, y_pt: f32) -> bool {
        let Some((block, offset)) = self.hit_test(page, x_pt, y_pt) else {
            return false;
        };
        let Some(text) = self.editor.document().paragraph(block).map(|p| p.text()) else {
            return false;
        };
        // A click just past the end of a word must still select that word.
        let start = libertum_editor::prev_word_boundary(text, offset);
        let end = libertum_editor::next_word_boundary(text, start);
        if start == end {
            self.editor.set_caret(DocPosition { block, offset }, false);
            return true;
        }
        self.editor.set_selection(
            DocPosition {
                block,
                offset: start,
            },
            DocPosition { block, offset: end },
        );
        true
    }

    /// Selects the paragraph under a page point (triple-click).
    pub fn select_paragraph_from_pointer(&mut self, page: usize, x_pt: f32, y_pt: f32) -> bool {
        let Some((block, _)) = self.hit_test(page, x_pt, y_pt) else {
            return false;
        };
        let Some(len) = self.editor.document().paragraph(block).map(|p| p.len()) else {
            return false;
        };
        self.editor.set_selection(
            DocPosition { block, offset: 0 },
            DocPosition { block, offset: len },
        );
        true
    }

    /// Runs a movement command (no layout change, so no re-pagination).
    pub fn move_caret(&mut self, command: impl FnOnce(&mut Editor)) {
        command(&mut self.editor);
    }

    pub fn select_all(&mut self) {
        self.editor.select_all();
    }

    /// Moves the caret one visual line up (`direction < 0`) or down, using the layout to
    /// decide where the previous/next line starts. Crosses page boundaries.
    pub fn move_vertical(&mut self, direction: i32, extend: bool) -> bool {
        // The whole line box, not the caret: the caret is only as tall as the text, and a
        // step that short can stop in the space between paragraphs and stay on the line.
        let Some(caret) = self.caret_line_rect() else {
            return false;
        };
        // The laid-out text area: header and footer insets may shrink it from the page setup.
        let (top, bottom) = {
            let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
            let geo = layout.geometry();
            (geo.margins.top, geo.height_pt - geo.margins.bottom)
        };
        let line = caret.height.max(1.0);
        let last_page = self.page_total().saturating_sub(1);
        let mut page = caret.page;
        // Keep the column across short lines: aim for the remembered x, not the caret's.
        let x = self.editor.preferred_x().unwrap_or(caret.x);
        // Aim for the middle of the neighbouring line on this page, wherever it is (the
        // space between paragraphs can be wider than a line); off the page, the next one.
        let neighbour = {
            let layout = self.layout.read().unwrap_or_else(|e| e.into_inner());
            layout.neighbour_line_y(page, x, caret.y, caret.y + line, direction)
        };
        if let Some(y) = neighbour {
            let Some((block, offset)) = self.hit_test(page, x, y) else {
                return false;
            };
            self.editor
                .set_caret_vertical(DocPosition { block, offset }, extend, x);
            return true;
        }
        let mut y = if direction > 0 { bottom } else { top - 1.0 };
        if y < top {
            if page > 0 {
                page -= 1;
                y = bottom - line * 0.5;
            } else {
                y = top + line * 0.5;
            }
        } else if y + line > bottom {
            if page < last_page {
                page += 1;
                y = top + line * 0.5;
            } else {
                y = bottom - line;
            }
        }
        let Some((block, offset)) = self.hit_test(page, x, y) else {
            return false;
        };
        self.editor
            .set_caret_vertical(DocPosition { block, offset }, extend, x);
        true
    }
}

/// The layout behind its lock, with a count of the threads waiting for it.
///
/// The background paginator takes the write lock for one chunk of pages after another. The
/// system lock is not fair: a thread that releases it and asks again straight away usually
/// gets it back, so the UI thread and the page renderer could wait for the whole document
/// to paginate (over a second on a few hundred pages) before reading a caret position or
/// drawing a page. Everyone but the paginator announces that it is waiting, and the
/// paginator does not start its next chunk while anyone is: the longest wait is one chunk.
pub(crate) struct SharedLayout {
    lock: RwLock<LayoutState>,
    waiting: std::sync::atomic::AtomicUsize,
}

impl SharedLayout {
    fn new(state: LayoutState) -> Self {
        Self {
            lock: RwLock::new(state),
            waiting: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn read(&self) -> std::sync::LockResult<std::sync::RwLockReadGuard<'_, LayoutState>> {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let guard = self.lock.read();
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        guard
    }

    fn write(&self) -> std::sync::LockResult<std::sync::RwLockWriteGuard<'_, LayoutState>> {
        self.waiting.fetch_add(1, Ordering::AcqRel);
        let guard = self.lock.write();
        self.waiting.fetch_sub(1, Ordering::AcqRel);
        guard
    }

    /// The write lock for the background paginator: taken only once nobody else is
    /// waiting for the layout.
    fn write_in_background(
        &self,
    ) -> std::sync::LockResult<std::sync::RwLockWriteGuard<'_, LayoutState>> {
        while self.waiting.load(Ordering::Acquire) > 0 {
            std::thread::yield_now();
        }
        self.lock.write()
    }
}

fn spawn_paginator(
    layout: Arc<SharedLayout>,
    map: PageMap,
    tx: UnboundedSender<DocEvent>,
    epoch: Arc<AtomicU64>,
) {
    let mine = epoch.load(Ordering::Acquire);
    let spawned = std::thread::Builder::new()
        .name("libertum-paginate".into())
        .spawn(move || {
            let started = Instant::now();
            // Time spent waiting for readers (the UI, the page renderer) and doing the work,
            // for the completion log: the gap between them and `elapsed_ms` is the rest.
            let mut waited = Duration::ZERO;
            let mut worked = Duration::ZERO;
            loop {
                // A page-setup change bumped the epoch: this thread's document is gone.
                if epoch.load(Ordering::Acquire) != mine {
                    return;
                }
                let (pages, complete, built) = {
                    let asked = Instant::now();
                    let mut st = layout
                        .write_in_background()
                        .unwrap_or_else(|e| e.into_inner());
                    waited += asked.elapsed();
                    let begun = Instant::now();
                    let have = st.page_count();
                    if !st.is_complete() {
                        st.paginate_until(have + PAGINATE_CHUNK);
                    }
                    worked += begun.elapsed();
                    let mut m = lock(&map.0);
                    for i in m.hashes.len()..st.page_count() {
                        if let Some(h) = st.page_content_hash(i) {
                            m.hashes.push(h);
                        }
                    }
                    m.complete = st.is_complete();
                    (m.hashes.len(), m.complete, st.engine_stats().layouts_built)
                };
                if tx
                    .unbounded_send(DocEvent::Pagination { pages, complete })
                    .is_err()
                {
                    return; // UI gone
                }
                if complete {
                    tracing::info!(
                        pages,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        waited_ms = waited.as_millis() as u64,
                        worked_ms = worked.as_millis() as u64,
                        layouts_built = built,
                        "background pagination complete"
                    );
                    return;
                }
                // The next chunk waits in `write_in_background` for anyone who wants the
                // layout first.
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start the pagination thread");
    }
}

fn spawn_render_worker(
    layout: Arc<SharedLayout>,
    map: PageMap,
    cache: Arc<Mutex<HandleCache>>,
    plans: Arc<Mutex<Plans>>,
    queue: Arc<RenderQueue>,
    tx: UnboundedSender<DocEvent>,
) {
    let spawned = std::thread::Builder::new()
        .name("libertum-render".into())
        .spawn(move || {
            let mut renderer = PageRenderer::default();
            loop {
                let (page, scale) = {
                    let mut q = lock(&queue.inner);
                    loop {
                        if q.shutdown {
                            return;
                        }
                        if !q.wanted.is_empty() {
                            break q.wanted.remove(0);
                        }
                        q = queue.wake.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                };
                if map.hash(page).is_none() {
                    continue;
                }
                let started = Instant::now();
                let mut rendered = 0usize;
                renderer.set_antialias(antialias());
                {
                    let st = layout.read().unwrap_or_else(|e| e.into_inner());
                    let Some(plan) = plan_bands(&st, page, scale) else {
                        continue;
                    };
                    lock(&plans).set(page, scale, plan.clone());
                    for (i, key) in plan.keys.iter().enumerate() {
                        if lock(&cache).contains(key) {
                            continue;
                        }
                        let (y0, rows) = plan.span(i);
                        let Some(pixmap) = renderer.render_band(&st, page, scale, y0, rows) else {
                            continue;
                        };
                        let (w, h) = (pixmap.width(), pixmap.height());
                        // Pages are opaque white with glyphs blended in, so tiny-skia's
                        // premultiplied RGBA equals straight RGBA (alpha is 255 everywhere).
                        let pixels = pixmap.take();
                        let bytes = pixels.len();
                        lock(&cache).insert(*key, Handle::from_rgba(w, h, pixels), bytes);
                        rendered += 1;
                    }
                }
                if rendered > 0 {
                    perf::render_done(started.elapsed());
                    perf::event("raster", started.elapsed().as_secs_f64() * 1000.0, || {
                        format!("{rendered} page(s)")
                    });
                }
                if tx.unbounded_send(DocEvent::PageReady).is_err() {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start the render thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_estimate_scales_with_the_paper_size() {
        let chars = 40_000;
        let a4 = estimated_pages_for(chars, &PageGeometry::a4());
        let letter = estimated_pages_for(chars, &PageGeometry::letter());
        let a5 = estimated_pages_for(
            chars,
            &PageGeometry::from_setup(&libertum_model::PageSetup::a5()),
        );
        assert!(
            a5 > a4 * 2,
            "A5 holds about 2.5 times less text: {a5} vs {a4}"
        );
        assert!(
            (letter as i64 - a4 as i64).abs() <= a4 as i64 / 10,
            "Letter is close to A4: {letter} vs {a4}"
        );
        assert_eq!(estimated_pages_for(0, &PageGeometry::a4()), 1, "never zero");
    }

    #[test]
    fn rendered_pages_are_opaque_so_premultiplied_equals_straight() {
        let doc = generate_model_document(2, 7);
        let mut st = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
        st.paginate_until(1);
        let pm = PageRenderer::default()
            .render_page(&st, 0, 1.25)
            .expect("page 0");
        assert!(pm.data().as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // Some pixels are not white, so text really was drawn.
        assert!(pm.data().as_chunks::<4>().0.iter().any(|p| p[0] < 200));
    }

    #[test]
    fn handle_cache_evicts_oldest_over_cap() {
        let mut c = HandleCache::new(2000);
        let key = |page| BandKey {
            page,
            band: 0,
            hash: 0,
            scale_bits: 1,
        };
        let h = || Handle::from_rgba(1, 1, vec![0u8; 4]);
        c.insert(key(0), h(), 1000);
        c.insert(key(1), h(), 1000);
        assert!(c.get(&key(0)).is_some(), "touch page 0");
        c.insert(key(2), h(), 1000);
        assert!(!c.contains(&key(1)), "least recently used goes first");
        assert!(c.contains(&key(0)) && c.contains(&key(2)));
    }

    #[test]
    fn strips_stay_under_the_synchronous_upload_limit() {
        for width in [1, 600, 800, 1190, 1984, 2380, 4000, 8000] {
            let rows = band_rows(width);
            assert!(rows >= 1);
            assert!(rows <= MAX_BAND_ROWS);
            assert!(
                rows * width * 4 < SYNC_UPLOAD_BYTES,
                "{width} px wide: {rows} rows is too large"
            );
        }
    }

    fn laid_out(texts: &[&str]) -> LayoutState {
        let doc = Document::from_paragraphs(
            texts
                .iter()
                .enumerate()
                .map(|(i, t)| libertum_model::Paragraph::new(BlockId(i as u64 + 1), *t))
                .collect(),
        );
        let mut st = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
        st.paginate_all();
        st
    }

    #[test]
    fn an_edit_changes_only_the_strips_it_touches() {
        let body = "The quick brown fox jumps over the lazy dog. ".repeat(4);
        let texts: Vec<&str> = std::iter::repeat_n(body.as_str(), 12).collect();
        let mut st = laid_out(&texts);
        let scale = 2.0;
        let before = plan_bands(&st, 0, scale).expect("plan");
        assert!(before.keys.len() > 4, "a page has several strips");
        // Margins and the empty bottom of the page share the blank strip.
        assert!(before.keys.iter().any(|k| k.page == BLANK_BAND_PAGE));

        // Change one word in the second paragraph without changing its line count.
        st.edit_paragraph(1, |p| p.text = p.text.replacen("quick", "quack", 1));
        let after = plan_bands(&st, 0, scale).expect("plan");
        assert_eq!(after.keys.len(), before.keys.len());
        let changed = before
            .keys
            .iter()
            .zip(&after.keys)
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed >= 1, "the edited lines' strip changes");
        assert!(
            changed <= 2,
            "only the strips around the edited paragraph change, not {changed} of {}",
            after.keys.len()
        );
    }

    #[test]
    fn strips_assemble_into_the_whole_page() {
        let mut st = laid_out(&["Hello strips. ".repeat(40).as_str(), "Second paragraph."]);
        if st.font_report().family_count == 0 {
            return;
        }
        st.paginate_all();
        let scale = 1.5;
        let plan = plan_bands(&st, 0, scale).expect("plan");
        let mut renderer = PageRenderer::default();
        let whole = renderer.render_page(&st, 0, scale).expect("page");
        for i in 0..plan.keys.len() {
            let (y0, rows) = plan.span(i);
            let band = renderer.render_band(&st, 0, scale, y0, rows).expect("band");
            let row_bytes = (whole.width() * 4) as usize;
            let start = y0 as usize * row_bytes;
            let end = start + rows as usize * row_bytes;
            assert_eq!(
                band.data(),
                &whole.data()[start..end],
                "strip {i} differs from the same rows of the whole page"
            );
        }
    }

    #[test]
    fn bands_show_the_previous_strips_until_the_new_ones_render() {
        let (view, _rx) = DocView::open(libertum_layout::spike::generate_model_document(2, 3));
        let source = view.source.clone();
        let scale = 1.0;
        let hash = source.pages.hash(0).expect("page 0");
        // Nothing rendered yet: nothing to draw.
        assert!(!source.bands(0, scale).complete);

        let st = view.layout.read().unwrap();
        let plan = plan_bands(&st, 0, scale).expect("plan");
        drop(st);
        assert_eq!(plan.page_hash, hash);
        let h = || Handle::from_rgba(1, 1, vec![255u8; 4]);
        {
            let mut cache = lock(&source.cache);
            for k in &plan.keys {
                cache.insert(*k, h(), 4);
            }
        }
        lock(&source.plans).set(0, scale, plan.clone());
        let bands = source.bands(0, scale);
        assert!(bands.complete);
        assert_eq!(bands.over.len(), plan.keys.len());
        assert!(bands.under.is_empty());
        let last = bands.over.last().expect("strip");
        assert!((last.bottom - 1.0).abs() < 1e-6, "strips reach the bottom");

        // A newer plan with one changed strip: the old strips stay as the under layer.
        let mut newer = plan.clone();
        newer.page_hash = hash;
        newer.keys[0].hash ^= 1;
        lock(&source.plans).set(0, scale, newer);
        let bands = source.bands(0, scale);
        assert!(!bands.complete);
        assert_eq!(bands.over.len(), plan.keys.len() - 1);
        assert_eq!(bands.under.len(), plan.keys.len());
    }

    #[test]
    fn down_and_up_cross_wide_space_between_paragraphs() {
        use libertum_model::{ParaFormat, ParaProps, Paragraph};
        // Space after each paragraph wider than a line: a step the height of the text alone
        // would stop in the gap and leave the caret where it was.
        let spaced = |id: u64, text: &str| {
            Paragraph::new(BlockId(id), text).with_props(ParaProps {
                format: ParaFormat {
                    space_after_pt: Some(30.0),
                    ..ParaFormat::default()
                },
                ..ParaProps::default()
            })
        };
        let doc = Document::from_paragraphs(vec![
            spaced(1, "First paragraph"),
            spaced(2, "Second paragraph"),
            spaced(3, "Third paragraph"),
        ]);
        let (mut view, _rx) = DocView::open(doc);
        view.move_caret(|ed| {
            ed.set_caret(
                DocPosition {
                    block: BlockId(1),
                    offset: 3,
                },
                false,
            )
        });
        assert!(view.move_vertical(1, false));
        assert_eq!(view.editor().caret().block, BlockId(2));
        assert!(view.move_vertical(1, false));
        assert_eq!(view.editor().caret().block, BlockId(3));
        assert!(view.move_vertical(-1, false));
        assert_eq!(view.editor().caret().block, BlockId(2));
        // The caret itself stays as tall as the text, inside its line box.
        let caret = view.caret_rect().expect("caret");
        let line = view.caret_line_rect().expect("line");
        assert!(caret.height <= line.height + 0.01);
        assert!(caret.y >= line.y - 0.01 && caret.y + caret.height <= line.y + line.height + 0.01);
    }

    #[test]
    fn up_and_down_keep_the_column_across_a_short_line() {
        use libertum_model::Paragraph;
        let doc = Document::from_paragraphs(vec![
            Paragraph::new(BlockId(1), "A fairly long first line of text."),
            Paragraph::new(BlockId(2), "ab"),
            Paragraph::new(BlockId(3), "A fairly long third line of text."),
        ]);
        let (mut view, _rx) = DocView::open(doc);
        view.move_caret(|ed| {
            ed.set_caret(
                DocPosition {
                    block: BlockId(1),
                    offset: 20,
                },
                false,
            )
        });
        assert!(view.move_vertical(1, false));
        assert_eq!(view.editor().caret().block, BlockId(2));
        assert_eq!(
            view.editor().caret().offset,
            2,
            "the short line clamps to its end"
        );
        assert!(view.move_vertical(1, false));
        let caret = view.editor().caret();
        assert_eq!(caret.block, BlockId(3));
        assert!(
            (18..=22).contains(&caret.offset),
            "the column is restored, got offset {}",
            caret.offset
        );
        // Back up through the short line to the first one, same column.
        assert!(view.move_vertical(-1, false));
        assert!(view.move_vertical(-1, false));
        let caret = view.editor().caret();
        assert_eq!(caret.block, BlockId(1));
        assert!((18..=22).contains(&caret.offset), "got {}", caret.offset);
        // Any other movement forgets the column.
        view.move_caret(|ed| ed.move_left(false));
        assert_eq!(view.editor().preferred_x(), None);
    }

    #[test]
    fn selection_rects_after_pointer_drag() {
        use libertum_model::{BlockId, Document, Paragraph};

        let doc = Document::from_paragraphs(vec![Paragraph::new(
            BlockId(1),
            "The quick brown fox jumps over the lazy dog and keeps running for a while.",
        )]);
        let (mut view, _rx) = DocView::open(doc);
        assert!(view.set_caret_from_pointer(0, 80.0, 100.0, false));
        assert!(view.set_caret_from_pointer(0, 300.0, 100.0, true));
        let (start, end) = view.editor().ordered_selection().expect("selection");
        let layout = view.layout.read().unwrap();
        let direct = layout.selection_rects(0, start.offset..end.offset);
        eprintln!(
            "start={start:?} end={end:?} text={:?} direct={direct:?}",
            layout.paragraphs()[0].text
        );
        drop(layout);
        let rects = view.selection_rects();
        eprintln!("view={rects:?}");
        assert!(!rects.is_empty());
    }

    #[test]
    fn editing_syncs_layout_hashes_and_word_count() {
        use libertum_editor::DocPosition;
        use libertum_model::{BlockId, Document, Paragraph};

        let doc = Document::from_paragraphs(vec![
            Paragraph::new(BlockId(1), "hello world"),
            Paragraph::new(BlockId(2), "second paragraph"),
        ]);
        let (mut view, _rx) = DocView::open(doc);
        assert!(
            view.source.pages.is_complete(),
            "small document completes on open"
        );
        let first = view.editor().document().first_id().expect("block");
        let before_hash = view.source.pages.hash(0).expect("page 0");
        let before_words = view.word_count();

        view.edit(|ed| {
            ed.set_caret(
                DocPosition {
                    block: first,
                    offset: 0,
                },
                false,
            );
            ed.insert_text(" new ")
        })
        .expect("edit applies");

        assert_eq!(
            view.editor().document().paragraph(first).unwrap().text(),
            " new hello world"
        );
        assert_eq!(view.word_count(), before_words + 1);
        assert_ne!(
            view.source.pages.hash(0),
            Some(before_hash),
            "the edited page gets a new content hash"
        );
        assert!(view.caret_rect().is_some());
        let hit = view.hit_test(0, 80.0, 80.0);
        assert!(hit.is_some(), "hit-testing works after a sync");
    }

    #[test]
    fn structural_edit_reconciles_paragraph_list() {
        use libertum_editor::DocPosition;
        use libertum_model::{BlockId, Document, Paragraph};

        let doc = Document::from_paragraphs(vec![
            Paragraph::new(BlockId(1), "alpha"),
            Paragraph::new(BlockId(2), "beta"),
        ]);
        let (mut view, _rx) = DocView::open(doc);
        let first = view.editor().document().first_id().expect("block");
        view.edit(|ed| {
            ed.set_caret(
                DocPosition {
                    block: first,
                    offset: 2,
                },
                false,
            );
            ed.insert_text("\n")
        })
        .expect("split applies");

        assert_eq!(view.editor().document().len(), 3);
        let layout = view.layout.read().unwrap();
        assert_eq!(layout.paragraph_count(), 3);
        assert_eq!(layout.paragraphs()[0].text, "al");
        assert_eq!(layout.paragraphs()[1].text, "pha");
    }

    #[test]
    fn a_pageless_document_uses_its_own_paper_width_without_page_margins() {
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Text",
            )]);
        let geometry = pageless_geometry(&document);
        let a4 = PageGeometry::a4();
        assert!((geometry.width_pt - a4.width_pt).abs() < 0.01, "A4 wide");
        assert_eq!(geometry.margins.top, 0.0, "no top margin");
        assert_eq!(geometry.margins.bottom, 0.0, "no bottom margin");
        assert!(
            (geometry.margins.left - a4.margins.left).abs() < 0.01,
            "the document's own side margins"
        );

        let (view, _events) = DocView::open_pageless(document);
        assert!(view.pageless, "opened pageless");
        assert_eq!(view.geometry.margins.top, 0.0);
    }

    #[test]
    fn a_pageless_document_reports_how_much_of_each_page_its_text_uses() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(4_000);
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                text,
            )]);
        let (mut view, _events) = DocView::open_pageless(document);
        view.paginate_everything();
        let used = view.page_used_heights();
        assert!(used.len() > 2, "the paragraph spans pages: {}", used.len());
        assert_eq!(used.len(), view.page_total());
        let content_h = view.geometry.content_height();
        // Every page reports the text it actually holds, never the whole sheet: the canvas
        // stacks the next page at that point, so the blank tail of a split paragraph or
        // table never shows as a gap.
        for (page, height) in used.iter().enumerate() {
            assert!(*height > 0.0, "page {page} has text");
            assert!(
                *height <= content_h + 0.01,
                "page {page} claims {height} pt on a {content_h} pt page"
            );
        }
        assert!(
            used.iter().any(|height| content_h - height > 1.0),
            "at least one page is not completely full"
        );
        // The last page is no taller than a page either.
        assert!(*used.last().expect("pages") <= content_h + 0.01);
    }

    #[test]
    fn turning_pageless_off_restores_the_page_geometry() {
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Text",
            )]);
        let (mut view, _events) = DocView::open_pageless(document);
        assert!(view.pageless);

        view.set_pageless(false);
        assert!(!view.pageless);
        let paged = PageGeometry::from_setup(&view.document().page_setup());
        assert_eq!(view.geometry, paged, "pages as the page setup says");
        assert!(paged.margins.top > 0.0, "a top margin again");

        view.set_pageless(true);
        assert_eq!(view.geometry.margins.top, 0.0, "pageless again");
    }

    #[test]
    fn a_page_point_maps_to_the_image_under_it() {
        use libertum_model::{BlockId, Document, InlineImage, Media, Paragraph};
        let mut paragraph = Paragraph::new(BlockId(1), "Two pictures: ");
        let small =
            std::sync::Arc::new(Media::new(libertum_model::test_png_header(8, 8)).expect("png"));
        let large =
            std::sync::Arc::new(Media::new(libertum_model::test_png_header(16, 16)).expect("png"));
        paragraph.set_images(vec![
            InlineImage::fitted(14, small, 200.0),
            InlineImage::fitted(14, large, 200.0),
        ]);
        let document = Document::from_paragraphs(vec![paragraph]);
        let (view, _events) = DocView::open(document);
        std::thread::sleep(std::time::Duration::from_millis(50));

        // Each picture's own rectangle on page 0 resolves to its own index.
        let rects = view.image_rects(0);
        assert_eq!(rects.len(), 2, "both pictures are drawn");
        for (index, (x, y, width, height)) in rects.iter().enumerate() {
            let found = view.image_at(0, x + width / 2.0, y + height / 2.0);
            assert_eq!(found.map(|(_, index)| index), Some(index), "rect {index}");
        }
        // A point on the text, away from both, is not a picture.
        assert!(view.image_at(0, 5.0, 5.0).is_none());
    }

    /// Select All then Bold touches every paragraph: the layout is swapped in at once, the
    /// pages are rebuilt afterwards, and the result matches a layout built from scratch.
    #[test]
    fn a_document_wide_edit_defers_pagination_and_still_matches_a_fresh_layout() {
        let (mut view, _rx) = DocView::open(libertum_layout::spike::generate_model_document(40, 5));
        view.paginate_everything();
        let before = view.page_total();
        view.select_all();
        view.edit(|editor| editor.toggle_bold()).expect("bold");
        // Whatever the background pass has done so far, finishing it gives the final layout.
        view.paginate_everything();
        let doc = view.document();
        let geometry = PageGeometry::from_setup(&doc.page_setup());
        let mut fresh = LayoutState::from_document(doc, geometry, LayoutOptions::default());
        fresh.paginate_all();
        assert_eq!(view.page_total(), fresh.page_count());
        assert!(
            view.page_total() >= before,
            "bold text cannot shrink the document"
        );
        for page in 0..fresh.page_count() {
            assert!(
                view.page_content_hash(page).is_some(),
                "page {page} has no hash"
            );
        }
    }

    /// Select All on a long document: the canvas asks for the highlight of the pages near the
    /// view only, and what it gets is the same as the matching part of the full highlight.
    #[test]
    fn selection_rects_on_pages_match_the_full_selection_there() {
        let (mut view, _rx) = DocView::open(libertum_layout::spike::generate_model_document(40, 5));
        view.paginate_everything();
        view.select_all();
        let full = view.selection_rects();
        let near = view.selection_rects_on(10..13);
        assert!(!near.is_empty());
        assert!(near.len() < full.len() / 4, "only a few pages' worth");
        for page in 11..12 {
            let expected: Vec<_> = full.iter().filter(|r| r.page == page).collect();
            let got: Vec<_> = near.iter().filter(|r| r.page == page).collect();
            assert_eq!(expected, got, "page {page}");
        }
        assert!(
            view.selection_rects_on(500..510).is_empty(),
            "beyond the pages"
        );
    }

    #[test]
    fn plans_for_pages_far_from_the_viewport_are_forgotten() {
        // A source with 400 pages, so `request` can run without a live worker.
        let pages = PageMap::default();
        {
            let mut map = lock(&pages.0);
            map.hashes = vec![1; 400];
            map.complete = true;
        }
        let source = PageSource {
            pages,
            cache: Arc::new(Mutex::new(HandleCache::new(1024))),
            plans: Arc::new(Mutex::new(Plans::default())),
            queue: Arc::new(RenderQueue::default()),
        };
        let plan = || BandPlan {
            page_hash: 1,
            height: 100,
            rows: 100,
            keys: Vec::new(),
        };
        for page in 0..400 {
            if page != 300 {
                lock(&source.plans).set(page, 1.0, plan());
            }
        }
        assert_eq!(lock(&source.plans).len(), 399);

        // Scrolling to page 300 keeps a margin either side and forgets the rest. (Page 300
        // itself has no plan yet, so the request is not a no-op that skips pruning.)
        source.request(&[300], 1.0);
        let plans = lock(&source.plans);
        assert_eq!(plans.len(), 400 - (300 - PENDING_MARGIN - PLAN_MARGIN) - 1);
        assert!(plans.current(163, 1.0, 1).is_none(), "far behind");
        assert!(plans.current(164, 1.0, 1).is_some(), "margin kept");
        assert!(plans.current(399, 1.0, 1).is_some());
    }

    #[test]
    fn page_views_are_shared_between_calls_and_rebuilt_after_an_edit() {
        use libertum_editor::DocPosition;
        use libertum_model::{BlockId, Document, Paragraph};

        let doc = Document::from_paragraphs(vec![
            Paragraph::new(BlockId(1), "one"),
            Paragraph::new(BlockId(2), "two"),
        ]);
        let (mut view, _rx) = DocView::open(doc);
        let first = view.editor().document().first_id().expect("block");

        let geometries = view.page_geometries();
        let again = view.page_geometries();
        assert!(
            Arc::ptr_eq(&geometries, &again),
            "a draw or a scroll event reuses the vectors"
        );
        let used = view.page_used_heights();
        let used_again = view.page_used_heights();
        assert!(Arc::ptr_eq(&used, &used_again));

        view.edit(|ed| {
            ed.set_caret(
                DocPosition {
                    block: first,
                    offset: 0,
                },
                false,
            );
            ed.insert_text("more ")
        })
        .expect("edit");
        let after = view.page_geometries();
        assert!(
            !Arc::ptr_eq(&geometries, &after),
            "the layout changed, so the next call rebuilds"
        );
        assert_eq!(after.len(), view.page_total());
    }
}
