//! The document tree, its block list and its typed errors (PLAN §4.1).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use thiserror::Error;

use crate::format::{StyleId, StyleSheet};
use crate::header_footer::{HeadersFooters, HfKind};
use crate::media::Media;
use crate::ops::Operation;
use crate::page::PageSetup;
use crate::paragraph::{Block, BlockId, ParaProps, Paragraph};
use crate::table::{MIN_TABLE_COLUMN_PT, Table, TableBorders, TableCell, TableRow};

/// Errors from applying an operation. Nothing in this crate panics on bad input.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ModelError {
    #[error("block {0:?} does not exist")]
    BlockNotFound(BlockId),
    #[error("the image {hash:016x} is missing from the file")]
    MissingMedia { hash: u64 },
    #[error("block {0:?} is not a paragraph")]
    NotParagraph(BlockId),
    #[error("block {0:?} is not a table")]
    NotTable(BlockId),
    #[error("offset {offset} is not a character boundary in block {block:?}")]
    NotCharBoundary { block: BlockId, offset: usize },
    #[error("range {start}..{end} is invalid in block {block:?} (length {len})")]
    RangeOutOfBounds {
        block: BlockId,
        start: usize,
        end: usize,
        len: usize,
    },
    #[error("block index {index} is out of range (length {len})")]
    IndexOutOfBounds { index: usize, len: usize },
    #[error("block id {0:?} is already in use")]
    DuplicateBlockId(BlockId),
    #[error("section {index} does not exist ({len} sections)")]
    SectionNotFound { index: usize, len: usize },
    #[error("paragraph style {0:?} does not exist")]
    StyleNotFound(StyleId),
}

/// Ordered top-level blocks with O(1) id lookup.
///
/// The `Vec` is fine up to roughly 20k blocks (PLAN §4.1); the `HashMap` keeps a text edit's
/// block lookup O(1) so the keystroke path is never O(document). Structural edits rebuild the
/// index, which is O(n) but only on split/merge/insert/remove.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "Vec<Block>", into = "Vec<Block>")]
pub struct BlockList {
    blocks: Vec<Block>,
    #[serde(skip)]
    positions: HashMap<BlockId, usize>,
}

impl From<Vec<Block>> for BlockList {
    fn from(blocks: Vec<Block>) -> Self {
        Self::from_blocks(blocks)
    }
}

impl From<BlockList> for Vec<Block> {
    fn from(list: BlockList) -> Self {
        list.blocks
    }
}

impl BlockList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        let mut list = Self {
            blocks,
            positions: HashMap::new(),
        };
        list.rebuild_positions();
        list
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn get(&self, id: BlockId) -> Option<&Block> {
        self.positions.get(&id).map(|&i| &self.blocks[i])
    }

    pub fn get_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        let i = *self.positions.get(&id)?;
        self.blocks.get_mut(i)
    }

    pub fn at(&self, index: usize) -> Option<&Block> {
        self.blocks.get(index)
    }

    pub fn at_mut(&mut self, index: usize) -> Option<&mut Block> {
        self.blocks.get_mut(index)
    }

    pub fn index_of(&self, id: BlockId) -> Option<usize> {
        self.positions.get(&id).copied()
    }

    pub fn id_at(&self, index: usize) -> Option<BlockId> {
        self.blocks.get(index).map(Block::id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Block> {
        self.blocks.iter()
    }

    pub fn ids(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.blocks.iter().map(Block::id)
    }

    pub fn insert(&mut self, index: usize, block: Block) {
        let index = index.min(self.blocks.len());
        self.blocks.insert(index, block);
        self.rebuild_positions();
    }

    pub fn push(&mut self, block: Block) {
        self.insert(self.blocks.len(), block);
    }

    pub fn remove(&mut self, index: usize) -> Option<Block> {
        if index >= self.blocks.len() {
            return None;
        }
        let block = self.blocks.remove(index);
        self.rebuild_positions();
        Some(block)
    }

    fn rebuild_positions(&mut self) {
        self.positions.clear();
        for (i, block) in self.blocks.iter().enumerate() {
            self.positions.insert(block.id(), i);
        }
    }
}

/// Which edge of the page the number sits on.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum PageNumberVertical {
    #[default]
    Top,
    Bottom,
}

/// Where along that edge the number sits.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum PageNumberHorizontal {
    Left,
    #[default]
    Centre,
    Right,
}

/// Where a paragraph lives in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParaLocation {
    /// Index of the top-level block.
    pub block: usize,
    /// The cell path when the paragraph is inside a table.
    pub table: Option<TableCellLocation>,
    /// Position in the flat paragraph order (cell paragraphs included).
    pub index: usize,
}

/// A paragraph's place inside a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableCellLocation {
    pub table: BlockId,
    pub row: usize,
    pub cell: usize,
    pub paragraph: usize,
}

/// A page number's place on the page, and whether alternate pages mirror it (book style:
/// outside edge on the first page and every odd page after it).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(default)]
pub struct PageNumberPos {
    pub vertical: PageNumberVertical,
    pub horizontal: PageNumberHorizontal,
    /// Mirror left/right on every second page: the chosen side on pages 1, 3, 5..., the other
    /// side on pages 2, 4, 6...
    pub mirror: bool,
}

impl PageNumberPos {
    /// Header, right-aligned (manuscript convention).
    pub const TOP_RIGHT: Self = Self {
        vertical: PageNumberVertical::Top,
        horizontal: PageNumberHorizontal::Right,
        mirror: false,
    };
    /// Footer, centred (book convention).
    pub const BOTTOM_CENTRE: Self = Self {
        vertical: PageNumberVertical::Bottom,
        horizontal: PageNumberHorizontal::Centre,
        mirror: false,
    };
}

/// Page numbering for the whole document: whether numbers are drawn, where, and where the
/// count begins. It lives on the document rather than on a section, so the controls apply to
/// every page; `None` on the document means "use the first section's settings", which keeps
/// files imported before this existed showing their numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PageNumbering {
    /// Draw a number in each page's margin.
    pub show: bool,
    pub position: PageNumberPos,
    /// Do not draw a number on page 1.
    pub hide_first_page: bool,
    /// Physical page (1-based) where numbering begins. Pages before it carry no number.
    pub start_page: u32,
    /// Number written on the start page; later pages count up from it.
    pub start_at: u32,
}

impl Default for PageNumbering {
    fn default() -> Self {
        Self {
            show: false,
            position: PageNumberPos::BOTTOM_CENTRE,
            hide_first_page: false,
            start_page: 1,
            start_at: 1,
        }
    }
}

impl PageNumbering {
    /// The settings an imported or older document's first section carries.
    pub fn from_sections(sections: &[Section]) -> Self {
        let first = sections.first();
        Self {
            show: first.is_some_and(|section| section.page_numbers.is_some()),
            position: first
                .and_then(|section| section.page_numbers)
                .unwrap_or(PageNumberPos::BOTTOM_CENTRE),
            hide_first_page: first.is_some_and(|section| section.hide_first_page),
            start_page: 1,
            start_at: first
                .and_then(|section| section.first_page_number)
                .unwrap_or(1),
        }
    }

    /// Clamps the page and number to sensible values (a hand-edited or imported file may
    /// hold anything).
    pub fn clamped(mut self) -> Self {
        self.start_page = self.start_page.max(1);
        self.start_at = self.start_at.min(1_000_000);
        self
    }

    /// The number drawn on physical page `page` (1-based), and whether it is drawn at all.
    pub fn number_on(&self, page: u32) -> (u32, bool) {
        let page = page.max(1);
        if page < self.start_page {
            (page, false)
        } else {
            (self.start_at.saturating_add(page - self.start_page), true)
        }
    }
}

/// A section of the document. Section 0 starts at the first paragraph; section `n` starts at
/// the `n`-th paragraph with `ParaProps::section_start`.
///
/// Page setup (paper, orientation, margins) is document-wide for now: the editor applies a
/// change to every section, because pagination still uses one geometry for the whole document.
/// Headers and footers are per section, and their text is laid out per page (PLAN §4.1, §4.2);
/// flowing each section at its own page size still waits for columns.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Section {
    /// Section name, for import/export round-tripping later.
    pub name: Option<String>,
    /// Paper size, orientation and margins.
    pub page: PageSetup,
    /// Page numbers shown directly in this section's margin, if any. A header or footer can
    /// also carry the number with the `{page}` field (DOCX `PAGE`, ODT `text:page-number`).
    pub page_numbers: Option<PageNumberPos>,
    /// Restart numbering at this value (DOCX `w:pgNumType w:start`, ODT
    /// `style:page-number`); `None` continues from the previous section. Zero is allowed.
    pub first_page_number: Option<u32>,
    /// Do not show the number on the section's first page (a title page, typically).
    pub hide_first_page: bool,
    /// Headers for every page, the first page and even pages.
    pub headers: HeadersFooters,
    /// Footers for every page, the first page and even pages.
    pub footers: HeadersFooters,
    /// Distance from the top of the page to the top of the header, points (DOCX
    /// `w:pgMar w:header`, ODT `style:header-distance`). Word's default is 0.5 in.
    pub header_distance_pt: f32,
    /// Distance from the bottom of the page to the bottom of the footer, points (DOCX
    /// `w:pgMar w:footer`). Word's default is 0.5 in.
    pub footer_distance_pt: f32,
    /// Number of text columns on this section's pages (DOCX `w:cols w:num`, ODT
    /// `style:columns fo:column-count`). One is a normal page.
    pub columns: u8,
    /// Space between columns, points (DOCX `w:cols w:space`, ODT `style:columns
    /// fo:column-gap`). Word's default is 0.5 in.
    pub column_gutter_pt: f32,
    /// Use the previous section's headers (Word "Link to Previous"). The section's own
    /// headers are ignored while this is on, so switching it back restores them.
    pub link_headers_to_previous: bool,
    /// Use the previous section's footers (Word "Link to Previous").
    pub link_footers_to_previous: bool,
    /// Text drawn large and faint behind the body of every page of this section ("DRAFT",
    /// "CONFIDENTIAL"). The app sets it on every section at once.
    pub watermark: Option<Watermark>,
    /// Background colour of this section's pages, straight RGBA (DOCX `w:background`, ODT
    /// `fo:background-color` on the page layout, HTML `body` background). `None` is plain
    /// white paper. The app sets it on every section at once, like the watermark.
    pub page_colour: Option<[u8; 4]>,
    /// The language the text is written in, as a BCP 47 tag such as `en-GB` (DOCX
    /// `w:lang`, ODT `fo:language` and `fo:country`, HTML `lang`). `None` says nothing about
    /// it. Always a tag [`language_tag`] accepted. The app sets it on every section at once,
    /// like the watermark.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// A second language for a document written in two (a tag like `language`). Spelling
    /// accepts a word either language knows; hyphenation and the regional font for Chinese,
    /// Japanese and Korean text take what either needs. `None` for one language. The app
    /// sets it on every section at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language2: Option<String>,
    /// Vertical text, set from right to left (DOCX `w:textDirection tbRl`, ODT
    /// `style:writing-mode tb-rl`): lines run down the page and stack from the right, as in
    /// Chinese, Japanese and Korean books. Latin text turns sideways and ideographs stay
    /// upright. The app sets it on every section at once, like the watermark.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub vertical: bool,
}

/// Normalises a language tag read from a file or typed in: a 2 or 3 letter language, then
/// optional subtags of 2 to 8 letters or digits, joined by `-` or `_`. The language is lower
/// case, a 2 letter region upper case, a 4 letter script title case, the rest lower case.
/// Anything else (empty, too long, odd characters) gives `None`, so a tag that reaches a file
/// writer is always safe to print.
pub fn language_tag(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 35 {
        return None;
    }
    let mut parts = raw.split(['-', '_']);
    let language = parts.next()?;
    if !(2..=3).contains(&language.len()) || !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = language.to_ascii_lowercase();
    for part in parts {
        if !(2..=8).contains(&part.len()) || !part.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        out.push('-');
        let alphabetic = part.chars().all(|c| c.is_ascii_alphabetic());
        match part.len() {
            2 if alphabetic => out.push_str(&part.to_ascii_uppercase()),
            4 if alphabetic => {
                out.push_str(&part[..1].to_ascii_uppercase());
                out.push_str(&part[1..].to_ascii_lowercase());
            }
            _ => out.push_str(&part.to_ascii_lowercase()),
        }
    }
    Some(out)
}

/// A text watermark: drawn behind the page's content, centred, in light grey.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub text: String,
    /// Along the page's diagonal, rising to the right; otherwise horizontal.
    pub diagonal: bool,
}

impl Default for Watermark {
    fn default() -> Self {
        Self {
            text: String::new(),
            diagonal: true,
        }
    }
}

impl Watermark {
    /// Longest watermark text kept, characters.
    pub const MAX_CHARS: usize = 60;

    /// A watermark for `text`, or `None` when the text is blank. Line breaks become spaces
    /// and the text is cut to [`MAX_CHARS`](Self::MAX_CHARS).
    pub fn new(text: &str, diagonal: bool) -> Option<Self> {
        let text: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(Self::MAX_CHARS)
            .collect();
        (!text.is_empty()).then_some(Self { text, diagonal })
    }
}

/// Most columns a section may use. Word allows up to 45; the inspector caps it at 12, and the
/// layout drops a column narrower than 0.5 in rather than drawing it.
pub const MAX_COLUMNS: u8 = 12;

/// Word's default space between columns: 0.5 in.
pub const DEFAULT_COLUMN_GUTTER_PT: f32 = 36.0;

/// Word's default distance from the page edge to a header or footer: 0.5 in.
pub const DEFAULT_HEADER_FOOTER_DISTANCE_PT: f32 = 36.0;

impl Default for Section {
    fn default() -> Self {
        Self {
            name: None,
            page: PageSetup::default(),
            page_numbers: None,
            first_page_number: None,
            hide_first_page: false,
            headers: HeadersFooters::default(),
            footers: HeadersFooters::default(),
            header_distance_pt: DEFAULT_HEADER_FOOTER_DISTANCE_PT,
            footer_distance_pt: DEFAULT_HEADER_FOOTER_DISTANCE_PT,
            columns: 1,
            column_gutter_pt: DEFAULT_COLUMN_GUTTER_PT,
            link_headers_to_previous: false,
            link_footers_to_previous: false,
            watermark: None,
            page_colour: None,
            language: None,
            language2: None,
            vertical: false,
        }
    }
}

impl Section {
    /// True when this section uses the previous one's headers (or footers).
    pub fn header_footer_linked(&self, kind: HfKind) -> bool {
        match kind {
            HfKind::Header => self.link_headers_to_previous,
            HfKind::Footer => self.link_footers_to_previous,
        }
    }

    /// This section's headers or footers.
    pub fn header_footer(&self, kind: HfKind) -> &HeadersFooters {
        match kind {
            HfKind::Header => &self.headers,
            HfKind::Footer => &self.footers,
        }
    }

    pub fn header_footer_mut(&mut self, kind: HfKind) -> &mut HeadersFooters {
        match kind {
            HfKind::Header => &mut self.headers,
            HfKind::Footer => &mut self.footers,
        }
    }

    /// Distance from the page edge to the header (top) or footer (bottom), points.
    pub fn header_footer_distance(&self, kind: HfKind) -> f32 {
        match kind {
            HfKind::Header => self.header_distance_pt,
            HfKind::Footer => self.footer_distance_pt,
        }
    }

    pub fn set_header_footer_distance(&mut self, kind: HfKind, pt: f32) {
        let pt = pt.max(0.0);
        match kind {
            HfKind::Header => self.header_distance_pt = pt,
            HfKind::Footer => self.footer_distance_pt = pt,
        }
    }

    /// Sets the number of text columns, clamped to 1..=12.
    pub fn set_columns(&mut self, columns: u8) {
        self.columns = columns.clamp(1, MAX_COLUMNS);
    }

    /// Sets the space between columns in points (never negative or NaN).
    pub fn set_column_gutter(&mut self, pt: f32) {
        self.column_gutter_pt = if pt.is_finite() {
            pt.max(0.0)
        } else {
            DEFAULT_COLUMN_GUTTER_PT
        };
    }
}

/// What a change touched, for layout invalidation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Blocks whose text, runs or props changed.
    pub dirty: Vec<BlockId>,
    /// A block was inserted, removed, split or merged: paragraph indices moved.
    pub structural: bool,
    /// Section settings (page numbering) changed.
    pub sections: bool,
    /// Paragraph styles whose definition changed: every paragraph using them, or a style
    /// based on them, must be re-resolved.
    pub styles: Vec<StyleId>,
    /// The document defaults changed: every paragraph must be re-resolved.
    pub all_styles: bool,
}

impl ChangeSet {
    pub fn touch(&mut self, id: BlockId) {
        if !self.dirty.contains(&id) {
            self.dirty.push(id);
        }
    }

    pub fn merge(&mut self, other: ChangeSet) {
        for id in other.dirty {
            self.touch(id);
        }
        self.structural |= other.structural;
        self.sections |= other.sections;
        self.all_styles |= other.all_styles;
        for id in other.styles {
            if !self.styles.contains(&id) {
                self.styles.push(id);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.dirty.is_empty()
            && !self.structural
            && !self.sections
            && self.styles.is_empty()
            && !self.all_styles
    }
}

/// How many operations a document has had applied. It tells caches built from the content
/// (the Find match list) that they are stale. It is bookkeeping, not content, so it never
/// makes two documents unequal and is never written to a file.
#[derive(Clone, Copy, Debug, Default)]
struct Revision(u64);

impl PartialEq for Revision {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// The document's last save, unix seconds (UTC). Bookkeeping, not content: it decides what
/// an "updated date and time" field shows, and two documents with different save times are
/// still the same document.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SavedAt(pub Option<i64>);

impl PartialEq for SavedAt {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// The document: body blocks plus the bookkeeping needed by the editor.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Document {
    #[serde(skip)]
    revision: Revision,
    body: BlockList,
    next_id: u64,
    #[serde(skip)]
    word_count: usize,
    /// Paragraph ids in document order, table cell paragraphs included.
    #[serde(skip)]
    flat: Vec<BlockId>,
    /// Where each paragraph lives: O(1) on the keystroke path.
    #[serde(skip)]
    locations: HashMap<BlockId, ParaLocation>,
    pub sections: Vec<Section>,
    pub styles: StyleSheet,
    /// Page numbering for the whole document. `None` uses the first section's settings (an
    /// imported or pre-0.1.1 file); the app always sets `Some`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    page_numbering: Option<PageNumbering>,
    /// When the document was last saved, if it ever was.
    saved_at: SavedAt,
}

/// Reading a `Document` rebuilds its indexes: the caches are derived state and are never
/// written to a file.
impl<'de> serde::Deserialize<'de> for Document {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Parts {
            body: BlockList,
            next_id: u64,
            #[serde(default)]
            sections: Vec<Section>,
            #[serde(default)]
            styles: StyleSheet,
            #[serde(default)]
            page_numbering: Option<PageNumbering>,
            #[serde(default)]
            saved_at: SavedAt,
        }
        let parts = Parts::deserialize(deserializer)?;
        let mut doc = Document::from_parts(parts.body, parts.next_id, parts.sections, parts.styles);
        doc.page_numbering = parts.page_numbering.map(PageNumbering::clamped);
        doc.saved_at = parts.saved_at;
        Ok(doc)
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// A new document with one empty paragraph.
    pub fn new() -> Self {
        let mut doc = Self {
            body: BlockList::new(),
            next_id: 1,
            word_count: 0,
            revision: Revision::default(),
            flat: Vec::new(),
            locations: HashMap::new(),
            sections: vec![Section::default()],
            styles: StyleSheet::default(),
            page_numbering: None,
            saved_at: SavedAt::default(),
        };
        let id = doc.alloc_id();
        doc.body.push(Block::Paragraph(Paragraph::new(id, "")));
        doc.reindex();
        doc
    }

    /// Page numbering for the whole document (from the first section when the document has
    /// no explicit setting, so imported files keep theirs).
    pub fn page_numbering(&self) -> PageNumbering {
        self.page_numbering
            .unwrap_or_else(|| PageNumbering::from_sections(&self.sections))
            .clamped()
    }

    /// True when the document carries an explicit whole-document numbering setting.
    pub fn has_page_numbering(&self) -> bool {
        self.page_numbering.is_some()
    }

    /// Sets the whole-document numbering (`None` returns to the first section's settings)
    /// and returns the old one. Called through `Operation::SetPageNumbering`.
    pub fn set_page_numbering(
        &mut self,
        numbering: Option<PageNumbering>,
    ) -> Option<PageNumbering> {
        let old = self.page_numbering;
        self.page_numbering = numbering.map(PageNumbering::clamped);
        self.revision.0 = self.revision.0.wrapping_add(1);
        old
    }

    /// When the document was last saved, unix seconds (UTC).
    pub fn saved_at(&self) -> Option<i64> {
        self.saved_at.0
    }

    /// Records the last save time (an "updated date and time" field shows it). Bookkeeping:
    /// it never counts as a content change and never bumps the revision.
    pub fn set_saved_at(&mut self, seconds: i64) {
        self.saved_at = SavedAt(Some(seconds));
    }

    /// The next id this document would allocate: stitching shifts a following document's ids
    /// from here so nothing collides.
    /// A counter that changes with every operation applied (undo and redo included). Equal
    /// values on the same document mean nothing has changed in between.
    pub fn revision(&self) -> u64 {
        self.revision.0
    }

    pub fn next_free_id(&self) -> u64 {
        self.next_id
    }

    /// The largest list group in use (0 when there are no lists).
    pub fn max_list_group(&self) -> u64 {
        self.paragraphs()
            .filter_map(|paragraph| paragraph.props.list.as_ref().map(|list| list.group))
            .max()
            .unwrap_or(0)
    }

    /// Shifts every paragraph and table id, the TOC targets that point at them, and every list
    /// group. Table cell paragraphs shift with everything else. Used by document stitching.
    pub fn restamp(&mut self, shift: u64, list_shift: u64) {
        for block in &mut self.body.blocks {
            match block {
                Block::Paragraph(paragraph) => paragraph.restamp(shift, list_shift),
                Block::Table(table) => {
                    table.id = BlockId(table.id.0 + shift);
                    for row in &mut table.rows {
                        for cell in &mut row.cells {
                            for paragraph in &mut cell.paragraphs {
                                paragraph.restamp(shift, list_shift);
                            }
                        }
                    }
                }
            }
        }
        self.next_id = self.next_id.saturating_add(shift);
        self.revision.0 = self.revision.0.wrapping_add(1);
        self.reindex();
    }

    /// Appends blocks at the end of the body and rebuilds the index. Used by stitching, where
    /// ids have already been made unique with [`restamp`](Self::restamp).
    pub fn append_blocks(&mut self, blocks: Vec<Block>) {
        if blocks.is_empty() {
            return;
        }
        for block in &blocks {
            if let Block::Paragraph(paragraph) = block {
                self.next_id = self.next_id.max(paragraph.id.0 + 1);
            }
            if let Block::Table(table) = block {
                self.next_id = self.next_id.max(table.id.0 + 1);
            }
        }
        self.word_count += blocks
            .iter()
            .map(|block| match block {
                Block::Paragraph(paragraph) => paragraph.word_count(),
                Block::Table(table) => table.word_count(),
            })
            .sum::<usize>();
        self.body.blocks.extend(blocks);
        self.revision.0 = self.revision.0.wrapping_add(1);
        self.reindex();
    }

    /// Rebuilds a document from its stored parts without touching the ids, so bookmarks,
    /// references and TOC targets keep pointing at the same paragraphs (the native format's
    /// reader). The indexes and the word count are recomputed here.
    pub fn from_parts(
        body: BlockList,
        next_id: u64,
        mut sections: Vec<Section>,
        styles: StyleSheet,
    ) -> Self {
        if sections.is_empty() {
            sections.push(Section::default());
        }
        let mut doc = Self {
            body,
            next_id: next_id.max(1),
            word_count: 0,
            revision: Revision::default(),
            flat: Vec::new(),
            locations: HashMap::new(),
            sections,
            styles,
            page_numbering: None,
            saved_at: SavedAt::default(),
        };
        doc.reindex();
        doc.word_count = doc.paragraphs().map(Paragraph::word_count).sum();
        doc
    }

    /// Replaces images whose bytes are missing (a native-format read leaves placeholders)
    /// with the images `resolve` returns, and returns how many were filled. A placeholder
    /// the caller cannot resolve is an error rather than an image that would draw nothing.
    pub fn resolve_media(
        &mut self,
        mut resolve: impl FnMut(&Media) -> Option<Arc<Media>>,
    ) -> Result<usize, ModelError> {
        let ids: Vec<BlockId> = self.paragraphs().map(|paragraph| paragraph.id).collect();
        let mut filled = 0usize;
        for id in ids {
            let Ok(paragraph) = self.paragraph_mut(id) else {
                continue;
            };
            if !paragraph
                .images()
                .iter()
                .any(|image| image.media.is_placeholder())
            {
                continue;
            }
            let mut images = paragraph.images().to_vec();
            for image in &mut images {
                if image.media.is_placeholder() {
                    let Some(media) = resolve(&image.media) else {
                        return Err(ModelError::MissingMedia {
                            hash: image.media.hash(),
                        });
                    };
                    image.media = media;
                    filled += 1;
                }
            }
            paragraph.restore_images(images);
        }
        Ok(filled)
    }

    /// Builds a document from existing paragraphs, reallocating ids from 1. Used by the
    /// generator and by tests/importers.
    pub fn from_paragraphs(paragraphs: Vec<Paragraph>) -> Self {
        Self::from_blocks(paragraphs.into_iter().map(Block::Paragraph).collect())
    }

    /// Builds a document from existing blocks (paragraphs and tables), reallocating every id
    /// that is zero or already used. Tables keep their structure and cell text.
    pub fn from_blocks(blocks: Vec<Block>) -> Self {
        let mut next_id = 1u64;
        let mut taken: HashSet<u64> = HashSet::new();
        let mut list = Vec::with_capacity(blocks.len().max(1));
        for mut block in blocks {
            reassign_ids(&mut block, &mut next_id, &mut taken);
            list.push(block);
        }
        if list.is_empty() {
            list.push(Block::Paragraph(Paragraph::new(BlockId(1), "")));
            next_id = 2;
        }
        let mut doc = Self {
            body: BlockList::from_blocks(list),
            next_id,
            word_count: 0,
            revision: Revision::default(),
            flat: Vec::new(),
            locations: HashMap::new(),
            sections: vec![Section::default()],
            styles: StyleSheet::default(),
            page_numbering: None,
            saved_at: SavedAt::default(),
        };
        doc.recount_words();
        doc.reindex();
        doc
    }

    /// Rebuilds the flat paragraph order and the id-to-location map. O(paragraphs); called
    /// on structural edits only, never on typing.
    fn reindex(&mut self) {
        self.flat.clear();
        self.locations.clear();
        for (block_index, block) in self.body.iter().enumerate() {
            match block {
                Block::Paragraph(p) => {
                    let index = self.flat.len();
                    self.flat.push(p.id);
                    self.locations.insert(
                        p.id,
                        ParaLocation {
                            block: block_index,
                            table: None,
                            index,
                        },
                    );
                }
                Block::Table(t) => {
                    for (row, r) in t.rows.iter().enumerate() {
                        for (cell, c) in r.cells.iter().enumerate() {
                            for (paragraph, p) in c.paragraphs.iter().enumerate() {
                                let index = self.flat.len();
                                self.flat.push(p.id);
                                self.locations.insert(
                                    p.id,
                                    ParaLocation {
                                        block: block_index,
                                        table: Some(TableCellLocation {
                                            table: t.id,
                                            row,
                                            cell,
                                            paragraph,
                                        }),
                                        index,
                                    },
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A fresh table with `rows` rows and `columns` columns, each cell holding one empty
    /// paragraph. Ids are allocated; insert it with [`Operation::InsertTable`].
    pub fn new_table(&mut self, rows: usize, columns: usize, width_pt: f32) -> Table {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let id = self.alloc_id();
        let width = (width_pt / columns as f32).max(MIN_TABLE_COLUMN_PT);
        let mut table = Table {
            style: None,
            id,
            widths_pt: vec![width; columns],
            rows: Vec::with_capacity(rows),
            header_rows: 0,
            borders: TableBorders::default(),
        };
        for _ in 0..rows {
            let mut cells = Vec::with_capacity(columns);
            for _ in 0..columns {
                let paragraph = self.alloc_id();
                cells.push(TableCell::new(Paragraph::new(paragraph, "")));
            }
            table.rows.push(TableRow {
                cells,
                min_height_pt: 0.0,
                keep_together: false,
            });
        }
        table
    }

    /// Allocates a stable block id.
    pub fn alloc_id(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn body(&self) -> &BlockList {
        &self.body
    }

    pub fn len(&self) -> usize {
        self.body.len()
    }

    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    pub fn word_count(&self) -> usize {
        self.word_count
    }

    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.body.get(id)
    }

    pub fn paragraph(&self, id: BlockId) -> Option<&Paragraph> {
        let location = *self.locations.get(&id)?;
        let block = self.body.at(location.block)?;
        match location.table {
            None => block.as_paragraph(),
            Some(cell) => block
                .as_table()?
                .cell(cell.row, cell.cell)?
                .paragraphs
                .get(cell.paragraph),
        }
    }

    pub fn paragraph_at(&self, index: usize) -> Option<&Paragraph> {
        self.paragraph(*self.flat.get(index)?)
    }

    /// Flat paragraph index (table cell paragraphs included), not the top-level block index.
    pub fn index_of(&self, id: BlockId) -> Option<usize> {
        self.locations.get(&id).map(|location| location.index)
    }

    /// Where `id` lives, if it is a paragraph.
    pub fn location_of(&self, id: BlockId) -> Option<ParaLocation> {
        self.locations.get(&id).copied()
    }

    /// Number of paragraphs in document order, table cells included.
    pub fn paragraph_count(&self) -> usize {
        self.flat.len()
    }

    /// Paragraph ids in document order, table cells included.
    pub fn paragraph_ids(&self) -> impl Iterator<Item = BlockId> + '_ {
        self.flat.iter().copied()
    }

    /// Paragraphs in document order, table cells included.
    pub fn paragraphs(&self) -> impl Iterator<Item = &Paragraph> + '_ {
        self.flat.iter().filter_map(|id| self.paragraph(*id))
    }

    pub fn table(&self, id: BlockId) -> Option<&Table> {
        self.body.get(id)?.as_table()
    }

    pub fn table_mut(&mut self, id: BlockId) -> Option<&mut Table> {
        self.body.get_mut(id)?.as_table_mut()
    }

    pub fn first_id(&self) -> Option<BlockId> {
        self.flat.first().copied()
    }

    pub fn last_id(&self) -> Option<BlockId> {
        self.flat.last().copied()
    }

    /// Applies one operation and returns its inverse. The document is unchanged when this
    /// returns an error.
    pub fn apply(&mut self, op: Operation) -> Result<Operation, ModelError> {
        self.revision.0 = self.revision.0.wrapping_add(1);
        match op {
            Operation::InsertText {
                block,
                offset,
                text,
                format,
            } => {
                let p = self.paragraph_mut(block)?;
                if !p.text().is_char_boundary(offset) || offset > p.len() {
                    return Err(ModelError::NotCharBoundary { block, offset });
                }
                let before = p.word_count();
                match &format {
                    Some(runs) => p.insert_text_formatted(offset, &text, runs),
                    None => p.insert_text(offset, &text),
                }
                let after = p.word_count();
                self.word_count = self.word_count - before + after;
                Ok(Operation::DeleteRange {
                    block,
                    range: offset..offset + text.len(),
                })
            }
            Operation::DeleteRange { block, range } => {
                let p = self.paragraph_mut(block)?;
                if range.start > range.end || range.end > p.len() {
                    return Err(ModelError::RangeOutOfBounds {
                        block,
                        start: range.start,
                        end: range.end,
                        len: p.len(),
                    });
                }
                let before = p.word_count();
                let (removed, runs) = p.delete_range_with_runs(range.clone());
                let after = p.word_count();
                self.word_count = self.word_count - before + after;
                Ok(Operation::InsertText {
                    block,
                    offset: range.start,
                    text: removed,
                    format: Some(runs),
                })
            }
            Operation::SplitBlock {
                block,
                offset,
                new_id,
                props,
            } => {
                if self.body.get(new_id).is_some() || self.locations.contains_key(&new_id) {
                    return Err(ModelError::DuplicateBlockId(new_id));
                }
                let location = self
                    .location_of(block)
                    .ok_or(ModelError::BlockNotFound(block))?;
                {
                    let p = self
                        .paragraph(block)
                        .ok_or(ModelError::NotParagraph(block))?;
                    if !p.text().is_char_boundary(offset) || offset > p.len() {
                        return Err(ModelError::NotCharBoundary { block, offset });
                    }
                }
                let before = self
                    .paragraph(block)
                    .map(Paragraph::word_count)
                    .unwrap_or(0);
                let tail = self.paragraph_mut(block)?.split_off(offset, new_id, props);
                let after = self
                    .paragraph(block)
                    .map(Paragraph::word_count)
                    .unwrap_or(0)
                    + tail.word_count();
                self.word_count = self.word_count - before + after;
                match location.table {
                    Some(cell) => {
                        let table = self
                            .table_mut(cell.table)
                            .ok_or(ModelError::BlockNotFound(cell.table))?;
                        let table_cell = table
                            .cell_mut(cell.row, cell.cell)
                            .ok_or(ModelError::BlockNotFound(block))?;
                        let at = (cell.paragraph + 1).min(table_cell.paragraphs.len());
                        table_cell.paragraphs.insert(at, tail);
                    }
                    None => {
                        let index = self
                            .body
                            .index_of(block)
                            .ok_or(ModelError::BlockNotFound(block))?;
                        self.body.insert(index + 1, Block::Paragraph(tail));
                    }
                }
                self.reindex();
                Ok(Operation::MergeBlocks { first: block })
            }
            Operation::MergeBlocks { first } => {
                let location = self
                    .location_of(first)
                    .ok_or(ModelError::BlockNotFound(first))?;
                if let Some(cell) = location.table {
                    let next = {
                        let table = self
                            .table(cell.table)
                            .ok_or(ModelError::BlockNotFound(cell.table))?;
                        table
                            .cell(cell.row, cell.cell)
                            .and_then(|c| c.paragraphs.get(cell.paragraph + 1))
                            .cloned()
                    };
                    let Some(next) = next else {
                        // A cell's last paragraph cannot merge into the next cell.
                        return Err(ModelError::NotParagraph(first));
                    };
                    let boundary = self.paragraph(first).map(Paragraph::len).unwrap_or(0);
                    let next_id = next.id;
                    let next_props = next.props;
                    let before = self
                        .paragraph(first)
                        .map(Paragraph::word_count)
                        .unwrap_or(0)
                        + next.word_count();
                    {
                        let table = self
                            .table_mut(cell.table)
                            .ok_or(ModelError::BlockNotFound(cell.table))?;
                        let table_cell = table
                            .cell_mut(cell.row, cell.cell)
                            .ok_or(ModelError::BlockNotFound(first))?;
                        if cell.paragraph + 1 < table_cell.paragraphs.len() {
                            table_cell.paragraphs.remove(cell.paragraph + 1);
                        }
                    }
                    let p = self.paragraph_mut(first)?;
                    p.append(next);
                    let after = p.word_count();
                    self.word_count = self.word_count - before + after;
                    self.reindex();
                    return Ok(Operation::SplitBlock {
                        block: first,
                        offset: boundary,
                        new_id: next_id,
                        props: Some(next_props),
                    });
                }
                let index = self
                    .body
                    .index_of(first)
                    .ok_or(ModelError::BlockNotFound(first))?;
                let next_id = self
                    .body
                    .id_at(index + 1)
                    .ok_or(ModelError::BlockNotFound(first))?;
                let (boundary, before, next_props) = {
                    let next = self
                        .paragraph(next_id)
                        .ok_or(ModelError::NotParagraph(next_id))?;
                    let first_para = self
                        .paragraph(first)
                        .ok_or(ModelError::NotParagraph(first))?;
                    // The split that undoes the merge lands at the end of the original
                    // first paragraph, not at the length of the second one.
                    (
                        first_para.len(),
                        first_para.word_count() + next.word_count(),
                        next.props,
                    )
                };
                let next = self
                    .body
                    .remove(index + 1)
                    .ok_or(ModelError::BlockNotFound(first))?;
                let Block::Paragraph(next) = next else {
                    return Err(ModelError::NotParagraph(next_id));
                };
                let p = self.paragraph_mut(first)?;
                p.append(next);
                let after = p.word_count();
                self.word_count = self.word_count - before + after;
                self.reindex();
                Ok(Operation::SplitBlock {
                    block: first,
                    offset: boundary,
                    new_id: next_id,
                    props: Some(next_props),
                })
            }
            Operation::InsertBlock { index, block } => {
                if index > self.body.len() {
                    return Err(ModelError::IndexOutOfBounds {
                        index,
                        len: self.body.len(),
                    });
                }
                self.word_count += block.word_count();
                let id = block.id();
                self.body.insert(index, block);
                self.reindex();
                Ok(Operation::RemoveBlock { block: id })
            }
            Operation::RemoveBlock { block } => {
                let index = self
                    .body
                    .index_of(block)
                    .ok_or(ModelError::BlockNotFound(block))?;
                let removed = self
                    .body
                    .remove(index)
                    .ok_or(ModelError::BlockNotFound(block))?;
                self.word_count = self.word_count.saturating_sub(removed.word_count());
                self.reindex();
                Ok(Operation::InsertBlock {
                    index,
                    block: removed,
                })
            }
            Operation::InsertTable { index, mut table } => {
                if index > self.body.len() {
                    return Err(ModelError::IndexOutOfBounds {
                        index,
                        len: self.body.len(),
                    });
                }
                if self.body.get(table.id).is_some() {
                    return Err(ModelError::DuplicateBlockId(table.id));
                }
                table.normalise();
                let id = table.id;
                self.word_count += table.word_count();
                self.body.insert(index, Block::Table(table));
                self.reindex();
                Ok(Operation::RemoveTable { table: id })
            }
            Operation::RemoveTable { table } => {
                let index = self
                    .body
                    .index_of(table)
                    .ok_or(ModelError::BlockNotFound(table))?;
                let removed = self
                    .body
                    .remove(index)
                    .ok_or(ModelError::BlockNotFound(table))?;
                let Block::Table(removed) = removed else {
                    return Err(ModelError::NotTable(table));
                };
                self.word_count = self.word_count.saturating_sub(removed.word_count());
                self.reindex();
                Ok(Operation::InsertTable {
                    index,
                    table: removed,
                })
            }
            Operation::ReplaceTable { table, mut new } => {
                let index = self
                    .body
                    .index_of(table)
                    .ok_or(ModelError::BlockNotFound(table))?;
                if new.id != table {
                    return Err(ModelError::NotTable(table));
                }
                new.normalise();
                let old = {
                    let slot = self
                        .body
                        .at_mut(index)
                        .ok_or(ModelError::BlockNotFound(table))?;
                    let Block::Table(slot) = slot else {
                        return Err(ModelError::NotTable(table));
                    };
                    std::mem::replace(slot, new)
                };
                self.word_count = self.word_count - old.word_count()
                    + self.table(table).map(Table::word_count).unwrap_or(0);
                self.reindex();
                Ok(Operation::ReplaceTable { table, new: old })
            }
            Operation::SetRuns { block, runs } => {
                let old = self.paragraph_mut(block)?.set_runs(runs);
                Ok(Operation::SetRuns { block, runs: old })
            }
            Operation::SetParaProps { block, props } => {
                let old = self.paragraph_mut(block)?.set_props(props);
                Ok(Operation::SetParaProps { block, props: old })
            }
            Operation::SetFootnote { block, note } => {
                let old = self.paragraph_mut(block)?.set_note(note);
                Ok(Operation::SetFootnote { block, note: old })
            }
            Operation::SetNotes { block, notes } => {
                let old = self.paragraph_mut(block)?.set_notes(notes);
                Ok(Operation::SetNotes { block, notes: old })
            }
            Operation::SetBookmarks { block, bookmarks } => {
                let old = self.paragraph_mut(block)?.set_bookmarks(bookmarks);
                Ok(Operation::SetBookmarks {
                    block,
                    bookmarks: old,
                })
            }
            Operation::SetImages { block, images } => {
                let old = self.paragraph_mut(block)?.set_images(images);
                Ok(Operation::SetImages { block, images: old })
            }
            Operation::SetPageNumbering { numbering } => {
                let old = self.set_page_numbering(numbering);
                Ok(Operation::SetPageNumbering { numbering: old })
            }
            Operation::SetCrossRefs { block, cross_refs } => {
                let old = self.paragraph_mut(block)?.set_cross_refs(cross_refs);
                Ok(Operation::SetCrossRefs {
                    block,
                    cross_refs: old,
                })
            }
            Operation::SetDateTimeFields { block, date_fields } => {
                let old = self.paragraph_mut(block)?.set_date_fields(date_fields);
                Ok(Operation::SetDateTimeFields {
                    block,
                    date_fields: old,
                })
            }
            Operation::SetRuby { block, ruby } => {
                let old = self.paragraph_mut(block)?.set_ruby(ruby);
                Ok(Operation::SetRuby { block, ruby: old })
            }
            Operation::SetSection { index, section } => {
                let len = self.sections.len();
                let slot = self
                    .sections
                    .get_mut(index)
                    .ok_or(ModelError::SectionNotFound { index, len })?;
                let old = std::mem::replace(slot, section);
                Ok(Operation::SetSection {
                    index,
                    section: old,
                })
            }
            Operation::SetParaDefaults { para } => {
                let old = std::mem::replace(&mut self.styles.para_defaults, para);
                Ok(Operation::SetParaDefaults { para: old })
            }
            Operation::SetCharDefaults { char } => {
                let old = std::mem::replace(&mut self.styles.defaults, char);
                Ok(Operation::SetCharDefaults { char: old })
            }
            Operation::SetStyle { id, style } => {
                let slot = self
                    .styles
                    .paragraph
                    .get_mut(id.0 as usize)
                    .ok_or(ModelError::StyleNotFound(id))?;
                let old = std::mem::replace(slot, *style);
                Ok(Operation::SetStyle {
                    id,
                    style: Box::new(old),
                })
            }
        }
    }

    /// The bullet or number of every paragraph in order (`None` for non-list paragraphs).
    /// One O(paragraphs) pass; run only when the structure or list settings change.
    pub fn list_labels(&self) -> Vec<Option<String>> {
        crate::lists::number_items(self.paragraphs().map(|p| p.props.list.as_ref()))
    }

    /// Index of the section paragraph `index` belongs to: the number of section starts at or
    /// before it, not counting paragraph 0, clamped to the sections that exist. O(index).
    pub fn section_of(&self, index: usize) -> usize {
        // One section (the usual document): nothing to count, so skip the O(index) walk
        // that otherwise runs twice on every keystroke.
        if self.sections.len() <= 1 {
            return 0;
        }
        // Cell paragraphs cannot open a section; each flat paragraph up to `index` that is
        // top-level and carries `section_start` starts one.
        let mut starts = 0usize;
        for i in 1..=index.min(self.flat.len().saturating_sub(1)) {
            let Some(location) = self.locations.get(&self.flat[i]) else {
                continue;
            };
            if location.table.is_some() {
                continue;
            }
            if self
                .paragraph(self.flat[i])
                .is_some_and(|p| p.props.section_start)
            {
                starts += 1;
            }
        }
        starts.min(self.sections.len().saturating_sub(1))
    }

    /// The document's page setup: section 0's, which every section shares until per-section
    /// geometry arrives (see [`Section`]).
    pub fn page_setup(&self) -> PageSetup {
        self.sections
            .first()
            .map_or_else(PageSetup::default, |s| s.page)
    }

    /// The props of paragraph `id`, if it is one.
    pub fn para_props(&self, id: BlockId) -> Option<ParaProps> {
        self.paragraph(id).map(|p| p.props)
    }

    fn paragraph_mut(&mut self, id: BlockId) -> Result<&mut Paragraph, ModelError> {
        let location = *self
            .locations
            .get(&id)
            .ok_or(ModelError::BlockNotFound(id))?;
        let block = self
            .body
            .at_mut(location.block)
            .ok_or(ModelError::BlockNotFound(id))?;
        match location.table {
            None => block.as_paragraph_mut().ok_or(ModelError::NotParagraph(id)),
            Some(cell) => block
                .as_table_mut()
                .ok_or(ModelError::NotParagraph(id))?
                .cell_mut(cell.row, cell.cell)
                .and_then(|c| c.paragraphs.get_mut(cell.paragraph))
                .ok_or(ModelError::BlockNotFound(id)),
        }
    }

    /// Applies a sequence in order, returning the inverses in the same order. Applying those
    /// inverses in reverse restores the original document.
    pub fn apply_transaction(&mut self, ops: Vec<Operation>) -> Result<Vec<Operation>, ModelError> {
        let mut inverses = Vec::with_capacity(ops.len());
        for op in ops {
            inverses.push(self.apply(op)?);
        }
        Ok(inverses)
    }

    /// Recomputes the cached word count from scratch (used after bulk import).
    pub fn recount_words(&mut self) {
        self.word_count = self.body.iter().map(Block::word_count).sum();
    }

    /// Structural equality, ignoring paragraph edit versions and the id allocator.
    ///
    /// Undo increments versions again (they are cache keys, not content), so this is the
    /// right comparison for "apply then invert restores the original".
    pub fn content_eq(&self, other: &Self) -> bool {
        self.word_count == other.word_count
            && self.sections == other.sections
            && self.styles == other.styles
            && self.body.iter().count() == other.body.iter().count()
            && self
                .body
                .iter()
                .zip(other.body.iter())
                .all(|(a, b)| a.content_eq(b))
    }
}

impl ChangeSet {
    /// Derives the set of blocks an operation touched.
    pub fn from_operation(op: &Operation) -> Self {
        let mut set = ChangeSet::default();
        match op {
            Operation::InsertText { block, .. } | Operation::DeleteRange { block, .. } => {
                set.touch(*block);
            }
            Operation::SplitBlock { block, new_id, .. } => {
                set.touch(*block);
                set.touch(*new_id);
                set.structural = true;
            }
            Operation::MergeBlocks { first } => {
                set.touch(*first);
                set.structural = true;
            }
            Operation::InsertBlock { block, .. } => {
                set.touch(block.id());
                set.structural = true;
            }
            Operation::RemoveBlock { block } => {
                set.touch(*block);
                set.structural = true;
            }
            Operation::InsertTable { table, .. } => {
                set.touch(table.id);
                set.structural = true;
            }
            Operation::RemoveTable { table } => {
                set.touch(*table);
                set.structural = true;
            }
            Operation::ReplaceTable { table, .. } => {
                set.touch(*table);
                set.structural = true;
            }
            Operation::SetParaProps { block, .. } | Operation::SetRuns { block, .. } => {
                set.touch(*block)
            }
            Operation::SetFootnote { block, .. } => set.touch(*block),
            Operation::SetNotes { block, .. } => set.touch(*block),
            Operation::SetBookmarks { block, .. } => set.touch(*block),
            Operation::SetImages { block, .. } => set.touch(*block),
            Operation::SetCrossRefs { block, .. } => set.touch(*block),
            Operation::SetDateTimeFields { block, .. } => set.touch(*block),
            Operation::SetRuby { block, .. } => set.touch(*block),
            // Page numbers are drawn on every page, so every page's hash changes.
            Operation::SetPageNumbering { .. } => set.sections = true,
            Operation::SetSection { .. } => set.sections = true,
            Operation::SetStyle { id, .. } => set.styles.push(*id),
            // Every paragraph resolves through the defaults.
            Operation::SetParaDefaults { .. } | Operation::SetCharDefaults { .. } => {
                set.all_styles = true
            }
        }
        set
    }
}

/// Gives every block and cell paragraph in `block` a unique id, reusing the existing one
/// when it is free (DOCX/ODT import and tests may supply ids).
fn reassign_ids(block: &mut Block, next_id: &mut u64, taken: &mut HashSet<u64>) {
    match block {
        Block::Paragraph(p) => p.id = unique_id(p.id, next_id, taken),
        Block::Table(t) => {
            t.id = unique_id(t.id, next_id, taken);
            for row in &mut t.rows {
                for cell in &mut row.cells {
                    for p in &mut cell.paragraphs {
                        p.id = unique_id(p.id, next_id, taken);
                    }
                }
            }
        }
    }
}

fn unique_id(candidate: BlockId, next_id: &mut u64, taken: &mut HashSet<u64>) -> BlockId {
    if candidate.0 != 0 && taken.insert(candidate.0) {
        *next_id = (*next_id).max(candidate.0 + 1);
        return candidate;
    }
    while !taken.insert(*next_id) {
        *next_id += 1;
    }
    let id = BlockId(*next_id);
    *next_id += 1;
    id
}
