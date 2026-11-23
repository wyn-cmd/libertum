//! Pagination (line-level flow into pages) with incremental re-pagination.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard};

use parley::editing::Cursor;
use parley::layout::Affinity;

use libertum_model::{
    Align, DEFAULT_CELL_MARGIN_PT, Document, HeaderFooter, HeadersFooters, HfKind, HfVariant,
    PageNumberHorizontal, PageNumberVertical, PageNumbering, ResolvedChar, Section, TableBorders,
};

use crate::engine::{Engine, EngineStats, FontReport, LayoutOptions, ParagraphLayout};
use crate::geometry::{Margins, PageGeometry};
use crate::spike::{ParagraphSnapshot, SpanStyle};

/// A position in the flow: (paragraph index, line index within the paragraph).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FlowPos {
    pub para: usize,
    pub line: usize,
}

/// A run of consecutive lines of one paragraph placed on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParaSlice {
    pub para: usize,
    /// First line (inclusive).
    pub line_start: usize,
    /// One past the last line.
    pub line_end: usize,
    /// Top of the first line, in points below the top margin.
    pub y: f32,
    /// Column the slice is in, 0-based (a one-column page always uses 0).
    pub col: u16,
    /// Left edge of that column, points from the left margin.
    pub x: f32,
    /// The slice is in a table cell kept upright on a vertical page. `x` and `y` are then
    /// on the sheet itself (points from its left and top margins), and the lines run along
    /// the sheet's top edge like any horizontal text.
    pub upright: bool,
}

/// One table in the flow. Cell `paragraphs` are indices into the layout's paragraph list.
#[derive(Clone, Debug, PartialEq)]
pub struct TableSnapshot {
    pub id: u64,
    pub widths_pt: Vec<f32>,
    pub header_rows: u8,
    pub borders: TableBorders,
    pub rows: Vec<RowSnapshot>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowSnapshot {
    pub cells: Vec<CellSnapshot>,
    pub min_height_pt: f32,
    /// The row may break across pages at a line boundary when it does not fit. A row that
    /// may not break still does when it is taller than a whole page.
    pub can_split: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellSnapshot {
    /// First grid column the cell covers.
    pub col: usize,
    /// Grid columns covered (DOCX `gridSpan`).
    pub span: usize,
    pub fill: Option<[u8; 4]>,
    /// Paragraph indices, in cell order.
    pub paragraphs: Vec<usize>,
    /// A vertical-merge continuation: covered by the cell above. Its paragraphs are not
    /// laid out and no line is drawn between it and the cell above.
    pub merged_above: bool,
}

/// A table row placed on a page, in page points (from the page's top-left corner).
#[derive(Clone, Debug, PartialEq)]
pub struct TableDraw {
    pub table: u64,
    pub row: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub cells: Vec<CellDraw>,
    pub borders: TableBorders,
    /// A repeated header row on a continuation page (drawn, not part of the flow).
    pub header: bool,
    /// The line along the bottom of this piece is drawn (the table's last row, or the last
    /// piece of the table in its column).
    pub bottom: bool,
    /// The left and right outer lines are drawn.
    pub sides: bool,
    /// The row is part of a table kept upright on a vertical page: all its coordinates are
    /// on the sheet itself (points from its margins), not on the turned layout page.
    pub upright: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellDraw {
    pub x: f32,
    pub width: f32,
    pub fill: Option<[u8; 4]>,
    /// The line along the top of the cell is drawn.
    pub top: bool,
    /// The line at the cell's left edge is drawn (an inner line; never for the first cell).
    pub left: bool,
}

/// A footnote shown at the bottom of a page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageNote {
    pub number: u32,
    pub text: String,
}

/// One laid-out page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageLayout {
    /// Where this page starts in the flow (also `slices[0]`'s first line).
    pub start: FlowPos,
    /// This page's own geometry (paper, margins, orientation).
    pub geometry: PageGeometry,
    pub slices: Vec<ParaSlice>,
    /// This page is the endnote section at the end of the document.
    pub endnotes: bool,
    /// Footnotes whose reference paragraphs start on this page.
    pub notes: Vec<PageNote>,
    /// Height of content on the page, in points.
    pub used_height: f32,
    /// Section the page belongs to (index into the document's sections).
    pub section: usize,
    /// Page number for the whole document (the document's numbering setting).
    pub number: u32,
    /// False on pages before the numbering's start page: they carry no number.
    pub number_shown: bool,
    /// True when this is the first page of its section (a title page, typically).
    pub first_in_section: bool,
    /// Table rows placed on this page, painted under the text.
    pub tables: Vec<TableDraw>,
}

/// Something drawn on a page that is not body text: page numbers, and the page numbers and
/// dot leaders of table-of-contents entries. Coordinates are page points.
#[derive(Clone)]
pub enum Decoration {
    /// A one-line text layout of `text` whose top-left corner is at (`x`, `y`).
    ///
    /// On a vertical page, `sheet` text (headers, footers and page numbers) is positioned on
    /// the real sheet and drawn upright and horizontal at the top or bottom of it, not turned
    /// with the columns like the rest of the page: `x` and `y` are then sheet coordinates.
    Text {
        layout: Arc<ParagraphLayout>,
        text: String,
        x: f32,
        y: f32,
        sheet: bool,
    },
    /// A dot leader from `x0` to `x1` sitting on `baseline`.
    Leader { x0: f32, x1: f32, baseline: f32 },
    /// A filled rectangle, page points: a paragraph border line.
    Rect {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        colour: [u8; 4],
    },
    /// An inline image drawn in the rectangle (`x`, `y`, `width`, `height`), page points.
    Image {
        media: Arc<libertum_model::Media>,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

/// A clickable contents entry: its box on the page and the page it jumps to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TocLink {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Index of the page the entry's heading starts on.
    pub target_page: usize,
}

/// A page's watermark, ready to draw: a one-line layout of the text, centred on
/// (`centre_x`, `centre_y`) page points and turned by `angle_deg` (negative rises to the
/// right). It is drawn behind everything else on the page.
#[derive(Clone)]
pub struct WatermarkDraw {
    pub layout: Arc<ParagraphLayout>,
    pub text: String,
    pub centre_x: f32,
    pub centre_y: f32,
    pub angle_deg: f32,
    /// Straight RGBA; the alpha is how faint the watermark is.
    pub colour: [u8; 4],
}

/// Colour of a watermark: mid grey at about a third strength, readable under black text.
pub const WATERMARK_COLOUR: [u8; 4] = [128, 128, 128, 80];
/// Font size the watermark text is measured at before it is scaled to the page, points.
const WATERMARK_MEASURE_PT: f32 = 100.0;
/// Smallest and largest watermark font size, points.
const WATERMARK_MIN_PT: f32 = 18.0;
const WATERMARK_MAX_PT: f32 = 220.0;

/// One thing drawn on a page, for strip invalidation: see [`LayoutState::page_items`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageItem {
    /// Top and bottom of the area it may ink, page points (padded for overhang).
    pub top: f32,
    pub bottom: f32,
    /// Hash of what is drawn and where; changes whenever its pixels would.
    pub key: u64,
}

/// Padding added above and below a line box for glyph ink outside it, as a fraction of the
/// line height.
pub const INK_OVERHANG: f32 = 0.35;

/// Spacing of leader dots, points.
pub const LEADER_PITCH_PT: f32 = 4.0;
/// Gap between text and its leader, points.
const LEADER_GAP_PT: f32 = 4.0;

/// What an (incremental) pagination pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaginationStats {
    /// Index of the first page that was re-laid-out.
    pub first_page: usize,
    /// Number of pages rebuilt by this pass.
    pub pages_rebuilt: usize,
    /// Number of old pages that were kept untouched because breaks converged.
    pub pages_reused: usize,
    /// True if re-pagination stopped early because a break matched the previous one.
    pub converged: bool,
    /// parley layouts built during the pass (paragraph shaping + line breaking).
    pub paragraphs_laid_out: u64,
    /// Total pages after the pass (of the part paginated so far).
    pub total_pages: usize,
}

/// Result of [`LayoutState::hit_test`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitResult {
    pub para: usize,
    /// Byte offset into the paragraph text (always on a cluster boundary).
    pub byte_offset: usize,
}

/// Caret rectangle in page coordinates (points, origin at the page's top-left corner).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaretRect {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A selection highlight rectangle in page coordinates (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionRect {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

const EPS: f32 = 0.01;

/// Narrowest a column may be drawn, points (0.5 in). A section asking for more columns than
/// fit at this width gets fewer, as Word does.
const MIN_COLUMN_PT: f32 = 36.0;

/// Splits the text width into `count` equal columns with `gutter` between them. Returns the
/// count actually used (reduced so no column is narrower than [`MIN_COLUMN_PT`]), the column
/// width and the gutter, all in points.
pub fn column_layout(content_width: f32, count: u8, gutter: f32) -> (usize, f32, f32) {
    let gutter = if gutter.is_finite() {
        gutter.max(0.0)
    } else {
        0.0
    };
    let content_width = content_width.max(1.0);
    let mut n = usize::from(count.max(1)).min(usize::from(libertum_model::MAX_COLUMNS));
    while n > 1 && ((content_width - (n - 1) as f32 * gutter) / n as f32) < MIN_COLUMN_PT {
        n -= 1;
    }
    let width = ((content_width - (n - 1) as f32 * gutter) / n as f32).max(1.0);
    (n, width, gutter)
}

/// True when two section lists ask for the same columns and gutters; a change needs a full
/// re-pagination, because every line moves.
fn same_columns(a: &[Section], b: &[Section]) -> bool {
    let default = [Section::default()];
    let a = if a.is_empty() { &default[..] } else { a };
    let b = if b.is_empty() { &default[..] } else { b };
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.columns == y.columns && (x.column_gutter_pt - y.column_gutter_pt).abs() < 0.001
        })
}

/// Layout id of a measured header or footer; furniture ids sit below this (see
/// `LayoutState::furniture_id`) and well above paragraph ids.
const FURNITURE_ID_BASE: u64 = u64::MAX - 1024;

/// The text a header or footer is measured at: its page fields shown as up to four digits,
/// which is never narrower than the real number for documents under 10,000 pages.
fn measurement_text(text: &str) -> String {
    text.replace("{pages}", "0000").replace("{page}", "0000")
}

/// Document layout state: paragraphs, page geometry, caches and the page map.
///
/// Interior `Mutex` guards the font context and caches, so read-only users (the renderer)
/// can hold `&LayoutState` and still trigger lazy re-layout of evicted paragraphs.
pub struct LayoutState {
    /// The page geometry content is laid out in. Header and footer furniture can push the
    /// body inwards, so this is [`base_geometry`](Self::base_geometry) plus those insets.
    geometry: PageGeometry,
    /// Per-section body geometry (section 0 is also kept in `geometry` for callers that
    /// still need a single value).
    geometries: Vec<PageGeometry>,
    /// The page setup's own geometry, before header and footer insets.
    base_geometry: PageGeometry,
    /// Section settings (page numbering, headers and footers); section `n` starts at the
    /// `n`-th section start.
    sections: Vec<Section>,
    paragraphs: Vec<ParagraphSnapshot>,
    /// Tables in the flow (cell paragraph indices point into `paragraphs`).
    tables: Vec<TableSnapshot>,
    /// First paragraph index of each table row -> (table index, row index).
    row_at: HashMap<usize, (usize, usize)>,
    /// Paragraph index -> (table index, first column, span) for cell widths.
    cell_at: HashMap<usize, (usize, usize, usize)>,
    /// Cell paragraph index -> first paragraph index of its row (the row's flow position).
    row_first_of: HashMap<usize, usize>,
    /// Per table: kept upright on this vertical layout (see [`Self::refresh_upright`]).
    upright_tables: Vec<bool>,
    pages: Vec<PageLayout>,
    /// Where the next page starts if pagination is not complete.
    resume: Option<FlowPos>,
    complete: bool,
    /// Document default character formatting: the font of header and footer text.
    furniture_style: SpanStyle,
    /// Bumped whenever `furniture_style` changes, so cached furniture layouts rebuild.
    furniture_epoch: u64,
    /// Markdown reading view: keep the caller's geometry instead of the sections' page
    /// margins (the sections still carry the document's own page setup for export).
    ignore_section_margins: bool,
    /// Pageless view: one endless column. Page numbers, headers and footers are not drawn
    /// and reserve no room, because there are no pages to put them on.
    pageless: bool,
    /// Whole-document page numbering (show, position, start page and number).
    numbering: PageNumbering,
    /// Whether the document-end endnote page has been appended.
    endnotes_emitted: bool,
    engine: Mutex<Engine>,
}

fn lock(m: &Mutex<Engine>) -> MutexGuard<'_, Engine> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How far ahead of pagination paragraphs are shaped in parallel: a window of this many
/// paragraphs per batch when paginating a few pages, and when paginating everything.
const PREWARM_WINDOW_CHUNK: usize = 128;
const PREWARM_WINDOW_ALL: usize = 512;
/// A batch is only worth the threads when at least this many paragraphs are missing.
const PREWARM_MIN_MISSES: usize = 48;
/// The next window is prepared when pagination is this close to the end of the last one.
const PREWARM_REFILL: usize = 32;

impl LayoutState {
    /// Creates a state with no pages yet; call [`paginate_until`](Self::paginate_until) or
    /// [`paginate_all`](Self::paginate_all).
    pub fn new(
        geometry: PageGeometry,
        paragraphs: Vec<ParagraphSnapshot>,
        options: LayoutOptions,
    ) -> Self {
        let complete = paragraphs.is_empty();
        let mut sections = vec![Section::default()];
        sections[0].page = page_setup_from_geometry(geometry);
        let state = Self {
            geometry,
            geometries: vec![geometry],
            base_geometry: geometry,
            sections,
            paragraphs,
            tables: Vec::new(),
            row_at: HashMap::new(),
            cell_at: HashMap::new(),
            row_first_of: HashMap::new(),
            upright_tables: Vec::new(),
            pages: Vec::new(),
            resume: Some(FlowPos::default()),
            complete,
            furniture_style: SpanStyle::new(11.0),
            furniture_epoch: 0,
            ignore_section_margins: false,
            pageless: false,
            numbering: PageNumbering::default(),
            endnotes_emitted: false,
            engine: Mutex::new(Engine::new(options)),
        };
        lock(&state.engine).set_max_image_height((geometry.content_height() - 36.0).max(36.0));
        state
    }

    pub fn geometry(&self) -> &PageGeometry {
        &self.geometry
    }

    /// Whether the document is set vertically (from the right, lines running down). The page
    /// is then laid out turned a quarter turn ([`PageGeometry::transposed`]): every geometry,
    /// coordinate and rectangle in this state is in that turned layout page, and the renderer,
    /// the PDF writer and the canvas turn them back with [`physical_page_geometry`]
    /// (Self::physical_page_geometry). Not for the pageless reading view.
    pub fn is_vertical(&self) -> bool {
        !self.pageless
            && !self.ignore_section_margins
            && self
                .sections
                .first()
                .is_some_and(|section| section.vertical)
    }

    /// The real page of laid-out page `page`: its layout geometry, turned back when the text
    /// is vertical. This is the size of the sheet the renderer paints and the canvas shows.
    pub fn physical_page_geometry(&self, page: usize) -> PageGeometry {
        let geometry = self.page_geometry(page);
        if self.is_vertical() {
            geometry.untransposed()
        } else {
            geometry
        }
    }

    /// The page setup geometry for a section, with header/footer insets applied.
    pub fn geometry_for_section(&self, index: usize) -> PageGeometry {
        self.geometries.get(index).copied().unwrap_or(self.geometry)
    }

    /// The geometry of one laid-out page. This is what the renderer and the canvas must use;
    /// sections may have different paper sizes.
    pub fn page_geometry(&self, page: usize) -> PageGeometry {
        self.pages
            .get(page)
            .map(|page| page.geometry)
            .unwrap_or(self.geometry)
    }
    /// Height of the content on `page`, in points. A pageless view stacks pages by this
    /// instead of by the full paper height, so the blank below the last line of a page does
    /// not open a gap where a paragraph or table simply continued.
    ///
    /// Pages that are not paginated yet report the height they will use: the full content
    /// height, so the scroll extent does not jump about while pagination runs.
    pub fn page_used_height(&self, page: usize) -> f32 {
        self.pages
            .get(page)
            .map_or_else(|| self.geometry.content_height(), |page| page.used_height)
    }

    /// The document's page setup, before header and footer insets.
    pub fn base_geometry(&self) -> &PageGeometry {
        &self.base_geometry
    }
    pub fn paragraphs(&self) -> &[ParagraphSnapshot] {
        &self.paragraphs
    }

    /// Ignore each section's own page margins when computing the body area. Used by the
    /// Markdown reading view, whose fixed geometry must win over the document's page setup.
    /// Returns `true` when the geometry changed and pages must be laid out again.
    pub fn set_ignore_section_margins(&mut self, ignore: bool) -> bool {
        if self.ignore_section_margins == ignore {
            return false;
        }
        self.ignore_section_margins = ignore;
        let changed = self.refresh_furniture_geometry();
        if changed {
            self.invalidate_pages();
        }
        changed
    }

    /// Turns the pageless view on or off. In a pageless view the document is one endless
    /// column: page numbers, headers and footers are neither drawn nor given room. Returns
    /// `true` when the pages must be laid out again.
    pub fn set_pageless(&mut self, pageless: bool) -> bool {
        if self.pageless == pageless {
            return false;
        }
        self.pageless = pageless;
        self.refresh_furniture_geometry();
        // Even when the body area does not move, the furniture drawn on each page changes.
        self.invalidate_pages();
        true
    }

    /// Whether the pages are drawn as one endless column.
    pub fn is_pageless(&self) -> bool {
        self.pageless
    }

    /// Replaces the table structure used by pagination. Cell paragraph indices must be the
    /// layout's paragraph indices; call after the paragraph list is up to date. Structural
    /// table edits drop the pages and paginate again.
    pub fn set_tables(&mut self, tables: Vec<TableSnapshot>) {
        self.tables = tables;
        self.row_at.clear();
        self.cell_at.clear();
        self.row_first_of.clear();
        for (table_index, table) in self.tables.iter().enumerate() {
            for (row_index, row) in table.rows.iter().enumerate() {
                let mut first = None;
                for cell in &row.cells {
                    for paragraph in &cell.paragraphs {
                        self.cell_at
                            .insert(*paragraph, (table_index, cell.col, cell.span));
                    }
                    if first.is_none() {
                        first = cell.paragraphs.first().copied();
                    }
                }
                if let Some(first) = first {
                    self.row_at.insert(first, (table_index, row_index));
                    for cell in &row.cells {
                        for paragraph in &cell.paragraphs {
                            self.row_first_of.insert(*paragraph, first);
                        }
                    }
                }
            }
        }
        self.invalidate_pages();
        self.refresh_upright();
    }

    pub fn tables(&self) -> &[TableSnapshot] {
        &self.tables
    }
    /// Pages paginated so far.
    pub fn pages(&self) -> &[PageLayout] {
        &self.pages
    }
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }
    /// True once every paragraph has been paginated.
    pub fn is_complete(&self) -> bool {
        self.complete
    }
    pub fn font_report(&self) -> FontReport {
        lock(&self.engine).report().clone()
    }
    /// The font families installed on this machine (for the font picker), sorted.
    pub fn font_families(&self) -> Vec<Arc<str>> {
        lock(&self.engine).family_names()
    }
    pub fn engine_stats(&self) -> EngineStats {
        lock(&self.engine).stats()
    }

    /// Cached (or freshly built) layout of paragraph `index` at the width of its section's
    /// columns. Paragraphs beyond pagination use section 0's width.
    pub fn paragraph_layout(&self, index: usize) -> Option<Arc<ParagraphLayout>> {
        let p = self.paragraphs.get(index)?;
        let width = self.layout_width(index);
        Some(lock(&self.engine).full(p, width))
    }

    /// Width a paragraph is laid out at: the column width of the section of the page it starts
    /// on. O(log pages), so the renderer can call it per slice.
    fn layout_width(&self, index: usize) -> f32 {
        let section = self
            .pages
            .get(self.page_of_para(index))
            .map_or(0, |page| page.section);
        let mut column = self.column_width(section);
        if let Some((table_index, col, span)) = self.cell_at.get(&index)
            && let Some(table) = self.tables.get(*table_index)
        {
            if self
                .upright_tables
                .get(*table_index)
                .copied()
                .unwrap_or(false)
            {
                // An upright table spreads along the sheet's width, which is the layout
                // page's height.
                column = self.geometry_for_section(section).content_height();
            }
            let total: f32 = table.widths_pt.iter().sum();
            let scale = if total > column && total > 0.0 {
                column / total
            } else {
                1.0
            };
            let cell: f32 = table
                .widths_pt
                .iter()
                .skip(*col)
                .take((*span).max(1))
                .sum::<f32>()
                * scale;
            return (cell - 2.0 * DEFAULT_CELL_MARGIN_PT * scale).max(1.0);
        }
        column
    }

    /// Columns actually used by `section` (a request for more than fit is reduced).
    pub fn column_count(&self, section: usize) -> usize {
        let s = self.section(section);
        column_layout(
            self.geometry_for_section(section).content_width(),
            s.columns,
            s.column_gutter_pt,
        )
        .0
    }

    /// Width of one column in `section`, points.
    pub fn column_width(&self, section: usize) -> f32 {
        let s = self.section(section);
        column_layout(
            self.geometry_for_section(section).content_width(),
            s.columns,
            s.column_gutter_pt,
        )
        .1
    }

    /// Left edge of column `col` in `section`, points from the left margin.
    pub fn column_left(&self, section: usize, col: usize) -> f32 {
        let s = self.section(section);
        let (n, width, gutter) = column_layout(
            self.geometry_for_section(section).content_width(),
            s.columns,
            s.column_gutter_pt,
        );
        col.min(n.saturating_sub(1)) as f32 * (width + gutter)
    }

    /// Paginates from the start until at least `min_pages` pages exist (or the document ends).
    /// Used for "first page visible" on open.
    pub fn paginate_until(&mut self, min_pages: usize) -> PaginationStats {
        let Some(start) = self.resume else {
            return PaginationStats {
                total_pages: self.pages.len(),
                ..Default::default()
            };
        };
        let first = self.pages.len();
        self.run(first, start, Some(min_pages), Vec::new(), None, None)
    }

    /// Paginates everything not yet paginated.
    pub fn paginate_all(&mut self) -> PaginationStats {
        let Some(start) = self.resume else {
            return PaginationStats {
                total_pages: self.pages.len(),
                ..Default::default()
            };
        };
        let first = self.pages.len();
        self.run(first, start, None, Vec::new(), None, None)
    }

    /// Builds a layout state from a model document (PLAN §4.1 → §4.2).
    pub fn from_document(doc: &Document, geometry: PageGeometry, options: LayoutOptions) -> Self {
        let mut labels = doc.list_labels().into_iter();
        let language = doc.sections.first().and_then(|s| s.language.as_deref());
        let language2 = doc.sections.first().and_then(|s| s.language2.as_deref());
        let vertical = doc.sections.first().is_some_and(|s| s.vertical);
        let paragraphs = doc
            .paragraphs()
            .map(|p| {
                let mut s = ParagraphSnapshot::from_model(p, &doc.styles, language);
                s.add_language(language2);
                s.set_vertical(vertical);
                s.list_label = labels.next().flatten();
                s
            })
            .collect();
        let mut state = Self::new(geometry, paragraphs, options);
        state.furniture_style = SpanStyle::from(&doc.styles.defaults);
        state.renumber_notes();
        state.set_sections(doc.sections.clone());
        state.set_numbering(doc.page_numbering());
        state.set_tables(table_snapshots(doc));
        state
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Numbers the footnotes in document order. Called only when a note appears or
    /// disappears, because the number is not part of the paragraph's text layout.
    fn renumber_notes(&mut self) {
        let mut footnotes = 0u32;
        let mut endnotes = 0u32;
        for paragraph in &mut self.paragraphs {
            for note in &mut paragraph.notes {
                match note.kind {
                    libertum_model::NoteKind::Footnote => {
                        footnotes += 1;
                        note.number = footnotes;
                    }
                    libertum_model::NoteKind::Endnote => {
                        endnotes += 1;
                        note.number = endnotes;
                    }
                }
            }
        }
    }

    /// Replaces the section settings. Page breaks do not move; headers and footers that are
    /// taller than the margins can push the body inwards, and a change to columns or their
    /// gutter changes where every line goes, so either re-paginates every page.
    ///
    /// Returns `true` when the geometry changed and the caller must paginate again (the pages
    /// are dropped); `false` keeps the existing pages and only recounts page numbers.
    pub fn set_sections(&mut self, sections: Vec<Section>) -> bool {
        let columns_changed = !same_columns(&self.sections, &sections);
        self.sections = if sections.is_empty() {
            vec![Section::default()]
        } else {
            sections
        };
        let reflow = self.refresh_furniture_geometry();
        if reflow || columns_changed {
            self.invalidate_pages();
        }
        self.renumber_from(0);
        reflow || columns_changed
    }

    /// Updates the document default character formatting used for header and footer text.
    /// Returns `true` when a taller or shorter furniture layout re-paginated the document.
    pub fn set_furniture_style(&mut self, style: &ResolvedChar) -> bool {
        let style = SpanStyle::from(style);
        if style == self.furniture_style {
            return false;
        }
        self.furniture_style = style;
        self.furniture_epoch += 1;
        let reflow = self.refresh_furniture_geometry();
        if reflow {
            self.invalidate_pages();
            self.renumber_from(0);
        }
        reflow
    }

    /// Replaces the whole-document page numbering. Page layout does not change, so only the
    /// numbers and their decorations are recomputed; returns whether it changed.
    pub fn set_numbering(&mut self, numbering: PageNumbering) -> bool {
        let numbering = numbering.clamped();
        if self.numbering == numbering {
            return false;
        }
        self.numbering = numbering;
        self.renumber_from(0);
        true
    }

    /// The document's page numbering as this layout uses it.
    pub fn numbering(&self) -> PageNumbering {
        self.numbering
    }

    /// Drops every paginated page so the next pass starts at the top of the document.
    fn invalidate_pages(&mut self) {
        self.pages.clear();
        self.endnotes_emitted = false;
        self.resume = Some(FlowPos::default());
        self.complete = self.paragraphs.is_empty();
    }

    /// Replaces the page geometry and drops every paginated page, so the next pass lays the
    /// document out again at the new size. Returns false (and does nothing) when unchanged.
    ///
    /// A page-setup change is an explicit document action, so re-flowing the whole document is
    /// acceptable (PLAN §4.2); the caller paginates the first screenful synchronously and the
    /// rest in the background. Paragraph layouts are cached by content width, so old-width
    /// entries are simply not reused and fall out of the LRU.
    pub fn set_geometry(&mut self, geometry: PageGeometry) -> bool {
        if self.base_geometry == geometry {
            return false;
        }
        self.base_geometry = geometry;
        if !self.ignore_section_margins {
            let setup = page_setup_from_geometry(geometry);
            for section in &mut self.sections {
                section.page = setup;
            }
        }
        self.refresh_furniture_geometry();
        self.pages.clear();
        self.resume = Some(FlowPos::default());
        self.complete = self.paragraphs.is_empty();
        true
    }

    fn section(&self, index: usize) -> &Section {
        let last = self.sections.len().saturating_sub(1);
        &self.sections[index.min(last)]
    }

    // ------------------------------------------------------------ headers and footers

    /// Stable layout id of one section's header or footer variant. Far above any paragraph
    /// id, so furniture layouts never collide with body text in the engine cache.
    fn furniture_id(&self, section: usize, kind: HfKind, variant: HfVariant) -> u64 {
        let kind = match kind {
            HfKind::Header => 0u64,
            HfKind::Footer => 1,
        };
        let variant = match variant {
            HfVariant::Default => 0u64,
            HfVariant::First => 1,
            HfVariant::Even => 2,
        };
        // 8 slots per section (header/footer x three variants), up to 256 sections, before
        // the measurement id.
        FURNITURE_ID_BASE - 4096 - (section.min(255) as u64) * 8 - kind * 4 - variant
    }

    /// A layout snapshot for one line of header or footer text, in the document's default
    /// character formatting.
    fn furniture_snapshot(&self, id: u64, text: &str, align: Align) -> ParagraphSnapshot {
        let mut snap = ParagraphSnapshot::new(id, text);
        snap.version = self.furniture_version(text, align);
        snap.base = Some(self.furniture_style.clone());
        snap.align = align;
        snap.space_before_pt = 0.0;
        snap.space_after_pt = 0.0;
        snap
    }

    /// Cache version of a furniture layout: everything that changes its shaping.
    fn furniture_version(&self, text: &str, align: Align) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        align.hash(&mut h);
        self.furniture_epoch.hash(&mut h);
        h.finish()
    }

    /// Height of `text` laid out at `width`, points, with page fields shown as four digits.
    fn measure_furniture(&mut self, text: &str, align: Align, width: f32) -> f32 {
        let text = measurement_text(text);
        let snap = self.furniture_snapshot(FURNITURE_ID_BASE, &text, align);
        lock(&self.engine)
            .metrics(&snap, width)
            .heights
            .iter()
            .sum()
    }

    /// Recomputes the effective page geometry from the sections' headers and footers: a
    /// header taller than the top margin pushes the body down, and a footer taller than the
    /// bottom margin pushes it up, as Word does. Returns `true` when the margins changed.
    /// Recomputes each section's body geometry (paper, margins and header/footer insets).
    /// Returns true when any page's geometry changed.
    /// Decides which tables stay upright: on a vertical page, a table whose cells hold plain
    /// text keeps its rows top to bottom and its text horizontal (Word and InDesign turn
    /// tables back the same way). Cells with pictures, list items, borders or contents
    /// entries turn with the page instead, as their extras are only drawn turned. Cell
    /// paragraphs of an upright table are laid out as horizontal text.
    fn refresh_upright(&mut self) {
        let vertical = self.is_vertical();
        let flags: Vec<bool> = self
            .tables
            .iter()
            .map(|table| {
                vertical
                    && table.rows.iter().all(|row| {
                        row.cells.iter().all(|cell| {
                            cell.paragraphs.iter().all(|&i| {
                                self.paragraphs.get(i).is_none_or(|p| {
                                    p.images.is_empty()
                                        && p.list.is_none()
                                        && p.borders.is_none()
                                        && p.toc_target.is_none()
                                })
                            })
                        })
                    })
            })
            .collect();
        for (table, upright) in self.tables.iter().zip(&flags) {
            for row in &table.rows {
                for cell in &row.cells {
                    for &i in &cell.paragraphs {
                        if let Some(p) = self.paragraphs.get_mut(i)
                            && p.upright != *upright
                        {
                            p.upright = *upright;
                            // The cached layout was made the other way up.
                            p.version = p.version.wrapping_add(1);
                        }
                    }
                }
            }
        }
        self.upright_tables = flags;
    }

    fn refresh_furniture_geometry(&mut self) -> bool {
        self.refresh_upright();
        let sections = self.sections.clone();
        let vertical = self.is_vertical();
        let mut geometries = Vec::with_capacity(sections.len());
        for (index, section) in sections.iter().enumerate() {
            let base = if self.ignore_section_margins {
                self.base_geometry
            } else {
                PageGeometry::from_setup(&section.page)
            };
            // Everything here is worked out on the sheet, headers and footers included: a
            // header sits at the sheet's top whichever way the text runs.
            let width = base.content_width();
            let mut top = base.margins.top;
            let mut bottom = base.margins.bottom;
            // A pageless view has no pages to carry furniture, so it reserves no room for it.
            let pageless = self.pageless;
            for kind in [HfKind::Header, HfKind::Footer]
                .into_iter()
                .filter(|_| !pageless)
            {
                let set = linked_furniture(&sections, index, kind);
                // Only variants the section actually shows can take room.
                let active = HfVariant::ALL.into_iter().filter(|variant| match variant {
                    HfVariant::Default => true,
                    HfVariant::First => set.different_first,
                    HfVariant::Even => set.different_even_odd,
                });
                for variant in active {
                    let Some(hf) = set.get(variant) else { continue };
                    if hf.is_empty() {
                        continue;
                    }
                    let height = self.measure_furniture(&hf.text, hf.align, width);
                    let distance = section.header_footer_distance(kind);
                    match kind {
                        HfKind::Header => top = top.max(distance + height),
                        HfKind::Footer => bottom = bottom.max(distance + height),
                    }
                }
            }
            // Never let furniture leave less than a quarter of the page (and 36 pt) for text.
            let min_body = (base.content_height() * 0.25).max(36.0);
            top = top.min((base.height_pt - bottom - min_body).max(base.margins.top));
            bottom = bottom.min((base.height_pt - top - min_body).max(base.margins.bottom));
            let sheet = PageGeometry {
                margins: Margins {
                    top,
                    bottom,
                    ..base.margins
                },
                ..base
            };
            // Vertical text is laid out on the page turned a quarter turn, so the sheet's top
            // and bottom insets become the layout's left and right margins.
            geometries.push(if vertical { sheet.transposed() } else { sheet });
        }
        let changed = geometries != self.geometries;
        if let Some(first) = geometries.first().copied() {
            self.geometry = first;
        }
        // An inline image is never taller than the shortest page body (less a line or two
        // of slack), so the line it sits on always fits on a page.
        let shortest = geometries
            .iter()
            .map(PageGeometry::content_height)
            .fold(f32::MAX, f32::min);
        if shortest < f32::MAX {
            lock(&self.engine).set_max_image_height((shortest - 36.0).max(36.0));
        }
        self.geometries = geometries;
        changed
    }

    /// The headers or footers a section actually shows: a section with "Link to Previous"
    /// uses the nearest earlier section that does not link.
    pub fn furniture_for_section(&self, index: usize, kind: HfKind) -> &HeadersFooters {
        linked_furniture(&self.sections, index, kind)
    }

    /// The header or footer shown on `page`, with the variant in use.
    pub fn furniture_on(&self, page: usize, kind: HfKind) -> Option<(HfVariant, HeaderFooter)> {
        let pg = self.pages.get(page)?;
        self.furniture_for_section(pg.section, kind)
            .for_page(page, pg.first_in_section)
            .map(|(variant, hf)| (variant, hf.clone()))
    }

    /// Total pages known so far, for the `{pages}` field.
    fn total_pages(&self) -> u32 {
        self.pages.len().min(u32::MAX as usize) as u32
    }

    /// (section, number, shown) of the page at `index` (0-based), given the page before it.
    /// The number itself is the document's: physical page `start_page` is `start_at`, and
    /// pages before it carry no number. Section breaks still decide headers, footers and
    /// columns, but no longer restart the count.
    fn number_for(
        &self,
        index: usize,
        start: FlowPos,
        prev: Option<&PageLayout>,
    ) -> (usize, u32, bool) {
        let opens_section = start.line == 0
            && start.para > 0
            && self
                .paragraphs
                .get(start.para)
                .is_some_and(|p| p.section_start);
        let (number, shown) = self.numbering.number_on(index as u32 + 1);
        let Some(prev) = prev else {
            return (0, number, shown);
        };
        let section = if opens_section {
            (prev.section + 1).min(self.sections.len().saturating_sub(1))
        } else {
            prev.section
        };
        (section, number, shown)
    }

    /// Recounts section and page number from page `from` on. Stops at the first page whose
    /// numbers are already right: every later page depends only on its predecessor, so the
    /// rest are right too. That keeps an ordinary edit O(1) here.
    fn renumber_from(&mut self, from: usize) {
        for i in from..self.pages.len() {
            let prev = i.checked_sub(1).map(|j| &self.pages[j]);
            let (section, number, number_shown) = self.number_for(i, self.pages[i].start, prev);
            let page = &mut self.pages[i];
            if i > from
                && page.section == section
                && page.number == number
                && page.number_shown == number_shown
            {
                break;
            }
            page.section = section;
            page.number = number;
            page.number_shown = number_shown;
        }
    }

    pub fn paragraph_count(&self) -> usize {
        self.paragraphs.len()
    }

    /// Replaces paragraph `index` with a fresh model snapshot and re-paginates incrementally.
    pub fn replace_paragraph(
        &mut self,
        index: usize,
        snapshot: ParagraphSnapshot,
    ) -> Option<PaginationStats> {
        let p = self.paragraphs.get_mut(index)?;
        let mut snapshot = snapshot;
        snapshot.upright = p.upright;
        let old_notes = p.notes.clone();
        let flow_changed = p.forces_new_page() != snapshot.forces_new_page()
            || p.space_before_pt != snapshot.space_before_pt
            || p.keep_with_next != snapshot.keep_with_next
            || p.keep_together != snapshot.keep_together
            || p.widow_control != snapshot.widow_control;
        let notes_compatible = old_notes.len() == snapshot.notes.len()
            && old_notes
                .iter()
                .zip(&snapshot.notes)
                .all(|(old, new)| old.kind == new.kind);
        *p = snapshot;
        if notes_compatible {
            for (note, old) in p.notes.iter_mut().zip(old_notes) {
                note.number = old.number;
            }
        } else {
            self.renumber_notes();
        }
        // A paragraph kept with this one may move back or forward with it.
        let kept = index > 0 && self.paragraphs[index - 1].keep_with_next;
        Some(self.repaginate_from(index, index, flow_changed || kept))
    }

    /// Replaces many paragraphs at once (a paragraph style changed) and re-paginates once,
    /// from the page of the first one. Entries with an index out of range are ignored.
    pub fn replace_paragraphs(
        &mut self,
        items: Vec<(usize, ParagraphSnapshot)>,
    ) -> Option<PaginationStats> {
        let mut range: Option<(usize, usize)> = None;
        for (index, snapshot) in items {
            if let Some(p) = self.paragraphs.get_mut(index) {
                *p = snapshot;
                range = Some(range.map_or((index, index), |(a, b)| (a.min(index), b.max(index))));
            }
        }
        let (first, last) = range?;
        self.renumber_notes();
        Some(self.repaginate_from(first, last, true))
    }

    /// Like [`replace_paragraphs`](Self::replace_paragraphs), but does not re-paginate: the
    /// snapshots are swapped in and every page from the first changed paragraph's page on is
    /// dropped, to be rebuilt by `paginate_until` and `paginate_all` (the background pass).
    /// For a change that touches thousands of paragraphs (Select All then Bold, a style
    /// change), where re-paginating on the caller's thread froze the window for over a second
    /// at 500 pages. Returns true when pages were dropped and a pagination pass is needed.
    pub fn replace_paragraphs_deferred(&mut self, items: Vec<(usize, ParagraphSnapshot)>) -> bool {
        let mut first: Option<usize> = None;
        for (index, snapshot) in items {
            if let Some(p) = self.paragraphs.get_mut(index) {
                let upright = p.upright;
                *p = snapshot;
                p.upright = upright;
                first = Some(first.map_or(index, |f| f.min(index)));
            }
        }
        let Some(first) = first else {
            return false;
        };
        self.renumber_notes();
        if self.pages.is_empty() {
            return false;
        }
        if let Some(resume) = self.resume
            && (FlowPos {
                para: first,
                line: 0,
            }) >= resume
        {
            // Everything changed lies beyond the paginated prefix.
            return false;
        }
        // One page earlier, as `replace_paragraphs` does: the paragraph may move back.
        let first_page = self.page_of_para(first).saturating_sub(1);
        let old_tail = self.pages.split_off(first_page);
        self.resume = Some(old_tail.first().map_or(FlowPos::default(), |p| p.start));
        self.complete = false;
        self.endnotes_emitted = false;
        true
    }

    /// Applies an edit to paragraph `index`, bumps its version, and re-paginates
    /// incrementally (PLAN §4.2): starts at the page where the paragraph begins and stops as
    /// soon as a page break lands on the same (paragraph, line) as before in an untouched
    /// later paragraph.
    ///
    /// Returns `None` if `index` is out of range. If the paragraph lies beyond the part of
    /// the document paginated so far, only the version is bumped.
    pub fn edit_paragraph(
        &mut self,
        index: usize,
        edit: impl FnOnce(&mut ParagraphSnapshot),
    ) -> Option<PaginationStats> {
        let p = self.paragraphs.get_mut(index)?;
        edit(p);
        p.version += 1;
        Some(self.repaginate_from(index, index, false))
    }

    /// Re-paginates after paragraphs `index..=last` changed. `flow_changed` (a page break,
    /// space before or keep option changed) starts one page earlier, where the paragraph may
    /// now move back to. Early convergence is only allowed after `last`.
    fn repaginate_from(
        &mut self,
        index: usize,
        last: usize,
        flow_changed: bool,
    ) -> PaginationStats {
        if self.pages.is_empty() {
            return PaginationStats::default();
        }
        if let Some(resume) = self.resume {
            let at = FlowPos {
                para: index,
                line: 0,
            };
            if at >= resume {
                return PaginationStats {
                    total_pages: self.pages.len(),
                    ..Default::default()
                };
            }
        }
        let mut first = self.page_of_para(index);
        if flow_changed {
            first = first.saturating_sub(1);
        }
        let old_tail = self.pages.split_off(first);
        let start = old_tail.first().map_or(FlowPos::default(), |p| p.start);
        self.run(first, start, None, old_tail, Some(last), None)
    }

    /// Inserts a paragraph (a split, or a new block) and re-paginates from its page.
    ///
    /// Structural edits rebuild from the affected page to the end of the already-paginated
    /// prefix. They do not converge early, because paragraph indices after the edit move.
    pub fn insert_paragraph(
        &mut self,
        index: usize,
        snapshot: ParagraphSnapshot,
    ) -> PaginationStats {
        let index = index.min(self.paragraphs.len());
        let old_resume = self.resume;
        let mut new_resume = old_resume;
        if let Some(r) = &mut new_resume
            && r.para >= index
        {
            r.para += 1;
        }
        self.paragraphs.insert(index, snapshot);
        self.renumber_notes();
        self.resume = new_resume;
        if self.pages.is_empty() {
            return PaginationStats::default();
        }
        // The new paragraph is beyond the paginated prefix: nothing to repaginate.
        if let Some(r) = new_resume
            && index >= r.para
        {
            return PaginationStats {
                total_pages: self.pages.len(),
                ..Default::default()
            };
        }
        let first = self.page_of_para(index);
        let old_tail = self.pages.split_off(first);
        let start = old_tail.first().map_or(FlowPos::default(), |p| p.start);
        self.run(first, start, None, Vec::new(), None, new_resume)
    }

    /// Removes a paragraph (a merge) and re-paginates from its page.
    pub fn remove_paragraph(&mut self, index: usize) -> Option<PaginationStats> {
        if index >= self.paragraphs.len() {
            return None;
        }
        self.paragraphs.remove(index);
        self.renumber_notes();
        let old_resume = self.resume;
        let mut new_resume = old_resume;
        if let Some(r) = &mut new_resume {
            match r.para.cmp(&index) {
                std::cmp::Ordering::Greater => r.para -= 1,
                std::cmp::Ordering::Equal => {
                    r.para = index;
                    r.line = 0;
                }
                std::cmp::Ordering::Less => {}
            }
        }
        self.resume = new_resume;
        if self.paragraphs.is_empty() {
            self.pages.clear();
            self.resume = None;
            self.complete = true;
            return Some(PaginationStats::default());
        }
        if self.pages.is_empty() {
            return Some(PaginationStats::default());
        }
        // The removed paragraph was beyond the paginated prefix: nothing to repaginate.
        if let Some(o) = old_resume
            && index >= o.para
        {
            return Some(PaginationStats {
                total_pages: self.pages.len(),
                ..Default::default()
            });
        }
        let first = self.page_of_para(index);
        let old_tail = self.pages.split_off(first);
        let start = old_tail.first().map_or(FlowPos::default(), |p| p.start);
        Some(self.run(first, start, None, Vec::new(), None, new_resume))
    }

    /// Index of the page on which paragraph `para` begins (its first line).
    ///
    /// A paragraph in a table cell is placed with its row, so this is the page its row
    /// starts on (a row that splits across pages continues on the following ones).
    pub fn page_of_para(&self, para: usize) -> usize {
        let para = self.row_first_of.get(&para).copied().unwrap_or(para);
        let at = FlowPos { para, line: 0 };
        self.pages
            .partition_point(|p| p.start <= at)
            .saturating_sub(1)
    }

    fn run(
        &mut self,
        first_page: usize,
        start: FlowPos,
        limit_pages: Option<usize>,
        old_tail: Vec<PageLayout>,
        edited: Option<usize>,
        stop_at: Option<FlowPos>,
    ) -> PaginationStats {
        let was_complete = self.complete;
        let old_resume = self.resume;
        let n = self.paragraphs.len();
        let mut eng = lock(&self.engine);
        let built_before = eng.stats().layouts_built;
        let mut pos = start;
        let mut rebuilt = 0usize;
        let mut reused = 0usize;
        let mut converged = false;
        self.pages.truncate(first_page);
        // Paragraphs below this index have been looked at for pre-shaping (see `prewarm`).
        let mut warmed_to = pos.para;
        loop {
            if pos.para >= n {
                self.complete = true;
                self.resume = None;
                break;
            }
            if let Some(stop) = stop_at
                && pos >= stop
            {
                self.resume = Some(pos);
                self.complete = false;
                break;
            }
            if let Some(lim) = limit_pages
                && self.pages.len() >= lim
            {
                self.resume = Some(pos);
                self.complete = false;
                break;
            }
            if !was_complete && edited.is_some() {
                // Incremental edit on a partially paginated document: do not run past the
                // previously paginated prefix.
                if let Some(r) = old_resume
                    && pos >= r
                {
                    self.resume = Some(pos);
                    self.complete = false;
                    break;
                }
            }
            let (section, number, number_shown) =
                self.number_for(self.pages.len(), pos, self.pages.last());
            if edited.is_none() && pos.para + PREWARM_REFILL >= warmed_to && warmed_to < n {
                // Several paragraphs ahead are not laid out yet: shape them on all cores now
                // instead of one at a time below. An incremental edit never needs this.
                let window = if limit_pages.is_some() {
                    PREWARM_WINDOW_CHUNK
                } else {
                    PREWARM_WINDOW_ALL
                };
                let from = pos.para.max(warmed_to);
                let to = (from + window).min(n);
                let width = self.column_width(section);
                let ahead: Vec<&ParagraphSnapshot> = self.paragraphs[from..to]
                    .iter()
                    .filter(|p| !eng.has_metrics(p, width))
                    .collect();
                if ahead.len() >= PREWARM_MIN_MISSES {
                    eng.prewarm(&ahead, width);
                }
                warmed_to = to;
            }
            let section_settings = self.section(section);
            let geo = self.geometry_for_section(section);
            let full_h = geo.content_height();
            let mut body_h = full_h;
            let mut attempts = 0;
            let (mut page, next) = loop {
                attempts += 1;
                let (mut page, next) = build_page(
                    &self.paragraphs,
                    &self.tables,
                    &self.upright_tables,
                    &self.row_at,
                    &mut eng,
                    &geo,
                    body_h,
                    section_settings.columns,
                    section_settings.column_gutter_pt,
                    pos,
                );
                let notes = notes_on_page(&page.slices, &self.paragraphs);
                if notes.is_empty() {
                    page.notes = notes;
                    break (page, next);
                }
                let note_h = note_block_height(&mut eng, &notes, geo.content_width());
                let wanted = (full_h - note_h).max(36.0);
                if body_h <= wanted + 0.5 {
                    page.notes = notes;
                    break (page, next);
                }
                if attempts >= 4 {
                    page.notes = notes;
                    break (page, next);
                }
                body_h = wanted;
            };
            (page.section, page.number, page.number_shown) = (section, number, number_shown);
            page.first_in_section = self
                .pages
                .last()
                .is_none_or(|previous| previous.section != page.section);
            self.pages.push(page);
            rebuilt += 1;
            if let Some(e) = edited {
                // Safe to converge only on a break inside a later, untouched paragraph.
                if next.para > e
                    && next.para < n
                    && let Ok(j) = old_tail.binary_search_by(|p| p.start.cmp(&next))
                {
                    reused = old_tail.len() - j;
                    let from = self.pages.len();
                    self.pages.extend(old_tail.into_iter().skip(j));
                    converged = true;
                    self.complete = was_complete;
                    self.resume = old_resume;
                    drop(eng);
                    // The reused pages keep their numbers unless a page was added or
                    // removed before them; renumbering stops at the first correct one.
                    self.renumber_from(from);
                    return PaginationStats {
                        first_page,
                        pages_rebuilt: rebuilt,
                        pages_reused: reused,
                        converged,
                        paragraphs_laid_out: lock(&self.engine).stats().layouts_built
                            - built_before,
                        total_pages: self.pages.len(),
                    };
                }
            }
            pos = next;
        }
        if self.complete && !self.endnotes_emitted {
            let mut notes = Vec::new();
            for paragraph in &self.paragraphs {
                for note in paragraph
                    .notes
                    .iter()
                    .filter(|note| note.kind == libertum_model::NoteKind::Endnote)
                {
                    notes.push(PageNote {
                        number: note.number,
                        text: note.text.clone(),
                    });
                }
            }
            if !notes.is_empty() {
                let (section, geometry) = self.pages.last().map_or((0, self.geometry), |last| {
                    (last.section, self.geometry_for_section(last.section))
                });
                let (number, number_shown) = self.numbering.number_on(self.pages.len() as u32 + 1);
                self.pages.push(PageLayout {
                    start: FlowPos {
                        para: self.paragraphs.len(),
                        line: 0,
                    },
                    geometry,
                    slices: Vec::new(),
                    endnotes: true,
                    notes,
                    used_height: 0.0,
                    section,
                    number,
                    number_shown,
                    first_in_section: false,
                    tables: Vec::new(),
                });
            }
            self.endnotes_emitted = true;
        }
        PaginationStats {
            first_page,
            pages_rebuilt: rebuilt,
            pages_reused: reused,
            converged,
            paragraphs_laid_out: eng.stats().layouts_built - built_before,
            total_pages: self.pages.len(),
        }
    }

    /// True when any page's content depends on the final page count or on another page's
    /// number: a `{pages}` header/footer, or a table-of-contents entry. The app then re-hashes
    /// the whole document after an edit; otherwise it can start at the first changed page.
    pub fn has_final_page_fields(&self) -> bool {
        self.paragraphs.iter().any(|p| p.toc_target.is_some())
            || self.sections.iter().any(|section| {
                [&section.headers, &section.footers].iter().any(|hf| {
                    hf.non_empty()
                        .any(|(_, header)| header.text.contains("{pages}"))
                })
            })
    }

    /// True when `page`'s content hash can change once the total page count is final: it
    /// shows a `{pages}` header or footer, or a table-of-contents entry whose target page
    /// was not known while the background pass was still running. Other pages keep their
    /// hash, so the UI does not need to re-hash the whole document at completion.
    pub fn page_hash_needs_final(&self, page: usize) -> bool {
        let Some(pg) = self.pages.get(page) else {
            return false;
        };
        if pg.slices.iter().any(|s| {
            self.paragraphs
                .get(s.para)
                .is_some_and(|p| p.toc_target.is_some())
        }) {
            return true;
        }
        [HfKind::Header, HfKind::Footer].iter().any(|kind| {
            self.furniture_for_section(pg.section, *kind)
                .for_page(page, pg.first_in_section)
                .is_some_and(|(_, hf)| hf.text.contains("{pages}"))
        })
    }

    /// Hash of everything that determines a page's pixels apart from zoom: the page's
    /// slices plus the version, id and width of each paragraph it shows. Used as the page
    /// bitmap cache key (PLAN §4.4).
    pub fn page_content_hash(&self, page: usize) -> Option<u64> {
        let pg = self.pages.get(page)?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let geo = pg.geometry;
        geo.width_pt.to_bits().hash(&mut h);
        geo.height_pt.to_bits().hash(&mut h);
        geo.content_width().to_bits().hash(&mut h);
        geo.content_height().to_bits().hash(&mut h);
        geo.margins.top.to_bits().hash(&mut h);
        geo.margins.bottom.to_bits().hash(&mut h);
        let section = self.section(pg.section);
        section.columns.hash(&mut h);
        section.column_gutter_pt.to_bits().hash(&mut h);
        for s in &pg.slices {
            s.para.hash(&mut h);
            s.line_start.hash(&mut h);
            s.line_end.hash(&mut h);
            s.y.to_bits().hash(&mut h);
            s.col.hash(&mut h);
            s.x.to_bits().hash(&mut h);
            if let Some(p) = self.paragraphs.get(s.para) {
                p.id.hash(&mut h);
                p.version.hash(&mut h);
                // A TOC entry shows another page's number: repaint when that moves.
                if let Some(target) = p.toc_target {
                    self.page_number_of_block(target).hash(&mut h);
                }
                p.list_label.hash(&mut h);
            }
        }
        for t in &pg.tables {
            t.table.hash(&mut h);
            t.row.hash(&mut h);
            t.header.hash(&mut h);
            t.x.to_bits().hash(&mut h);
            t.y.to_bits().hash(&mut h);
            t.width.to_bits().hash(&mut h);
            t.height.to_bits().hash(&mut h);
            t.borders.width_pt.to_bits().hash(&mut h);
            t.borders.colour.hash(&mut h);
            t.borders.outer.hash(&mut h);
            t.borders.inner.hash(&mut h);
            t.bottom.hash(&mut h);
            t.sides.hash(&mut h);
            for cell in &t.cells {
                cell.x.to_bits().hash(&mut h);
                cell.width.to_bits().hash(&mut h);
                cell.fill.hash(&mut h);
                cell.top.hash(&mut h);
                cell.left.hash(&mut h);
            }
        }
        for note in &pg.notes {
            note.number.hash(&mut h);
            note.text.hash(&mut h);
        }
        self.section(pg.section).watermark.hash(&mut h);
        self.section(pg.section).page_colour.hash(&mut h);
        pg.endnotes.hash(&mut h);
        if !self.pageless {
            self.numbering.hash(&mut h);
        }
        pg.number.hash(&mut h);
        pg.number_shown.hash(&mut h);
        // Header and footer text, with this page's fields filled in.
        for kind in [HfKind::Header, HfKind::Footer] {
            if let Some((variant, hf)) = self.furniture_on(page, kind) {
                variant.hash(&mut h);
                hf.align.hash(&mut h);
                hf.resolve(pg.number, self.total_pages()).hash(&mut h);
                self.section(pg.section)
                    .header_footer_distance(kind)
                    .to_bits()
                    .hash(&mut h);
            }
        }
        Some(h.finish())
    }

    /// Page number of the page where the paragraph with model id `id` begins, once it is
    /// paginated. O(paragraphs) scan; only TOC pages ask, a few dozen times per render.
    pub fn page_number_of_block(&self, id: u64) -> Option<u32> {
        let para = self.paragraphs.iter().position(|p| p.id == id)?;
        if self.resume.is_some_and(|r| para >= r.para) {
            return None;
        }
        self.pages.get(self.page_of_para(para)).map(|p| p.number)
    }

    /// Index of the page where the paragraph with model id `id` begins, once it is
    /// paginated (the page's position, not its printed number). O(paragraphs) scan.
    pub fn page_index_of_block(&self, id: u64) -> Option<usize> {
        let para = self.paragraphs.iter().position(|p| p.id == id)?;
        if self.resume.is_some_and(|r| para >= r.para) {
            return None;
        }
        Some(self.page_of_para(para))
    }

    /// The clickable areas of the generated contents entries on page `page`: each entry's
    /// box (page points from the top-left) and the index of the page its heading is on.
    pub fn page_toc_links(&self, page: usize) -> Vec<TocLink> {
        let Some(pg) = self.pages.get(page) else {
            return Vec::new();
        };
        let geo = pg.geometry;
        let mut out = Vec::new();
        for s in &pg.slices {
            let Some(target) = self.paragraphs.get(s.para).and_then(|p| p.toc_target) else {
                continue;
            };
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let (Some(first), Some(last)) = (
                pl.lines.get(s.line_start),
                s.line_end.checked_sub(1).and_then(|i| pl.lines.get(i)),
            ) else {
                continue;
            };
            let Some(target_page) = self.page_index_of_block(target) else {
                continue;
            };
            out.push(TocLink {
                x: geo.margins.left + s.x,
                y: geo.margins.top + s.y,
                width: self.column_width(pg.section),
                height: (last.top + last.height) - first.top,
                target_page,
            });
        }
        out
    }

    /// The background colour of page `page` (straight RGBA), if its section has one. `None`
    /// is white paper.
    pub fn page_colour(&self, page: usize) -> Option<[u8; 4]> {
        let pg = self.pages.get(page)?;
        self.section(pg.section).page_colour
    }

    /// The watermark of page `page`, if its section has one: the text sized to span most of
    /// the page (along the diagonal when the watermark is diagonal) and centred on it.
    pub fn page_watermark(&self, page: usize) -> Option<WatermarkDraw> {
        let pg = self.pages.get(page)?;
        let watermark = self.section(pg.section).watermark.as_ref()?;
        if watermark.text.is_empty() {
            return None;
        }
        let geo = pg.geometry;
        let (width, height) = (geo.width_pt, geo.height_pt);
        let (span, angle_deg) = if watermark.diagonal {
            (
                (width * width + height * height).sqrt() * 0.62,
                -(height.atan2(width).to_degrees()),
            )
        } else {
            (width * 0.8, 0.0)
        };
        let sized = |id: u64, size: f32| {
            let mut snap = self.furniture_snapshot(id, &watermark.text, Align::Start);
            let colour = [0, 0, 0, 255];
            snap.base = Some(SpanStyle {
                weight: libertum_model::BOLD,
                color: colour,
                ..SpanStyle::new(size)
            });
            snap.version = {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                watermark.text.hash(&mut h);
                size.to_bits().hash(&mut h);
                h.finish()
            };
            lock(&self.engine).full(&snap, f32::MAX / 4.0)
        };
        let measured = sized(9_800_000, WATERMARK_MEASURE_PT);
        let measured_width = measured.layout.width().max(1.0);
        let size = (WATERMARK_MEASURE_PT * span / measured_width)
            .clamp(WATERMARK_MIN_PT, WATERMARK_MAX_PT);
        // Whole points: the same text gets the same layout on every page of one size.
        let layout = sized(9_800_001, size.round());
        Some(WatermarkDraw {
            layout,
            text: watermark.text.clone(),
            centre_x: width * 0.5,
            centre_y: height * 0.5,
            angle_deg,
            colour: WATERMARK_COLOUR,
        })
    }

    /// Page furniture for page `page`: its page number (if its section shows them) and the
    /// page numbers and dot leaders of any table-of-contents entries on it.
    pub fn page_decorations(&self, page: usize) -> Vec<Decoration> {
        let Some(pg) = self.pages.get(page) else {
            return Vec::new();
        };
        let geo = pg.geometry;
        // Headers, footers and page numbers are drawn on the sheet itself, upright, even when
        // the text is vertical: worked out in the sheet's own geometry.
        let on_sheet = self.is_vertical();
        let sheet = if on_sheet { geo.untransposed() } else { geo };
        let sheet_right = sheet.margins.left + sheet.content_width();
        let mut out = Vec::new();
        // Hyphens at the end of lines broken inside a word.
        for s in &pg.slices {
            let Some(p) = self.paragraphs.get(s.para) else {
                continue;
            };
            if !p.hyphenate || p.upright {
                continue;
            }
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let Some(first) = pl.lines.get(s.line_start) else {
                continue;
            };
            for &li in pl
                .hyphen_lines
                .iter()
                .filter(|&&li| s.line_start <= li && li < s.line_end)
            {
                let Some(line) = pl.layout.get(li) else {
                    continue;
                };
                let m = line.metrics();
                // The style of the text just before the break.
                let end = line.text_range().end;
                let style = p
                    .spans
                    .iter()
                    .find(|(r, _)| r.start < end && end <= r.end)
                    .map(|(_, st)| st)
                    .or(p.base.as_ref());
                let hyphen = lock(&self.engine).marker("-", style);
                let hyphen_baseline = hyphen.lines.first().map_or(0.0, |l| l.baseline);
                let baseline = geo.margins.top + s.y + (m.baseline - first.top);
                out.push(Decoration::Text {
                    text: "-".to_owned(),
                    x: geo.margins.left + s.x + pl.x_offset + line_ink_end(&line),
                    y: baseline - hyphen_baseline,
                    layout: hyphen,
                    sheet: false,
                });
            }
        }
        // Inline images, at the inline box layout gave them.
        for s in &pg.slices {
            if self
                .paragraphs
                .get(s.para)
                .is_none_or(|p| p.images.is_empty())
            {
                continue;
            }
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let Some(first) = pl.lines.get(s.line_start) else {
                continue;
            };
            for image in pl
                .images
                .iter()
                .filter(|image| s.line_start <= image.line && image.line < s.line_end)
            {
                out.push(Decoration::Image {
                    media: image.media.clone(),
                    x: geo.margins.left + s.x + pl.x_offset + image.x,
                    y: geo.margins.top + s.y + (image.y - first.top),
                    width: image.width,
                    height: image.height,
                });
            }
        }
        // Ruby: each reading over the start of its base, on the line the base is on. The
        // readings of an upright table cell are on the sheet, like its text.
        for s in &pg.slices {
            let Some(p) = self.paragraphs.get(s.para) else {
                continue;
            };
            if p.ruby.is_empty() {
                continue;
            }
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let Some(first) = pl.lines.get(s.line_start) else {
                continue;
            };
            let frame = if s.upright { geo.untransposed() } else { geo };
            for r in pl
                .ruby
                .iter()
                .filter(|r| s.line_start <= r.line && r.line < s.line_end)
            {
                out.push(Decoration::Text {
                    text: r.layout.text.clone(),
                    x: frame.margins.left + s.x + pl.x_offset + r.x,
                    y: frame.margins.top + s.y + (r.y - first.top),
                    layout: r.layout.clone(),
                    sheet: s.upright,
                });
            }
        }
        // List markers: in the hanging indent, on the item's first line.
        for s in pg.slices.iter().filter(|s| s.line_start == 0) {
            let Some(p) = self.paragraphs.get(s.para) else {
                continue;
            };
            let Some(label) = &p.list_label else {
                continue;
            };
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let Some(first) = pl.lines.first() else {
                continue;
            };
            let marker = lock(&self.engine).marker(label, p.base.as_ref());
            let marker_baseline = marker.lines.first().map_or(0.0, |l| l.baseline);
            let baseline = geo.margins.top + s.y + first.baseline - first.top;
            // The marker sits in the hanging gap left of the text, right after its start.
            let x = (pl.x_offset + p.indent_first_pt - libertum_model::LIST_HANGING_PT).max(0.0);
            out.push(Decoration::Text {
                text: label.clone(),
                x: geo.margins.left + s.x + x,
                y: baseline - marker_baseline,
                layout: marker,
                sheet: false,
            });
        }
        for s in &pg.slices {
            let Some(target) = self.paragraphs.get(s.para).and_then(|p| p.toc_target) else {
                continue;
            };
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            // The number goes on the entry's last line, if that line is on this page.
            if s.line_end != pl.lines.len() {
                continue;
            }
            let Some(number) = self.page_number_of_block(target) else {
                continue;
            };
            let last = s.line_end - 1;
            let (Some(line), Some(first)) = (pl.layout.get(last), pl.lines.get(s.line_start))
            else {
                continue;
            };
            let m = line.metrics();
            let baseline = geo.margins.top + s.y + (m.baseline - first.top);
            let label = lock(&self.engine).label(&number.to_string());
            let label_baseline = label.lines.first().map_or(0.0, |l| l.baseline);
            let right = geo.margins.left + s.x + self.column_width(pg.section);
            let x = right - label.layout.width();
            let text_end = geo.margins.left + s.x + pl.x_offset + m.offset + m.advance;
            let (x0, x1) = (text_end + LEADER_GAP_PT, x - LEADER_GAP_PT);
            if x1 - x0 >= LEADER_PITCH_PT {
                out.push(Decoration::Leader { x0, x1, baseline });
            }
            out.push(Decoration::Text {
                layout: label,
                text: number.to_string(),
                x,
                y: baseline - label_baseline,
                sheet: false,
            });
        }
        // Paragraph borders: a line on each chosen side, `space_pt` outside the text. A
        // paragraph split across pages opens on its first page and closes on its last.
        for s in &pg.slices {
            let Some(p) = self.paragraphs.get(s.para) else {
                continue;
            };
            let Some(b) = p.borders.filter(|b| b.is_visible()) else {
                continue;
            };
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let line_count = pl.lines.len();
            if line_count == 0 || s.line_start >= line_count || s.line_end == 0 {
                continue;
            }
            let m = lock(&self.engine).metrics(p, self.column_width(pg.section));
            let from = s.line_start.min(m.heights.len());
            let to = s.line_end.min(m.heights.len());
            let slice_height: f32 = m.heights.get(from..to).map_or(0.0, |hs| hs.iter().sum());
            let space = b.space_pt.max(0.0);
            let width = b.width_pt.max(0.25);
            let text_left = geo.margins.left + s.x + pl.x_offset;
            let text_right = text_left + pl.width;
            let text_top = geo.margins.top + s.y;
            let text_bottom = text_top + slice_height;
            let left = text_left - space;
            let right = text_right + space;
            let top = text_top - space;
            let bottom = text_bottom + space;
            let opens = s.line_start == 0;
            let closes = s.line_end >= line_count;
            let box_width = right - left + width * 2.0;
            if b.sides.top && opens {
                out.push(Decoration::Rect {
                    x: left - width,
                    y: top - width,
                    width: box_width,
                    height: width,
                    colour: b.colour,
                });
            }
            if b.sides.bottom && closes {
                out.push(Decoration::Rect {
                    x: left - width,
                    y: bottom,
                    width: box_width,
                    height: width,
                    colour: b.colour,
                });
            }
            let side_top = if b.sides.top && opens {
                top - width
            } else {
                top
            };
            let side_bottom = if b.sides.bottom && closes {
                bottom + width
            } else {
                bottom
            };
            let side_height = (side_bottom - side_top).max(width);
            if b.sides.left {
                out.push(Decoration::Rect {
                    x: left - width,
                    y: side_top,
                    width,
                    height: side_height,
                    colour: b.colour,
                });
            }
            if b.sides.right {
                out.push(Decoration::Rect {
                    x: right,
                    y: side_top,
                    width,
                    height: side_height,
                    colour: b.colour,
                });
            }
        }
        if !self.pageless && self.numbering.show && pg.number_shown {
            let hidden = self.numbering.hide_first_page && page == 0;
            if !hidden {
                let pos = self.numbering.position;
                let label = lock(&self.engine).label(&pg.number.to_string());
                let (w, h) = (label.layout.width(), label.height);
                // Book style: the chosen side on pages 1, 3, 5..., mirrored on 2, 4, 6...
                let horizontal = if pos.mirror && page % 2 == 1 {
                    match pos.horizontal {
                        PageNumberHorizontal::Left => PageNumberHorizontal::Right,
                        PageNumberHorizontal::Right => PageNumberHorizontal::Left,
                        PageNumberHorizontal::Centre => PageNumberHorizontal::Centre,
                    }
                } else {
                    pos.horizontal
                };
                let x = match horizontal {
                    PageNumberHorizontal::Left => sheet.margins.left,
                    PageNumberHorizontal::Centre => (sheet.width_pt - w) * 0.5,
                    PageNumberHorizontal::Right => sheet_right - w,
                };
                // Centred in the page setup's margin, not the insets a taller header or
                // footer may add, so the number keeps its place when header text grows.
                let base = self.base_geometry;
                let y = match pos.vertical {
                    PageNumberVertical::Top => base.margins.top * 0.5 - h * 0.5,
                    PageNumberVertical::Bottom => {
                        base.height_pt - base.margins.bottom * 0.5 - h * 0.5
                    }
                };
                out.push(Decoration::Text {
                    layout: label,
                    text: pg.number.to_string(),
                    x,
                    y,
                    sheet: on_sheet,
                });
            }
        }
        // Header and footer text: in the margin, at the section's distance from the page
        // edge, aligned across the text area. A pageless view draws none of it.
        for kind in [HfKind::Header, HfKind::Footer]
            .into_iter()
            .filter(|_| !self.pageless)
        {
            let Some((variant, hf)) = self.furniture_on(page, kind) else {
                continue;
            };
            if hf.is_empty() {
                continue;
            }
            let text = hf.resolve(pg.number, self.total_pages());
            if text.is_empty() {
                continue;
            }
            let id = self.furniture_id(pg.section, kind, variant);
            let snap = self.furniture_snapshot(id, &text, hf.align);
            let layout = lock(&self.engine).full(&snap, sheet.content_width());
            let distance = self.section(pg.section).header_footer_distance(kind);
            let y = match kind {
                HfKind::Header => distance,
                HfKind::Footer => sheet.height_pt - distance - layout.height,
            };
            out.push(Decoration::Text {
                layout,
                text,
                x: sheet.margins.left,
                y,
                sheet: on_sheet,
            });
        }
        // The endnote section: heading and notes from the top of its own page.
        if pg.endnotes {
            let width = geo.content_width();
            let heading = self.text_layout(9_700_000, "Endnotes", SpanStyle::new(14.0), width);
            let mut y = geo.margins.top;
            let heading_height = heading.height;
            out.push(Decoration::Text {
                layout: heading,
                text: "Endnotes".to_owned(),
                x: geo.margins.left,
                y,
                sheet: false,
            });
            y += heading_height + 8.0;
            for note in &pg.notes {
                let text = format!("{}. {}", note.number, note.text);
                let layout = self.text_layout(
                    9_000_000 + u64::from(note.number),
                    &text,
                    SpanStyle::new(9.0),
                    width,
                );
                let height = layout.height;
                out.push(Decoration::Text {
                    layout,
                    text,
                    x: geo.margins.left,
                    y,
                    sheet: false,
                });
                y += height + 2.0;
            }
            return out;
        }
        // Footnote markers and the note block at the foot of the page.
        if !pg.notes.is_empty() {
            let note_width = geo.content_width();
            let note_layouts: Vec<_> = pg
                .notes
                .iter()
                .map(|note| {
                    let text = format!("{}. {}", note.number, note.text);
                    let layout = self.text_layout(
                        9_000_000 + u64::from(note.number),
                        &text,
                        SpanStyle::new(9.0),
                        note_width,
                    );
                    (text, layout)
                })
                .collect();
            let note_height: f32 = note_layouts
                .iter()
                .map(|(_, layout)| layout.height + 2.0)
                .sum::<f32>()
                + 4.0;
            let note_top = geo.margins.top + geo.content_height() - note_height;
            out.push(Decoration::Rect {
                x: geo.margins.left,
                y: note_top - 3.0,
                width: geo.content_width(),
                height: 0.5,
                colour: [120, 120, 120, 255],
            });
            let mut y = note_top + 4.0;
            for (text, layout) in note_layouts {
                let height = layout.height;
                out.push(Decoration::Text {
                    layout,
                    text,
                    x: geo.margins.left,
                    y,
                    sheet: false,
                });
                y += height + 2.0;
            }
            // Superscript reference numbers at each note anchor on this page.
            for (para_index, paragraph) in self.paragraphs.iter().enumerate() {
                for note in paragraph
                    .notes
                    .iter()
                    .filter(|note| note.kind == libertum_model::NoteKind::Footnote)
                {
                    let Some(rect) = self.caret_rect(para_index, note.offset, 0.0) else {
                        continue;
                    };
                    if rect.page != page {
                        continue;
                    }
                    let marker = self.text_layout(
                        9_500_000 + u64::from(note.number),
                        &note.number.to_string(),
                        SpanStyle::new(7.0),
                        12.0,
                    );
                    out.push(Decoration::Text {
                        layout: marker.clone(),
                        text: String::new(),
                        x: rect.x - marker.width - 1.0,
                        y: rect.y - 2.0,
                        sheet: false,
                    });
                }
            }
        }
        out
    }

    /// A small layout for footnote text and markers. Cached by id and text hash.
    fn text_layout(
        &self,
        id: u64,
        text: &str,
        style: SpanStyle,
        width: f32,
    ) -> Arc<ParagraphLayout> {
        let mut snap = self.furniture_snapshot(id, text, Align::Start);
        snap.base = Some(style);
        snap.version = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut h);
            h.finish()
        };
        lock(&self.engine).full(&snap, width)
    }

    /// Everything drawn on page `page` (each text line and each decoration) with its
    /// vertical extent in page points and a hash of what it shows and where. A horizontal
    /// strip of the page bitmap needs re-rendering exactly when the set of items that overlap
    /// it changes, which is how the app re-renders only the part of a page an edit touched.
    ///
    /// Extents are padded by [`INK_OVERHANG`] of the line height, because glyph ink can reach
    /// past the line box (descenders, accents, italics).
    pub fn page_items(&self, page: usize) -> Vec<PageItem> {
        let Some(pg) = self.pages.get(page) else {
            return Vec::new();
        };
        let geo = pg.geometry;
        let mut out = Vec::new();
        // A page colour lies under the whole page: every strip repaints when it changes.
        if let Some(colour) = self.section(pg.section).page_colour {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            colour.hash(&mut h);
            (geo.width_pt.to_bits(), geo.height_pt.to_bits()).hash(&mut h);
            out.push(PageItem {
                top: 0.0,
                bottom: geo.height_pt,
                key: h.finish(),
            });
        }
        // A watermark lies under the whole page: every strip repaints when it changes.
        if let Some(watermark) = self.section(pg.section).watermark.as_ref() {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            watermark.hash(&mut h);
            (geo.width_pt.to_bits(), geo.height_pt.to_bits()).hash(&mut h);
            out.push(PageItem {
                top: 0.0,
                bottom: geo.height_pt,
                key: h.finish(),
            });
        }
        for s in &pg.slices {
            let (Some(p), Some(pl)) = (self.paragraphs.get(s.para), self.paragraph_layout(s.para))
            else {
                continue;
            };
            let Some(first) = pl.lines.get(s.line_start) else {
                continue;
            };
            for li in s.line_start..s.line_end {
                let Some(line) = pl.lines.get(li) else {
                    continue;
                };
                let top = geo.margins.top + s.y + (line.top - first.top);
                let pad = line.height * INK_OVERHANG;
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (
                    p.id,
                    p.version,
                    li,
                    top.to_bits(),
                    (geo.margins.left + s.x + pl.x_offset).to_bits(),
                )
                    .hash(&mut h);
                // A reflow from a column or cell-width change can keep every other key
                // field equal.
                self.layout_width(s.para).to_bits().hash(&mut h);
                out.push(PageItem {
                    top: top - pad,
                    bottom: top + line.height + pad,
                    key: h.finish(),
                });
            }
        }
        for t in &pg.tables {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (
                t.table,
                t.row,
                t.header,
                t.x.to_bits(),
                t.y.to_bits(),
                t.width.to_bits(),
                t.height.to_bits(),
                (
                    t.borders.width_pt.to_bits(),
                    t.borders.colour,
                    t.borders.outer,
                    t.borders.inner,
                    t.bottom,
                    t.sides,
                ),
                t.cells
                    .iter()
                    .map(|c| (c.x.to_bits(), c.width.to_bits(), c.fill, c.top, c.left))
                    .collect::<Vec<_>>(),
            )
                .hash(&mut h);
            out.push(PageItem {
                top: geo.margins.top + t.y,
                bottom: geo.margins.top + t.y + t.height,
                key: h.finish(),
            });
        }
        for d in self.page_decorations(page) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            let (top, bottom) = match &d {
                Decoration::Text {
                    layout,
                    text,
                    x,
                    y,
                    sheet,
                } => {
                    (text, x.to_bits(), y.to_bits(), sheet).hash(&mut h);
                    let pad = layout.height * INK_OVERHANG;
                    (y - pad, y + layout.height + pad)
                }
                Decoration::Leader { x0, x1, baseline } => {
                    (x0.to_bits(), x1.to_bits(), baseline.to_bits()).hash(&mut h);
                    (baseline - 2.0, baseline + 2.0)
                }
                Decoration::Rect {
                    x,
                    y,
                    width,
                    height,
                    colour,
                } => {
                    (
                        x.to_bits(),
                        y.to_bits(),
                        width.to_bits(),
                        height.to_bits(),
                        colour,
                    )
                        .hash(&mut h);
                    (*y, *y + *height)
                }
                Decoration::Image {
                    media,
                    x,
                    y,
                    width,
                    height,
                } => {
                    (
                        media.hash(),
                        x.to_bits(),
                        y.to_bits(),
                        width.to_bits(),
                        height.to_bits(),
                    )
                        .hash(&mut h);
                    (*y, *y + *height)
                }
            };
            out.push(PageItem {
                top,
                bottom,
                key: h.finish(),
            });
        }
        out
    }

    /// Maps a point on a page (points, origin at the page's top-left) to a paragraph and
    /// byte offset. Points outside the text snap to the nearest line; a point in a gutter
    /// snaps to the nearer column.
    pub fn hit_test(&self, page: usize, x_pt: f32, y_pt: f32) -> Option<HitResult> {
        let pg = self.pages.get(page)?;
        let geo = pg.geometry;
        let cx = x_pt - geo.margins.left;
        let cy = y_pt - geo.margins.top;
        // A point on an upright table is looked up on the sheet itself: the turned layout
        // point (x, y) is at (sheet width - y, x) there.
        if pg.tables.iter().any(|t| t.upright) {
            let sheet = geo.untransposed();
            let sx = geo.height_pt - y_pt - sheet.margins.left;
            let sy = x_pt - sheet.margins.top;
            if let Some(s) = self.table_slice_at(pg, sx, sy, true) {
                return self.hit_test_slice(s, sx, sy);
            }
        }
        if let Some(s) = self.table_slice_at(pg, cx, cy, false) {
            return self.hit_test_slice(s, cx, cy);
        }
        let section = self.section(pg.section);
        let (ncols, width, gutter) = column_layout(
            geo.content_width(),
            section.columns,
            section.column_gutter_pt,
        );
        let col = if ncols <= 1 {
            0
        } else {
            ((cx + gutter * 0.5) / (width + gutter))
                .floor()
                .clamp(0.0, (ncols - 1) as f32) as usize
        };
        // The last slice of that column starting at or above cy (or its first slice). A
        // column with no text (the tail of a page) falls back to the page's last slice.
        let in_column: Vec<usize> = pg
            .slices
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.upright && s.col as usize == col)
            .map(|(i, _)| i)
            .collect();
        let idx = if in_column.is_empty() {
            pg.slices
                .iter()
                .rposition(|s| !s.upright)
                .or_else(|| pg.slices.len().checked_sub(1))?
        } else {
            in_column
                .iter()
                .copied()
                .take_while(|&i| pg.slices[i].y <= cy)
                .last()
                .unwrap_or(in_column[0])
        };
        let s = pg.slices[idx];
        self.hit_test_slice(s, cx, cy)
    }

    /// Cursor for a click at page-local (`cx`, `cy`) inside `s`.
    fn hit_test_slice(&self, s: ParaSlice, cx: f32, cy: f32) -> Option<HitResult> {
        let pl = self.paragraph_layout(s.para)?;
        let cx = cx - s.x - pl.x_offset;
        let top = pl.lines.get(s.line_start)?.top;
        let bottom = pl.lines.get(s.line_end.checked_sub(1)?)?;
        let max_y = bottom.top + bottom.height - EPS;
        let ly = (cy - s.y + top).clamp(top, max_y.max(top));
        let cursor = Cursor::from_point(&pl.layout, cx, ly);
        Some(HitResult {
            para: s.para,
            byte_offset: cursor.index(),
        })
    }

    /// The text slice under a point inside a table row (cells are side by side, so the
    /// column-based choice used for body text would pick the wrong cell).
    fn table_slice_at(
        &self,
        pg: &PageLayout,
        cx: f32,
        cy: f32,
        upright: bool,
    ) -> Option<ParaSlice> {
        for draw in pg.tables.iter().filter(|t| t.upright == upright) {
            if cy < draw.y || cy > draw.y + draw.height || cx < draw.x || cx > draw.x + draw.width {
                continue;
            }
            for cell in &draw.cells {
                if cx < cell.x - EPS || cx > cell.x + cell.width + EPS {
                    continue;
                }
                let mut best: Option<ParaSlice> = None;
                for s in pg.slices.iter().filter(|s| s.upright == upright) {
                    if s.x < cell.x - EPS || s.x > cell.x + cell.width + EPS {
                        continue;
                    }
                    if s.y <= cy + EPS && best.is_none_or(|b| s.y >= b.y) {
                        best = Some(*s);
                    }
                }
                if best.is_none() {
                    best = pg
                        .slices
                        .iter()
                        .find(|s| {
                            s.upright == upright
                                && s.x >= cell.x - EPS
                                && s.x <= cell.x + cell.width + EPS
                        })
                        .copied();
                }
                if let Some(s) = best {
                    return Some(s);
                }
            }
        }
        None
    }

    /// Caret rectangle for a paragraph/byte offset, in page coordinates.
    pub fn caret_rect(&self, para: usize, byte_offset: usize, width_pt: f32) -> Option<CaretRect> {
        let pl = self.paragraph_layout(para)?;
        let text_len = self.paragraphs.get(para)?.text.len();
        let cursor =
            Cursor::from_byte_index(&pl.layout, byte_offset.min(text_len), Affinity::Downstream);
        let g = cursor.geometry(&pl.layout, width_pt);
        let y_mid = (g.y0 + g.y1) as f32 * 0.5;
        let line = pl.lines.iter().rposition(|l| l.top <= y_mid).unwrap_or(0);
        let (page, s, top) = self.page_and_slice(para, line)?;
        let geo = self.page_geometry(page);
        // The caret is as tall as the text, not as the line: the line box includes the
        // leading that separates lines, which made the caret look oversized beside the glyphs.
        let (caret_top, caret_height) = match pl.layout.get(line) {
            Some(line) => {
                let metrics = line.metrics();
                let top = metrics.content_block_min_coord;
                (top, (metrics.content_block_max_coord - top).max(1.0))
            }
            None => (g.y0 as f32, (g.y1 - g.y0) as f32),
        };
        let (x, y, width, height) = slice_rect(
            &geo,
            s,
            (
                pl.x_offset + g.x0 as f32,
                caret_top - top,
                (g.x1 - g.x0) as f32,
                caret_height,
            ),
        );
        Some(CaretRect {
            page,
            x,
            y,
            width,
            height,
        })
    }

    /// The whole line box holding byte `byte_offset` of paragraph `para`, in page
    /// coordinates: as tall as the line with its leading, unlike [`caret_rect`](Self::caret_rect),
    /// which is only as tall as the text. Moving up or down steps by this box, so the step
    /// always reaches the neighbouring line whatever the font's proportions.
    pub fn line_rect(&self, para: usize, byte_offset: usize) -> Option<CaretRect> {
        let pl = self.paragraph_layout(para)?;
        let text_len = self.paragraphs.get(para)?.text.len();
        let cursor =
            Cursor::from_byte_index(&pl.layout, byte_offset.min(text_len), Affinity::Downstream);
        let g = cursor.geometry(&pl.layout, 0.0);
        let y_mid = (g.y0 + g.y1) as f32 * 0.5;
        let line = pl.lines.iter().rposition(|l| l.top <= y_mid).unwrap_or(0);
        let (page, s, top) = self.page_and_slice(para, line)?;
        let geo = self.page_geometry(page);
        let line_box = pl.lines.get(line)?;
        let (x, y, width, height) = slice_rect(
            &geo,
            s,
            (
                pl.x_offset + g.x0 as f32,
                line_box.top - top,
                0.0,
                line_box.height,
            ),
        );
        Some(CaretRect {
            page,
            x,
            y,
            width,
            height,
        })
    }

    /// The middle (page points) of the line on page `page` just below (`direction` > 0) or
    /// just above the line spanning `line_top..line_bottom`, preferring lines whose column
    /// holds `x` (so moving down in a table cell stays in its column). `None` when there is
    /// no such line on the page: the caller moves on to the next or previous page.
    pub fn neighbour_line_y(
        &self,
        page: usize,
        x: f32,
        line_top: f32,
        line_bottom: f32,
        direction: i32,
        upright: bool,
    ) -> Option<f32> {
        let pg = self.pages.get(page)?;
        // Upright slices are on the sheet, so their neighbours are looked for there.
        let geo = if upright {
            pg.geometry.untransposed()
        } else {
            pg.geometry
        };
        // (holds x, distance from the current line, middle)
        let mut best: Option<(bool, f32, f32)> = None;
        for s in pg.slices.iter().filter(|s| s.upright == upright) {
            let Some(pl) = self.paragraph_layout(s.para) else {
                continue;
            };
            let Some(first) = pl.lines.get(s.line_start) else {
                continue;
            };
            let left = geo.margins.left + s.x + pl.x_offset;
            let holds_x = x >= left - 1.0 && x <= left + pl.width + 1.0;
            for li in s.line_start..s.line_end {
                let Some(lb) = pl.lines.get(li) else {
                    continue;
                };
                let top = geo.margins.top + s.y + (lb.top - first.top);
                let bottom = top + lb.height;
                let distance = if direction > 0 {
                    top - line_bottom
                } else {
                    line_top - bottom
                };
                if distance < -EPS * 10.0 {
                    continue;
                }
                let candidate = (holds_x, distance, top + lb.height * 0.5);
                best = match best {
                    None => Some(candidate),
                    Some(current) => {
                        let better = (candidate.0 && !current.0)
                            || (candidate.0 == current.0 && candidate.1 < current.1);
                        Some(if better { candidate } else { current })
                    }
                };
            }
        }
        best.map(|(_, _, middle)| middle)
    }

    /// Selection highlight rectangles for a byte range in one paragraph, in page coordinates.
    /// A selection spanning blocks is drawn by calling this once per paragraph (PLAN §4.4:
    /// selection is an overlay, never baked into the page bitmap).
    pub fn selection_rects(
        &self,
        para: usize,
        range: std::ops::Range<usize>,
    ) -> Vec<SelectionRect> {
        let Some(pl) = self.paragraph_layout(para) else {
            return Vec::new();
        };
        let Some(p) = self.paragraphs.get(para) else {
            return Vec::new();
        };
        let start = range.start.min(p.text.len());
        let end = range.end.min(p.text.len());
        if start >= end {
            return Vec::new();
        }
        let selection = parley::editing::Selection::new(
            Cursor::from_byte_index(&pl.layout, start, Affinity::Downstream),
            Cursor::from_byte_index(&pl.layout, end, Affinity::Downstream),
        );
        let mut out = Vec::new();
        for (rect, line) in selection.geometry(&pl.layout) {
            let Some((page, s, top)) = self.page_and_slice(para, line) else {
                continue;
            };
            let width = (rect.x1 - rect.x0) as f32;
            let height = (rect.y1 - rect.y0) as f32;
            if width <= EPS || height <= EPS {
                continue;
            }
            let geo = self.page_geometry(page);
            let (x, y, width, height) = slice_rect(
                &geo,
                s,
                (
                    pl.x_offset + rect.x0 as f32,
                    rect.y0 as f32 - top,
                    width,
                    height,
                ),
            );
            out.push(SelectionRect {
                page,
                x,
                y,
                width,
                height,
            });
        }
        out
    }

    /// Whether paragraph `index` is in a table kept upright on a vertical page.
    pub fn paragraph_is_upright(&self, index: usize) -> bool {
        self.paragraphs.get(index).is_some_and(|p| p.upright)
    }

    /// Page index, page slice and the slice's first-line top for a paragraph line.
    fn page_and_slice(&self, para: usize, line: usize) -> Option<(usize, &ParaSlice, f32)> {
        let pl = self.paragraph_layout(para)?;
        let Some(&row_first) = self.row_first_of.get(&para) else {
            let page = self
                .pages
                .partition_point(|p| p.start <= FlowPos { para, line })
                .checked_sub(1)?;
            let s = self.pages[page]
                .slices
                .iter()
                .find(|s| s.para == para && s.line_start <= line && line < s.line_end)?;
            let top = pl.lines.get(s.line_start)?.top;
            return Some((page, s, top));
        };
        // A cell paragraph: its row starts on one page and may continue on the next ones
        // (a split row), and a repeated header row is drawn on later pages too. The line is
        // in the flow on exactly one of the pages the row occupies.
        let first_page = self.page_of_para(row_first);
        for (page, pg) in self.pages.iter().enumerate().skip(first_page) {
            if page > first_page && pg.start.para != row_first {
                break;
            }
            if let Some(s) = pg
                .slices
                .iter()
                .find(|s| s.para == para && s.line_start <= line && line < s.line_end)
            {
                let top = pl.lines.get(s.line_start)?.top;
                return Some((page, s, top));
            }
        }
        None
    }
}

/// A rectangle given relative to a slice's text (x along the line, y down from the slice's
/// first line), as it lies on the layout page `geo`. A slice of an upright table is on the
/// sheet, so its rectangle is turned back the other way: the sheet point (x, y) is the layout
/// point (y, sheet width - x), which the app turns to the sheet again.
fn slice_rect(
    geo: &PageGeometry,
    s: &ParaSlice,
    (x, y, w, h): (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    if !s.upright {
        return (geo.margins.left + s.x + x, geo.margins.top + s.y + y, w, h);
    }
    let sheet = geo.untransposed();
    let sx = sheet.margins.left + s.x + x;
    let sy = sheet.margins.top + s.y + y;
    (sy, sheet.width_pt - sx - w, h, w)
}

/// Where a line's text ends, measured from the positioned glyph runs: justified lines are
/// wider than their metrics' advance, which is taken before justification.
/// The paper and orientation closest to a geometry, used when only a geometry is given.
fn page_setup_from_geometry(geometry: PageGeometry) -> libertum_model::PageSetup {
    use libertum_model::{Orientation, PageSetup, PaperSize};
    let landscape = geometry.width_pt > geometry.height_pt;
    let (long, short) = if landscape {
        (geometry.width_pt, geometry.height_pt)
    } else {
        (geometry.height_pt, geometry.width_pt)
    };
    let mut best = PaperSize::A4;
    let mut best_delta = f32::MAX;
    for paper in PaperSize::ALL {
        let (w, h) = paper.points();
        let (paper_long, paper_short) = if w >= h { (w, h) } else { (h, w) };
        let delta = (paper_long - long).abs() + (paper_short - short).abs();
        if delta < best_delta {
            best_delta = delta;
            best = paper;
        }
    }
    PageSetup {
        paper: best,
        orientation: if landscape {
            Orientation::Landscape
        } else {
            Orientation::Portrait
        },
        margins: libertum_model::Margins {
            top: geometry.margins.top,
            right: geometry.margins.right,
            bottom: geometry.margins.bottom,
            left: geometry.margins.left,
        },
    }
}

/// The set a section shows, following "Link to Previous" backwards.
fn linked_furniture(sections: &[Section], index: usize, kind: HfKind) -> &HeadersFooters {
    const EMPTY: HeadersFooters = HeadersFooters {
        default: None,
        first: None,
        even: None,
        different_first: false,
        different_even_odd: false,
    };
    if sections.is_empty() {
        return &EMPTY;
    }
    let mut at = index.min(sections.len() - 1);
    loop {
        let section = &sections[at];
        if at == 0 || !section.header_footer_linked(kind) {
            return section.header_footer(kind);
        }
        at -= 1;
    }
}

fn line_ink_end(line: &parley::layout::Line<'_, crate::engine::Brush>) -> f32 {
    line.items()
        .filter_map(|item| match item {
            parley::PositionedLayoutItem::GlyphRun(run) => run
                .positioned_glyphs()
                .map(|g| g.x + g.advance)
                .reduce(f32::max),
            _ => None,
        })
        .fold(0.0, f32::max)
}

/// Builds one page starting at `start`; returns it and the start of the next page
/// (`para == paras.len()` when the document ends).
///
/// Pagination rules, as in Word: a paragraph with *keep together* moves whole to the next
/// column rather than split; *widow control* never leaves its first line alone at the bottom
/// of a column or its last line alone at the top of the next; *keep with next* moves a
/// paragraph (a heading, typically) to the next column when the paragraph after it does not
/// start in this one. A page break or section break always starts a new page, even when the
/// section has several columns. Every rule gives way when obeying it would leave the column
/// empty, so pagination always advances.
/// Builds the table snapshots for a document, with cell paragraph indices in the layout's
/// flat paragraph order.
pub fn table_snapshots(doc: &Document) -> Vec<TableSnapshot> {
    doc.body()
        .iter()
        .filter_map(|block| block.as_table())
        .map(|table| {
            // Fill of the cell that starts a vertical merge, by grid column, so its
            // continuation cells below are painted the same.
            let mut origin_fill: Vec<Option<[u8; 4]>> = vec![None; table.columns().max(1)];
            TableSnapshot {
                id: table.id.0,
                widths_pt: table.widths_pt.clone(),
                header_rows: table.header_rows,
                borders: table.borders,
                rows: table
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(row_index, row)| {
                        let mut col = 0usize;
                        let cells = row
                            .cells
                            .iter()
                            .map(|cell| {
                                let span = usize::from(cell.col_span.max(1));
                                let paragraphs = cell
                                    .paragraphs
                                    .iter()
                                    .filter_map(|p| doc.index_of(p.id))
                                    .collect();
                                // The first row cannot continue anything.
                                let merged_above = cell.merged_into_above && row_index > 0;
                                let fill = if merged_above {
                                    origin_fill.get(col).copied().flatten()
                                } else {
                                    table.resolved_fill(row_index, cell)
                                };
                                if let Some(slot) = origin_fill.get_mut(col) {
                                    *slot = fill;
                                }
                                let snapshot = CellSnapshot {
                                    col,
                                    span,
                                    fill,
                                    paragraphs,
                                    merged_above,
                                };
                                col += span;
                                snapshot
                            })
                            .collect();
                        RowSnapshot {
                            cells,
                            min_height_pt: row.min_height_pt,
                            can_split: !row.keep_together,
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

/// One row (or one piece of a split row) laid out for placement.
struct LaidRow {
    height: f32,
    slices: Vec<ParaSlice>,
    draw: TableDraw,
    /// `Some(consumed)` when lines of the row remain for the next column or page: how much
    /// of the row's content height has been placed so far.
    rest: Option<f32>,
    /// False when nothing of the row fitted in the room given.
    placed_any: bool,
}

/// Tolerance for comparing positions inside a row, points. Wider than [`EPS`] because the
/// consumed height of a split row travels through [`FlowPos::line`] in 1/64 pt steps.
const ROW_EPS: f32 = 0.02;

/// Encodes how much of a split row has been placed as a flow position line (never 0, which
/// means "the row starts here").
fn encode_consumed(consumed: f32) -> usize {
    (consumed.max(0.0) * 64.0).round() as usize + 1
}

/// The consumed height a flow position line stands for (0 at the start of a row).
fn decode_consumed(line: usize) -> f32 {
    line.saturating_sub(1) as f32 / 64.0
}

/// One line of a cell in the row's own coordinates (0 = top of the cell's text).
struct CellLine {
    para: usize,
    line: usize,
    top: f32,
    /// Bottom of the line, including the paragraph's space after on its last line.
    bottom: f32,
}

/// Lays out one table row at `y`: every cell at its (scaled) column width, side by side.
///
/// `consumed` is how much of the row's content earlier pieces placed (0 for a fresh row).
/// With `room: None` everything left is placed. With `room: Some(height)` only the lines
/// that fit in that height are, and `rest` says where the next piece continues; `force`
/// places at least one line when nothing fits (an empty column must take something).
#[allow(clippy::too_many_arguments)]
fn layout_table_row(
    paras: &[ParagraphSnapshot],
    eng: &mut Engine,
    table: &TableSnapshot,
    row_index: usize,
    x: f32,
    column_width: f32,
    y: f32,
    col: u16,
    header: bool,
    consumed: f32,
    room: Option<f32>,
    force: bool,
) -> LaidRow {
    let total: f32 = table.widths_pt.iter().sum();
    let scale = if total > column_width && total > 0.0 {
        column_width / total
    } else {
        1.0
    };
    let margin = DEFAULT_CELL_MARGIN_PT * scale;
    let last_row = row_index + 1 >= table.rows.len();
    let empty_draw = |cells: Vec<CellDraw>, width: f32, height: f32| TableDraw {
        table: table.id,
        row: row_index,
        x,
        y,
        width,
        height,
        cells,
        borders: table.borders,
        header,
        bottom: last_row && table.borders.outer,
        sides: table.borders.outer,
        upright: false,
    };
    let Some(row) = table.rows.get(row_index) else {
        return LaidRow {
            height: 0.0,
            slices: Vec::new(),
            draw: empty_draw(Vec::new(), 0.0, 0.0),
            rest: None,
            placed_any: true,
        };
    };
    // Every cell's lines in row coordinates.
    let mut cell_lines: Vec<Vec<CellLine>> = Vec::with_capacity(row.cells.len());
    let mut geometry: Vec<(f32, f32)> = Vec::with_capacity(row.cells.len());
    for cell in &row.cells {
        let cell_x = x + table.widths_pt.iter().take(cell.col).sum::<f32>() * scale;
        let cell_width = table
            .widths_pt
            .iter()
            .skip(cell.col)
            .take(cell.span.max(1))
            .sum::<f32>()
            * scale;
        geometry.push((cell_x, cell_width));
        let mut lines = Vec::new();
        if !cell.merged_above {
            let text_width = (cell_width - 2.0 * margin).max(1.0);
            let mut cy = 0.0f32;
            let mut first_paragraph = true;
            for &index in &cell.paragraphs {
                let Some(para) = paras.get(index) else {
                    continue;
                };
                if !first_paragraph {
                    cy += para.space_before_pt;
                }
                let m = eng.metrics(para, text_width);
                let count = m.heights.len();
                for (line, height) in m.heights.iter().enumerate() {
                    let top = cy;
                    cy += height;
                    let after = if line + 1 == count {
                        para.space_after_pt
                    } else {
                        0.0
                    };
                    lines.push(CellLine {
                        para: index,
                        line,
                        top,
                        bottom: cy + after,
                    });
                }
                cy += para.space_after_pt;
                first_paragraph = false;
            }
        }
        cell_lines.push(lines);
    }
    // The cut: lines whose bottom is at or above `limit` belong to this piece.
    fn remaining(lines: &[CellLine], consumed: f32) -> impl Iterator<Item = &CellLine> {
        lines.iter().filter(move |l| l.bottom > consumed + ROW_EPS)
    }
    let mut limit = match room {
        None => f32::MAX,
        Some(room) => consumed + (room - 2.0 * margin).max(0.0),
    };
    let fits = |limit: f32| {
        cell_lines
            .iter()
            .any(|lines| remaining(lines, consumed).any(|l| l.bottom <= limit + ROW_EPS))
    };
    let anything_left = cell_lines
        .iter()
        .any(|lines| remaining(lines, consumed).next().is_some());
    if room.is_some() && anything_left && !fits(limit) {
        if !force {
            return LaidRow {
                height: 0.0,
                slices: Vec::new(),
                draw: empty_draw(Vec::new(), total * scale, 0.0),
                rest: Some(consumed),
                placed_any: false,
            };
        }
        // Take the first line that ends soonest, whatever its height.
        limit = cell_lines
            .iter()
            .filter_map(|lines| remaining(lines, consumed).next().map(|l| l.bottom))
            .fold(f32::MAX, f32::min);
    }
    let mut slices = Vec::new();
    let mut cells = Vec::new();
    let mut content = 0.0f32;
    let mut placed_to = consumed;
    let mut more = false;
    for (index, cell) in row.cells.iter().enumerate() {
        let (cell_x, cell_width) = geometry[index];
        let lines = &cell_lines[index];
        let mut origin = None;
        let mut open: Option<ParaSlice> = None;
        for line in remaining(lines, consumed) {
            if line.bottom > limit + ROW_EPS {
                more = true;
                break;
            }
            let origin = *origin.get_or_insert(line.top);
            let line_y = y + margin + (line.top - origin);
            match open.as_mut() {
                Some(s) if s.para == line.para && s.line_end == line.line => {
                    s.line_end = line.line + 1;
                }
                _ => {
                    if let Some(done) = open.take() {
                        slices.push(done);
                    }
                    open = Some(ParaSlice {
                        para: line.para,
                        line_start: line.line,
                        line_end: line.line + 1,
                        y: line_y,
                        col,
                        x: cell_x + margin,
                        upright: false,
                    });
                }
            }
            content = content.max(line.bottom - origin);
            placed_to = placed_to.max(line.bottom);
        }
        if let Some(done) = open.take() {
            slices.push(done);
        }
        cells.push(CellDraw {
            x: cell_x,
            width: cell_width,
            fill: cell.fill,
            top: true,
            left: index > 0 && table.borders.inner,
        });
    }
    let mut height = content + 2.0 * margin;
    if !more && consumed <= ROW_EPS {
        height = height.max(row.min_height_pt * scale);
    }
    LaidRow {
        height,
        slices,
        draw: empty_draw(cells, total * scale, height),
        rest: more.then_some(placed_to),
        placed_any: true,
    }
}

/// Decides which lines each table piece in one column draws: the top line of every cell and
/// the bottom line of the piece. `draws` are the pieces of one column in order.
///
/// A row's top line is the table's outer line on its first row and wherever the table starts
/// a column (a continuation page); otherwise it is an inner line, left out above a cell that
/// continues the cell above it. The bottom line closes the table's last row and the last
/// piece in the column.
fn finish_table_lines(draws: &mut [TableDraw], tables: &[TableSnapshot]) {
    let count = draws.len();
    for index in 0..count {
        let opens = index == 0 || draws[index - 1].table != draws[index].table;
        let closes = index + 1 == count || draws[index + 1].table != draws[index].table;
        let draw = &mut draws[index];
        let Some(table) = tables.iter().find(|t| t.id == draw.table) else {
            continue;
        };
        let borders = draw.borders;
        let last_row = draw.row + 1 >= table.rows.len();
        let row = table.rows.get(draw.row);
        for (cell_index, cell) in draw.cells.iter_mut().enumerate() {
            let merged = row
                .and_then(|r| r.cells.get(cell_index))
                .is_some_and(|c| c.merged_above);
            cell.top = if opens || draw.row == 0 {
                borders.outer
            } else {
                borders.inner && !merged
            };
        }
        draw.bottom = if last_row {
            borders.outer
        } else {
            closes && (borders.outer || borders.inner)
        };
    }
}

fn notes_on_page(slices: &[ParaSlice], paras: &[ParagraphSnapshot]) -> Vec<PageNote> {
    let mut notes = Vec::new();
    for slice in slices {
        if slice.line_start != 0 {
            continue;
        }
        let Some(paragraph) = paras.get(slice.para) else {
            continue;
        };
        for note in paragraph
            .notes
            .iter()
            .filter(|note| note.kind == libertum_model::NoteKind::Footnote)
        {
            notes.push(PageNote {
                number: note.number,
                text: note.text.clone(),
            });
        }
    }
    notes
}

fn note_block_height(eng: &mut Engine, notes: &[PageNote], width: f32) -> f32 {
    if notes.is_empty() {
        return 0.0;
    }
    let mut height = 4.0;
    for (index, note) in notes.iter().enumerate() {
        let text = format!("{}. {}", note.number, note.text);
        let mut snap = ParagraphSnapshot::new(8_000_000 + index as u64, text);
        snap.base = Some(SpanStyle::new(9.0));
        let metrics = eng.metrics(&snap, width);
        height += metrics.heights.iter().sum::<f32>() + 2.0;
    }
    height
}

#[allow(clippy::too_many_arguments)]
fn build_page(
    paras: &[ParagraphSnapshot],
    tables: &[TableSnapshot],
    upright: &[bool],
    row_at: &HashMap<usize, (usize, usize)>,
    eng: &mut Engine,
    geo: &PageGeometry,
    body_h: f32,
    columns: u8,
    gutter: f32,
    start: FlowPos,
) -> (PageLayout, FlowPos) {
    let page_h = body_h;
    let (ncols, width, gutter) = column_layout(geo.content_width(), columns, gutter);
    let page = |slices: Vec<ParaSlice>, used_height: f32, tables: Vec<TableDraw>| PageLayout {
        start,
        geometry: *geo,
        slices,
        endnotes: false,
        notes: Vec::new(),
        used_height,
        section: 0,
        number: 0,
        number_shown: true,
        first_in_section: false,
        tables,
    };
    let mut slices: Vec<ParaSlice> = Vec::new();
    let mut page_tables: Vec<TableDraw> = Vec::new();
    let mut p = start.para;
    let mut l = start.line;
    let mut used = 0.0f32;
    for col in 0..ncols {
        let x = col as f32 * (width + gutter);
        let col_start = slices.len();
        let mut y = 0.0f32;
        // A page that continues a table repeats its header rows at the top (Word).
        let column_tables = page_tables.len();
        if col == 0
            && let Some(&(table_index, row_index)) = row_at.get(&p)
            && let Some(table) = tables.get(table_index)
            && !upright.get(table_index).copied().unwrap_or(false)
        {
            let headers = usize::from(table.header_rows).min(row_index);
            for header in 0..headers {
                let laid = layout_table_row(
                    paras, eng, table, header, x, width, y, col as u16, true, 0.0, None, false,
                );
                if y + laid.height > page_h + EPS {
                    break;
                }
                y += laid.height;
                slices.extend(laid.slices);
                page_tables.push(laid.draw);
            }
        }
        // Where body content starts in this column (after any repeated header rows).
        let body_start = y;
        let mut filled = false;
        while p < paras.len() {
            // A table row: cells side by side. It is placed whole when it fits; otherwise it
            // moves on, or breaks at a line boundary when it may (or when it is taller than
            // a whole column). `l` then carries how much of the row is already placed.
            if let Some(&(table_index, row_index)) = row_at.get(&p) {
                let Some(table) = tables.get(table_index) else {
                    p += 1;
                    l = 0;
                    continue;
                };
                if upright.get(table_index).copied().unwrap_or(false) {
                    // An upright table: laid out on the sheet's own frame as horizontal
                    // rows, as wide as the table is (at most the sheet's width) along the
                    // flow, and as many rows as the column holds across it.
                    let sheet_width = geo.content_height();
                    let total: f32 = table.widths_pt.iter().sum();
                    let flow = total.min(sheet_width).max(1.0);
                    if y > EPS && y + flow > page_h + EPS {
                        filled = true;
                        break;
                    }
                    let left = (sheet_width - (y + flow)).max(0.0);
                    let mut cross = 0.0f32;
                    let mut row = row_index;
                    let mut next = p;
                    while row < table.rows.len() {
                        let mut laid = layout_table_row(
                            paras,
                            eng,
                            table,
                            row,
                            left,
                            flow,
                            x + cross,
                            col as u16,
                            false,
                            0.0,
                            None,
                            false,
                        );
                        if row > row_index && cross + laid.height > width + EPS {
                            break;
                        }
                        cross += laid.height;
                        for s in &mut laid.slices {
                            s.upright = true;
                        }
                        laid.draw.upright = true;
                        slices.extend(laid.slices);
                        page_tables.push(laid.draw);
                        next = table.rows[row]
                            .cells
                            .iter()
                            .flat_map(|cell| cell.paragraphs.iter())
                            .copied()
                            .max()
                            .map_or(next + 1, |last| last + 1);
                        row += 1;
                    }
                    y += flow;
                    used = used.max(y);
                    p = next;
                    l = 0;
                    continue;
                }
                let after_row = table.rows[row_index]
                    .cells
                    .iter()
                    .flat_map(|cell| cell.paragraphs.iter())
                    .copied()
                    .max()
                    .map_or(p + 1, |last| last + 1);
                let consumed = decode_consumed(l);
                let whole = layout_table_row(
                    paras, eng, table, row_index, x, width, y, col as u16, false, consumed, None,
                    false,
                );
                let empty = y <= body_start + EPS;
                if y + whole.height <= page_h + EPS {
                    y += whole.height;
                    used = used.max(y);
                    slices.extend(whole.slices);
                    page_tables.push(whole.draw);
                    p = after_row;
                    l = 0;
                    continue;
                }
                let can_split =
                    table.rows[row_index].can_split && row_index >= usize::from(table.header_rows);
                if !empty && !can_split {
                    // The whole row moves to the next column (or page).
                    filled = true;
                    break;
                }
                let part = layout_table_row(
                    paras,
                    eng,
                    table,
                    row_index,
                    x,
                    width,
                    y,
                    col as u16,
                    false,
                    consumed,
                    Some(page_h - y),
                    empty,
                );
                if !part.placed_any {
                    filled = true;
                    break;
                }
                y += part.height;
                used = used.max(y);
                slices.extend(part.slices);
                page_tables.push(part.draw);
                match part.rest {
                    Some(rest) => {
                        l = encode_consumed(rest);
                        filled = true;
                        break;
                    }
                    None => {
                        p = after_row;
                        l = 0;
                        continue;
                    }
                }
            }
            let para = &paras[p];
            if l == 0 && para.forces_new_page() && !slices.is_empty() {
                // Page or section break: this paragraph opens the next page.
                return (
                    page(slices, used.max(y).min(page_h), page_tables),
                    FlowPos { para: p, line: 0 },
                );
            }
            let m = eng.metrics(para, width);
            let n = m.heights.len();
            // Space before is dropped at the top of a column that a paragraph merely
            // overflowed onto, but kept after a page break and at the top of the document
            // (as Word does), which is how a title is placed lower on its page.
            let y_before = y;
            if l == 0 && (y > 0.0 || para.forces_new_page() || p == 0) {
                y += para.space_before_pt;
            }
            let first = l;
            let y_start = y;
            while l < n {
                let h = m.heights[l];
                let can_break = slices.len() > col_start || l > first;
                if y + h > page_h + EPS && can_break {
                    break;
                }
                y += h;
                l += 1;
            }
            if l < n {
                // The paragraph does not fit: decide how much of it stays in this column.
                let mut keep = if para.keep_together && first == 0 {
                    0
                } else if para.widow_control {
                    widow_orphan_split(first, l, n)
                } else {
                    l
                };
                // Something must go in every column, so the rules give way on an empty one.
                if keep == first && slices.len() == col_start {
                    keep = l;
                }
                if keep < l {
                    l = keep;
                    y = y_start + m.heights[first..l].iter().sum::<f32>();
                }
            }
            if l > first {
                slices.push(ParaSlice {
                    para: p,
                    line_start: first,
                    line_end: l,
                    y: y_start,
                    col: col as u16,
                    x,
                    upright: false,
                });
            } else {
                y = y_before;
            }
            if l < n {
                // Column full mid-paragraph (or before its first line).
                let mut next = FlowPos { para: p, line: l };
                if l == 0 {
                    // Nothing of this paragraph is here: paragraphs kept with it follow it.
                    pull_kept_paragraphs(paras, &mut slices, &mut next, &mut y, col_start);
                }
                p = next.para;
                l = next.line;
                filled = true;
                break;
            }
            p += 1;
            l = 0;
            y += para.space_after_pt;
        }
        used = used.max(y);
        finish_table_lines(&mut page_tables[column_tables..], tables);
        if !filled {
            // The document ended inside this column: nothing follows on this page.
            return (
                page(slices, used.min(page_h), page_tables),
                FlowPos {
                    para: paras.len(),
                    line: 0,
                },
            );
        }
    }
    (
        page(slices, used.min(page_h), page_tables),
        FlowPos { para: p, line: l },
    )
}

/// Widow and orphan control for a paragraph of `n` lines whose lines `first..fits` fit on
/// the page: how many of its lines to place here (`first` moves the whole rest on). A
/// paragraph never leaves just its first line at the bottom of a page (an orphan) or carries
/// just its last line to the next (a widow).
fn widow_orphan_split(first: usize, fits: usize, n: usize) -> usize {
    let mut keep = fits;
    if n - keep == 1 && keep > first {
        keep -= 1;
    }
    if first == 0 && keep < 2 {
        keep = 0;
    }
    keep.max(first)
}

/// The next column starts at the first line of `next.para`: the whole paragraphs at the end
/// of this column that are kept with it (a chain of *keep with next*) go to the next column
/// too, unless that would empty the column. `min_cut` is where this column's slices start, so
/// the previous column's content is never pulled.
fn pull_kept_paragraphs(
    paras: &[ParagraphSnapshot],
    slices: &mut Vec<ParaSlice>,
    next: &mut FlowPos,
    y: &mut f32,
    min_cut: usize,
) {
    let mut cut = slices.len();
    let mut target = next.para;
    while cut > min_cut.max(1) {
        let s = slices[cut - 1];
        if s.para + 1 != target || s.line_start != 0 || !paras[s.para].keep_with_next {
            break;
        }
        cut -= 1;
        target = s.para;
    }
    if cut < slices.len() {
        let moved = slices.split_off(cut);
        *next = FlowPos {
            para: target,
            line: 0,
        };
        // Content now ends where the first moved paragraph began.
        *y = moved.first().map_or(*y, |m| m.y);
    }
}
