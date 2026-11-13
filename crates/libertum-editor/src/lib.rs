//! `libertum-editor` — selection, movement, typing, deletion and plain-text clipboard
//! (PLAN §4.3). UI-agnostic: it can be tested without a window.
//!
//! The editor owns the [`Document`] and derives every mutation from a [`DocPosition`], never
//! from an absolute document offset. Visual-line movement and hit-testing need layout and are
//! wired in the app through [`Editor::set_caret`]; paragraph-level movement lives here.

use std::cmp::Ordering;
use std::time::Instant;

use std::sync::Arc;

use libertum_model::{
    Align, BOLD, Block, BlockId, Bookmark, Borders, ChangeSet, CharFormat, Document, EditKind,
    HeaderFooter, HfKind, HfVariant, History, HistoryEntry, Ligatures, LineSpacing, ListItem,
    ListStyle, MAX_LIST_LEVEL, MIN_TABLE_COLUMN_PT, ModelError, Note, NoteKind, Operation,
    PageNumbering, PageSetup, ParaFormat, ParaProps, Paragraph, REGULAR, ResolvedChar,
    ResolvedPara, Run, Section, StyleId, Table, TableCell, TableCellLocation, TableRow,
    VerticalAlign, floor_char_boundary,
};
use thiserror::Error;

mod objects;
pub use objects::{UNKNOWN_PAGE, format_date_time};
mod search;
pub use search::{SearchMatch, SearchOptions, matches_in};
use unicode_segmentation::UnicodeSegmentation;

/// A position in the document: a block plus a byte offset on a grapheme boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocPosition {
    pub block: BlockId,
    pub offset: usize,
}

/// A selection: an anchor that stays put and a moving head. `anchor == head` is a caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: DocPosition,
    pub head: DocPosition,
}

impl Selection {
    pub fn caret(at: DocPosition) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum EditorError {
    #[error(transparent)]
    Model(#[from] ModelError),
    #[error("the document has no blocks")]
    EmptyDocument,
    /// Table-of-contents entries are generated from the headings.
    #[error("table of contents entries are generated from the headings and cannot be edited")]
    ReadOnly,
    /// Merging across a section start would renumber every later section.
    #[error("a section boundary cannot be merged away")]
    SectionBoundary,
}

/// The selection's character formatting, property by property: `Some` when uniform across
/// the selection, `None` when mixed. For a caret, the formatting text typed there would get.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharSummary {
    pub family: Option<Arc<str>>,
    pub size_pt: Option<f32>,
    pub weight: Option<u16>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    pub colour: Option<[u8; 4]>,
    pub highlight: Option<Option<[u8; 4]>>,
    pub vertical: Option<VerticalAlign>,
    pub letter_spacing_pt: Option<f32>,
    pub ligatures: Option<libertum_model::Ligatures>,
}

/// The selected paragraphs' formatting, property by property: `Some` when uniform, `None`
/// when mixed (as [`CharSummary`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaSummary {
    pub style: Option<StyleId>,
    pub align: Option<Align>,
    pub indent_left_pt: Option<f32>,
    pub indent_right_pt: Option<f32>,
    pub indent_first_pt: Option<f32>,
    pub space_before_pt: Option<f32>,
    pub space_after_pt: Option<f32>,
    pub line_spacing: Option<LineSpacing>,
    pub keep_with_next: Option<bool>,
    pub keep_together: Option<bool>,
    pub widow_control: Option<bool>,
    pub page_break_before: Option<bool>,
    pub hyphenate: Option<bool>,
    /// List style (`Some(None)`: not a list).
    pub list: Option<Option<ListStyle>>,
    /// List level of the list items (`None` when mixed or not a list).
    pub list_level: Option<u8>,
    /// Border box (`Some(None)`: no borders).
    pub borders: Option<Option<Borders>>,
    /// The first selected paragraph has formatting of its own on top of its style, so
    /// "Update style to match" would change something.
    pub differs_from_style: bool,
}

impl ParaSummary {
    fn of(props: &ParaProps, r: &ResolvedPara) -> Self {
        Self {
            style: Some(props.style),
            align: Some(r.align),
            indent_left_pt: Some(r.indent_left_pt),
            indent_right_pt: Some(r.indent_right_pt),
            indent_first_pt: Some(r.indent_first_pt),
            space_before_pt: Some(r.space_before_pt),
            space_after_pt: Some(r.space_after_pt),
            line_spacing: Some(r.line_spacing),
            keep_with_next: Some(r.keep_with_next),
            keep_together: Some(r.keep_together),
            widow_control: Some(r.widow_control),
            page_break_before: Some(props.page_break_before),
            hyphenate: Some(r.hyphenate),
            list: Some(props.list.map(|l| l.style)),
            list_level: props.list.map(|l| l.level),
            borders: Some(r.borders),
            differs_from_style: false,
        }
    }

    fn merge(&mut self, props: &ParaProps, r: &ResolvedPara) {
        fn keep<T: PartialEq>(slot: &mut Option<T>, v: T) {
            if slot.as_ref() != Some(&v) {
                *slot = None;
            }
        }
        keep(&mut self.style, props.style);
        keep(&mut self.align, r.align);
        keep(&mut self.indent_left_pt, r.indent_left_pt);
        keep(&mut self.indent_right_pt, r.indent_right_pt);
        keep(&mut self.indent_first_pt, r.indent_first_pt);
        keep(&mut self.space_before_pt, r.space_before_pt);
        keep(&mut self.space_after_pt, r.space_after_pt);
        keep(&mut self.line_spacing, r.line_spacing);
        keep(&mut self.keep_with_next, r.keep_with_next);
        keep(&mut self.keep_together, r.keep_together);
        keep(&mut self.widow_control, r.widow_control);
        keep(&mut self.page_break_before, props.page_break_before);
        keep(&mut self.hyphenate, r.hyphenate);
        keep(&mut self.list, props.list.map(|l| l.style));
        keep(&mut self.borders, r.borders);
        if self.list_level != props.list.map(|l| l.level) {
            self.list_level = None;
        }
    }
}

/// One indent step (the inspector's indent buttons and Tab-style indenting): half an inch,
/// as in Word and Pages.
pub const INDENT_STEP_PT: f32 = 36.0;

impl CharSummary {
    fn of(r: &ResolvedChar) -> Self {
        Self {
            family: Some(r.family.clone()),
            size_pt: Some(r.size_pt),
            weight: Some(r.weight),
            italic: Some(r.italic),
            underline: Some(r.underline),
            strikethrough: Some(r.strikethrough),
            colour: Some(r.colour),
            highlight: Some(r.highlight),
            vertical: Some(r.vertical),
            letter_spacing_pt: Some(r.letter_spacing_pt),
            ligatures: Some(r.ligatures),
        }
    }

    /// Keeps each property only where `r` agrees with it.
    fn merge(&mut self, r: &ResolvedChar) {
        fn keep<T: PartialEq>(slot: &mut Option<T>, v: T) {
            if slot.as_ref() != Some(&v) {
                *slot = None;
            }
        }
        keep(&mut self.family, r.family.clone());
        keep(&mut self.size_pt, r.size_pt);
        keep(&mut self.weight, r.weight);
        keep(&mut self.italic, r.italic);
        keep(&mut self.underline, r.underline);
        keep(&mut self.strikethrough, r.strikethrough);
        keep(&mut self.colour, r.colour);
        keep(&mut self.highlight, r.highlight);
        keep(&mut self.vertical, r.vertical);
        keep(&mut self.letter_spacing_pt, r.letter_spacing_pt);
        keep(&mut self.ligatures, r.ligatures);
    }

    pub fn is_bold(&self) -> Option<bool> {
        self.weight.map(|w| w >= 600)
    }
}

/// The document plus cursor, selection and undo history.
pub struct Editor {
    doc: Document,
    selection: Selection,
    history: History,
    preferred_x: Option<f32>,
    /// Formatting chosen with a caret (no selection), for the next text typed *at that
    /// position*; moving the caret drops it, as in Word and Pages.
    pending: Option<(DocPosition, CharFormat)>,
    /// Every match of the query searched last, for the document revision it was built
    /// from (see `search.rs`). Behind a mutex so searching stays a `&self` operation.
    search_cache: std::sync::Mutex<Option<search::MatchCache>>,
}

impl Editor {
    pub fn new(doc: Document) -> Self {
        let at = first_position(&doc).unwrap_or(DocPosition {
            block: BlockId(0),
            offset: 0,
        });
        Self {
            doc,
            selection: Selection::caret(at),
            history: History::default(),
            preferred_x: None,
            pending: None,
            search_cache: std::sync::Mutex::new(None),
        }
    }

    /// Records the document's last save time (unix seconds), which is what an "updated date
    /// and time" field shows. Bookkeeping: it is not an edit and does not enter undo.
    pub fn mark_saved_at(&mut self, seconds: i64) {
        self.doc.set_saved_at(seconds);
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Mutable access for the app to load or replace a document. Clears history and moves the
    /// caret to the first block.
    pub fn replace_document(&mut self, doc: Document) {
        self.doc = doc;
        // Another document may carry the same revision number: the match list is not its.
        *self.search_cache.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.history = History::default();
        self.preferred_x = None;
        let at = first_position(&self.doc).unwrap_or(DocPosition {
            block: BlockId(0),
            offset: 0,
        });
        self.selection = Selection::caret(at);
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn caret(&self) -> DocPosition {
        self.selection.head
    }

    pub fn has_selection(&self) -> bool {
        !self.selection.is_collapsed()
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// Set the caret, extending the selection if `extend`.
    pub fn set_caret(&mut self, pos: DocPosition, extend: bool) {
        let pos = self.clamp(pos);
        self.preferred_x = None;
        if extend {
            // A table cell bounds a selection: extending into another cell would make
            // typing and deleting across cells undefined.
            if !self.same_cell(self.selection.anchor, pos) {
                return;
            }
            self.selection.head = pos;
        } else {
            self.selection = Selection::caret(pos);
        }
    }

    /// Moves the caret for Up/Down: like [`set_caret`](Self::set_caret), but remembers `x`
    /// as the preferred column for the next vertical move.
    pub fn set_caret_vertical(&mut self, pos: DocPosition, extend: bool, x: f32) {
        self.set_caret(pos, extend);
        self.preferred_x = Some(x);
    }

    /// Set both ends (also used by tests and hit-testing with shift-click).
    pub fn set_selection(&mut self, anchor: DocPosition, head: DocPosition) {
        let anchor = self.clamp(anchor);
        let head = self.clamp(head);
        self.selection = if self.same_cell(anchor, head) {
            Selection { anchor, head }
        } else {
            // Cross-cell selections are not editable; keep the caret where the pointer is.
            Selection::caret(head)
        };
        self.preferred_x = None;
    }

    pub fn select_all(&mut self) {
        // Inside a table, Ctrl/Cmd+A selects the caret's cell, not the whole document.
        if let Some(cell) = self.caret_cell()
            && let Some(cells) = self
                .doc
                .table(cell.table)
                .and_then(|t| t.cell(cell.row, cell.cell))
            && let (Some(first), Some(last)) = (cells.paragraphs.first(), cells.paragraphs.last())
        {
            self.selection = Selection {
                anchor: DocPosition {
                    block: first.id,
                    offset: 0,
                },
                head: DocPosition {
                    block: last.id,
                    offset: last.len(),
                },
            };
            self.preferred_x = None;
            return;
        }
        if let (Some(first), Some(last)) = (self.doc.first_id(), self.doc.last_id()) {
            let end = self.doc.paragraph(last).map_or(0, |p| p.len());
            self.selection = Selection {
                anchor: DocPosition {
                    block: first,
                    offset: 0,
                },
                head: DocPosition {
                    block: last,
                    offset: end,
                },
            };
            self.preferred_x = None;
        }
    }

    /// The selection in document order.
    pub fn ordered_selection(&self) -> Option<(DocPosition, DocPosition)> {
        let a = self.selection.anchor;
        let b = self.selection.head;
        match self.compare(a, b)? {
            Ordering::Greater => Some((b, a)),
            _ => Some((a, b)),
        }
    }

    /// The selected text, with blocks joined by `\n`. `None` for a collapsed selection.
    pub fn selected_text(&self) -> Option<String> {
        let (start, end) = self.ordered_selection()?;
        if start == end {
            return None;
        }
        let mut out = String::new();
        let first_index = self.doc.index_of(start.block)?;
        let last_index = self.doc.index_of(end.block)?;
        for i in first_index..=last_index {
            let p = self.doc.paragraph_at(i)?;
            let from = if i == first_index {
                start.offset.min(p.len())
            } else {
                0
            };
            let to = if i == last_index {
                end.offset.min(p.len())
            } else {
                p.len()
            };
            if from < to {
                out.push_str(&p.text()[from..to]);
            }
            if i < last_index {
                out.push('\n');
            }
        }
        Some(out)
    }

    // ------------------------------------------------------------ editing

    /// Inserts text at the caret (or replaces the selection). `\n` splits a paragraph.
    pub fn insert_text(&mut self, text: &str) -> Result<ChangeSet, EditorError> {
        self.insert_text_at(Instant::now(), text)
    }

    /// Same as [`insert_text`](Self::insert_text), with an explicit clock for coalescing tests.
    pub fn insert_text_at(&mut self, now: Instant, text: &str) -> Result<ChangeSet, EditorError> {
        let text = normalise_newlines(text);
        if text.is_empty() {
            return Ok(ChangeSet::default());
        }
        self.guard_caret()?;
        if text == "\n" && !self.has_selection() && self.caret_in_empty_list_item() {
            // Return on an empty list item ends the list there (Word, Pages).
            return self.set_list(None);
        }
        let pending = self.take_pending();
        let mut b = EditBuilder::default();
        let mut caret = self.delete_selection_into(&mut b)?;
        let mut dirty_blocks = b.dirty.clone();
        let mut inserted: Vec<(BlockId, std::ops::Range<usize>)> = Vec::new();

        for (i, piece) in text.split('\n').enumerate() {
            if i > 0 {
                let new_id = self.doc.alloc_id();
                let props = self.split_tail_props(caret);
                b.apply(
                    &mut self.doc,
                    Operation::SplitBlock {
                        block: caret.block,
                        offset: caret.offset,
                        new_id,
                        props,
                    },
                )?;
                caret = DocPosition {
                    block: new_id,
                    offset: 0,
                };
            }
            if !piece.is_empty() {
                b.apply(
                    &mut self.doc,
                    Operation::InsertText {
                        block: caret.block,
                        offset: caret.offset,
                        text: piece.to_owned(),
                        format: None,
                    },
                )?;
                inserted.push((caret.block, caret.offset..caret.offset + piece.len()));
                caret.offset += piece.len();
            }
        }
        if let Some(patch) = pending {
            for (block, range) in inserted {
                if let Some(p) = self.doc.paragraph(block) {
                    let runs = p.runs_with(range, &patch);
                    b.apply(&mut self.doc, Operation::SetRuns { block, runs })?;
                }
            }
        }
        self.sync_toc(&mut b)?;
        dirty_blocks.merge(b.dirty.clone());

        let kind = if b.forward.len() == 1
            && !text.contains('\n')
            && text.chars().count() == 1
            && self.selection.is_collapsed()
        {
            EditKind::Typing
        } else {
            EditKind::Other
        };
        b.record(&mut self.history, kind, now);
        self.selection = Selection::caret(caret);
        self.preferred_x = None;
        Ok(dirty_blocks)
    }

    /// Backspace: delete the selection, the previous grapheme, or merge with the block above.
    pub fn delete_backward(&mut self) -> Result<ChangeSet, EditorError> {
        self.delete_backward_at(Instant::now())
    }

    pub fn delete_backward_at(&mut self, now: Instant) -> Result<ChangeSet, EditorError> {
        if self.has_selection() {
            return self.delete_selection_transaction(now);
        }
        self.guard_caret()?;
        let caret = self.clamp(self.selection.head);
        let Some(text) = self.paragraph_text(caret.block) else {
            return Ok(ChangeSet::default());
        };
        let mut b = EditBuilder::default();
        if caret.offset > 0 {
            let start = floor_char_boundary(text, prev_grapheme(text, caret.offset));
            b.apply(
                &mut self.doc,
                Operation::DeleteRange {
                    block: caret.block,
                    range: start..caret.offset,
                },
            )?;
            self.selection = Selection::caret(DocPosition {
                block: caret.block,
                offset: start,
            });
            self.commit(&mut b, EditKind::Typing, now)?;
        } else if let Some(props) = self
            .doc
            .para_props(caret.block)
            .filter(|p| p.list.is_some())
        {
            // Backspace at the start of a list item removes its bullet or number first.
            b.apply(
                &mut self.doc,
                Operation::SetParaProps {
                    block: caret.block,
                    props: ParaProps {
                        list: None,
                        ..props
                    },
                },
            )?;
            self.commit(&mut b, EditKind::Other, now)?;
        } else if let Some(props) = self
            .doc
            .para_props(caret.block)
            .filter(|p| p.page_break_before && !p.section_start)
        {
            // Backspace at the start of a page-broken paragraph removes the break first, the
            // way it deletes the break character in Word.
            b.apply(
                &mut self.doc,
                Operation::SetParaProps {
                    block: caret.block,
                    props: ParaProps {
                        page_break_before: false,
                        ..props
                    },
                },
            )?;
            self.commit(&mut b, EditKind::Other, now)?;
        } else if let Some((table, row, index)) = self.previous_cell_target() {
            // At the start of a cell, Backspace moves to the previous cell rather than
            // merging across the cell boundary (which v1 does not model).
            self.move_to_cell(table, row, index, true);
        } else if let Some(prev) = self.previous_block(caret.block) {
            self.guard_merge(prev, caret.block)?;
            let end = self.paragraph_text(prev).map_or(0, str::len);
            b.apply(&mut self.doc, Operation::MergeBlocks { first: prev })?;
            self.selection = Selection::caret(DocPosition {
                block: prev,
                offset: end,
            });
            self.commit(&mut b, EditKind::Other, now)?;
        }
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Delete: remove the selection, the next grapheme, or merge with the block below.
    pub fn delete_forward(&mut self) -> Result<ChangeSet, EditorError> {
        self.delete_forward_at(Instant::now())
    }

    pub fn delete_forward_at(&mut self, now: Instant) -> Result<ChangeSet, EditorError> {
        if self.has_selection() {
            return self.delete_selection_transaction(now);
        }
        self.guard_caret()?;
        let caret = self.clamp(self.selection.head);
        let Some(text) = self.paragraph_text(caret.block) else {
            return Ok(ChangeSet::default());
        };
        let mut b = EditBuilder::default();
        if caret.offset < text.len() {
            let end = next_grapheme(text, caret.offset);
            b.apply(
                &mut self.doc,
                Operation::DeleteRange {
                    block: caret.block,
                    range: caret.offset..end,
                },
            )?;
            self.commit(&mut b, EditKind::Typing, now)?;
        } else if let Some((table, row, index)) = self.next_cell_target() {
            // At the end of a cell, Delete moves to the next cell rather than merging
            // across the cell boundary.
            self.move_to_cell(table, row, index, false);
        } else if let Some(next) = self.next_block(caret.block) {
            self.guard_merge(caret.block, next)?;
            b.apply(&mut self.doc, Operation::MergeBlocks { first: caret.block })?;
            self.commit(&mut b, EditKind::Other, now)?;
        }
        self.preferred_x = None;
        Ok(b.dirty)
    }

    pub fn delete_word_backward(&mut self) -> Result<ChangeSet, EditorError> {
        if self.has_selection() {
            return self.delete_selection_transaction(Instant::now());
        }
        self.guard_caret()?;
        let caret = self.clamp(self.selection.head);
        let Some(text) = self.paragraph_text(caret.block) else {
            return Ok(ChangeSet::default());
        };
        let start = prev_word_boundary(text, caret.offset);
        if start == caret.offset {
            return self.delete_backward();
        }
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::DeleteRange {
                block: caret.block,
                range: start..caret.offset,
            },
        )?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.selection = Selection::caret(DocPosition {
            block: caret.block,
            offset: start,
        });
        self.preferred_x = None;
        Ok(b.dirty)
    }

    pub fn delete_word_forward(&mut self) -> Result<ChangeSet, EditorError> {
        if self.has_selection() {
            return self.delete_selection_transaction(Instant::now());
        }
        self.guard_caret()?;
        let caret = self.clamp(self.selection.head);
        let Some(text) = self.paragraph_text(caret.block) else {
            return Ok(ChangeSet::default());
        };
        let end = next_word_boundary(text, caret.offset);
        if end == caret.offset {
            return self.delete_forward();
        }
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::DeleteRange {
                block: caret.block,
                range: caret.offset..end,
            },
        )?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Deletes the selection as one transaction and collapses the caret at its start.
    fn delete_selection_transaction(&mut self, now: Instant) -> Result<ChangeSet, EditorError> {
        let mut b = EditBuilder::default();
        let caret = self.delete_selection_into(&mut b)?;
        self.commit(&mut b, EditKind::Other, now)?;
        self.selection = Selection::caret(caret);
        self.preferred_x = None;
        Ok(b.dirty)
    }

    fn delete_selection_into(&mut self, b: &mut EditBuilder) -> Result<DocPosition, EditorError> {
        let caret = self.clamp(self.selection.head);
        let Some((start, end)) = self.ordered_selection() else {
            return Ok(caret);
        };
        if start == end {
            return Ok(start);
        }
        let Some(si) = self.doc.index_of(start.block) else {
            return Ok(start);
        };
        let Some(ei) = self.doc.index_of(end.block) else {
            return Ok(start);
        };
        self.guard_range(si, ei)?;
        if si == ei {
            if start.offset < end.offset {
                b.apply(
                    &mut self.doc,
                    Operation::DeleteRange {
                        block: start.block,
                        range: start.offset..end.offset,
                    },
                )?;
            }
            return Ok(start);
        }

        // Tail of the first block.
        let start_len = self.paragraph_text(start.block).map_or(0, str::len);
        if start.offset < start_len {
            b.apply(
                &mut self.doc,
                Operation::DeleteRange {
                    block: start.block,
                    range: start.offset..start_len,
                },
            )?;
        }
        // Head of the last block.
        if end.offset > 0 {
            b.apply(
                &mut self.doc,
                Operation::DeleteRange {
                    block: end.block,
                    range: 0..end.offset,
                },
            )?;
        }
        // Blocks in between, removed from the back so indices stay valid.
        for index in (si + 1..ei).rev() {
            if let Some(id) = self.doc.body().id_at(index) {
                b.apply(&mut self.doc, Operation::RemoveBlock { block: id })?;
            }
        }
        // Bring the surviving tail of the last block up into the first block.
        if self.doc.index_of(end.block).is_some() {
            b.apply(&mut self.doc, Operation::MergeBlocks { first: start.block })?;
        }
        Ok(start)
    }

    /// Ctrl/Cmd+Enter: splits the paragraph at the caret (replacing any selection) and starts
    /// the second half on a new page.
    pub fn insert_page_break(&mut self) -> Result<ChangeSet, EditorError> {
        self.guard_caret()?;
        let mut b = EditBuilder::default();
        let caret = self.delete_selection_into(&mut b)?;
        let new_id = self.doc.alloc_id();
        b.apply(
            &mut self.doc,
            Operation::SplitBlock {
                block: caret.block,
                offset: caret.offset,
                new_id,
                props: None,
            },
        )?;
        let props = self.doc.para_props(new_id).unwrap_or_default();
        b.apply(
            &mut self.doc,
            Operation::SetParaProps {
                block: new_id,
                props: ParaProps {
                    page_break_before: true,
                    ..props
                },
            },
        )?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.selection = Selection::caret(DocPosition {
            block: new_id,
            offset: 0,
        });
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Rebuilds the table of contents from the headings without recording an undo step.
    /// For a document that was just created or opened (templates, importers), so the TOC
    /// matches before the first edit.
    pub fn refresh_table_of_contents(&mut self) -> Result<ChangeSet, EditorError> {
        let mut b = EditBuilder::default();
        b.dirty.structural = true;
        self.sync_toc(&mut b)?;
        b.dirty.structural = !b.forward.is_empty();
        Ok(b.dirty)
    }

    /// Inserts a table of contents at the caret: a "Contents" title followed by one entry per
    /// chapter heading. Existing entries are left alone; a second TOC is not inserted.
    pub fn insert_table_of_contents(&mut self) -> Result<ChangeSet, EditorError> {
        if self
            .doc
            .paragraphs()
            .any(|paragraph| paragraph.props.toc_target.is_some())
        {
            return Ok(ChangeSet::default());
        }
        let index = self
            .doc
            .index_of(self.selection.head.block)
            .unwrap_or(self.doc.body().len());
        let mut b = EditBuilder::default();
        let title = self.doc.alloc_id();
        b.apply(
            &mut self.doc,
            Operation::InsertBlock {
                index,
                block: Block::Paragraph(Paragraph::new(title, "Contents").with_props(ParaProps {
                    style: StyleId::TITLE,
                    ..ParaProps::default()
                })),
            },
        )?;
        // A zero-target entry marks the TOC start; sync_toc replaces it with the chapter
        // entries and keeps them up to date afterwards.
        let marker = self.doc.alloc_id();
        b.apply(
            &mut self.doc,
            Operation::InsertBlock {
                index: index + 1,
                block: Block::Paragraph(Paragraph::new(marker, "").with_props(ParaProps {
                    toc_target: Some(BlockId(0)),
                    ..ParaProps::default()
                })),
            },
        )?;
        self.sync_toc(&mut b)?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.selection = Selection::caret(DocPosition {
            block: title,
            offset: 0,
        });
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Attaches an empty footnote to the caret paragraph. One footnote per paragraph for now;
    /// calling it again leaves the existing note alone.
    /// Attaches an empty footnote at the caret. Repeated calls add more notes to the same
    /// paragraph.
    pub fn insert_footnote(&mut self) -> Result<ChangeSet, EditorError> {
        self.insert_note(NoteKind::Footnote)
    }

    /// Attaches an empty endnote at the caret.
    pub fn insert_endnote(&mut self) -> Result<ChangeSet, EditorError> {
        self.insert_note(NoteKind::Endnote)
    }

    fn insert_note(&mut self, kind: NoteKind) -> Result<ChangeSet, EditorError> {
        let block = self.selection.head.block;
        let offset = self.selection.head.offset;
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        let mut notes = paragraph.notes().to_vec();
        notes.push(Note {
            kind,
            offset: offset.min(paragraph.len()),
            text: String::new(),
        });
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetNotes { block, notes })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Replaces the caret paragraph's first footnote text.
    pub fn set_footnote_text(&mut self, text: String) -> Result<ChangeSet, EditorError> {
        self.set_note_text(NoteKind::Footnote, text)
    }

    fn set_note_text(&mut self, kind: NoteKind, text: String) -> Result<ChangeSet, EditorError> {
        let block = self.selection.head.block;
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        let mut notes = paragraph.notes().to_vec();
        let Some(note) = notes.iter_mut().find(|note| note.kind == kind) else {
            return Ok(ChangeSet::default());
        };
        if note.text == text {
            return Ok(ChangeSet::default());
        }
        note.text = text;
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetNotes { block, notes })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Removes the caret paragraph's first footnote.
    pub fn remove_footnote(&mut self) -> Result<ChangeSet, EditorError> {
        let block = self.selection.head.block;
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        let mut notes = paragraph.notes().to_vec();
        let before = notes.len();
        notes.retain(|note| note.kind != NoteKind::Footnote);
        if notes.len() == before {
            return Ok(ChangeSet::default());
        }
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetNotes { block, notes })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// The caret paragraph's first footnote text.
    pub fn footnote(&self) -> Option<&str> {
        self.doc
            .paragraph(self.selection.head.block)
            .and_then(Paragraph::note)
    }

    /// All notes on the caret paragraph.
    pub fn notes(&self) -> &[Note] {
        self.doc
            .paragraph(self.selection.head.block)
            .map_or(&[], Paragraph::notes)
    }

    /// Replaces note `index` in `block` with `text` (used by the Notes pane).
    pub fn set_note_at(
        &mut self,
        block: BlockId,
        index: usize,
        text: String,
    ) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        let mut notes = paragraph.notes().to_vec();
        let Some(note) = notes.get_mut(index) else {
            return Ok(ChangeSet::default());
        };
        if note.text == text {
            return Ok(ChangeSet::default());
        }
        note.text = text;
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetNotes { block, notes })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// The 1-based number of a note; footnote and endnote numbers are separate.
    pub fn note_number(&self, kind: NoteKind, block: BlockId, offset: usize) -> Option<u32> {
        let mut number = 0u32;
        for paragraph in self.doc.paragraphs() {
            for note in paragraph.notes() {
                if note.kind == kind {
                    number += 1;
                    if paragraph.id == block && note.offset == offset {
                        return Some(number);
                    }
                }
            }
        }
        None
    }

    /// The 1-based footnote number of paragraph `block`'s first footnote.
    pub fn footnote_number(&self, block: BlockId) -> Option<u32> {
        let paragraph = self.doc.paragraph(block)?;
        let note = paragraph
            .notes()
            .iter()
            .find(|note| note.kind == NoteKind::Footnote)?;
        self.note_number(NoteKind::Footnote, block, note.offset)
    }

    // ------------------------------------------------------------ bookmarks

    /// Every bookmark in document order, with its paragraph and index in that paragraph.
    pub fn bookmarks(&self) -> Vec<(BlockId, usize, Bookmark)> {
        let mut out = Vec::new();
        for paragraph in self.doc.paragraphs() {
            for (index, bookmark) in paragraph.bookmarks().iter().enumerate() {
                out.push((paragraph.id, index, bookmark.clone()));
            }
        }
        out
    }

    /// Where a bookmark points, if the name exists.
    pub fn bookmark_position(&self, name: &str) -> Option<DocPosition> {
        self.doc.paragraphs().find_map(|paragraph| {
            paragraph.bookmark(name).map(|bookmark| DocPosition {
                block: paragraph.id,
                offset: bookmark.offset,
            })
        })
    }

    /// A fresh, unused name for a new bookmark: `Bookmark_1`, `Bookmark_2`, ...
    pub fn next_bookmark_name(&self) -> String {
        let mut number = 1u32;
        loop {
            let name = format!("Bookmark_{number}");
            if !self.bookmark_taken(&name, None) {
                return name;
            }
            number += 1;
        }
    }

    /// Adds a bookmark at the caret and returns the name actually used (duplicates and
    /// invalid characters are repaired). Bookmark names are unique, case-insensitively.
    pub fn insert_bookmark(&mut self, name: &str) -> Result<(ChangeSet, String), EditorError> {
        let block = self.selection.head.block;
        let offset = self.selection.head.offset;
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok((ChangeSet::default(), name.to_owned()));
        };
        if paragraph.props.toc_target.is_some() {
            return Err(EditorError::ReadOnly);
        }
        let offset = offset.min(paragraph.len());
        let name = self.unique_bookmark_name(name, None);
        let mut bookmarks = paragraph.bookmarks().to_vec();
        bookmarks.push(Bookmark {
            name: name.clone(),
            offset,
        });
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetBookmarks { block, bookmarks })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok((b.dirty, name))
    }

    /// Renames bookmark `index` in `block`. The new name is repaired like an inserted one.
    pub fn rename_bookmark(
        &mut self,
        block: BlockId,
        index: usize,
        name: &str,
    ) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        let Some(current) = paragraph.bookmarks().get(index) else {
            return Ok(ChangeSet::default());
        };
        let old = current.name.clone();
        let name = self.unique_bookmark_name(name, Some((block, index)));
        if name == old {
            return Ok(ChangeSet::default());
        }
        let mut bookmarks = paragraph.bookmarks().to_vec();
        bookmarks[index].name = name;
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetBookmarks { block, bookmarks })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Removes bookmark `index` in `block`.
    pub fn remove_bookmark(
        &mut self,
        block: BlockId,
        index: usize,
    ) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        if index >= paragraph.bookmarks().len() {
            return Ok(ChangeSet::default());
        }
        let mut bookmarks = paragraph.bookmarks().to_vec();
        bookmarks.remove(index);
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetBookmarks { block, bookmarks })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// A unique name based on `desired`, skipping the bookmark at `skip` (itself).
    /// Scanning the document is O(paragraphs), but this only runs on an explicit action.
    fn unique_bookmark_name(&self, desired: &str, skip: Option<(BlockId, usize)>) -> String {
        let base = sanitise_bookmark_name(desired);
        let mut candidate = base.clone();
        let mut number = 2u32;
        while self.bookmark_taken(&candidate, skip) {
            let suffix = format!("_{number}");
            let keep = 40usize.saturating_sub(suffix.len());
            candidate = format!("{}{suffix}", &base[..base.len().min(keep)]);
            number += 1;
        }
        candidate
    }

    fn bookmark_taken(&self, name: &str, skip: Option<(BlockId, usize)>) -> bool {
        self.doc.paragraphs().any(|paragraph| {
            paragraph
                .bookmarks()
                .iter()
                .enumerate()
                .any(|(index, bookmark)| {
                    bookmark.name.eq_ignore_ascii_case(name) && skip != Some((paragraph.id, index))
                })
        })
    }

    // ------------------------------------------------------------ character formatting

    /// Lays `patch` over the selected text as one undoable step. With a caret, the patch is
    /// kept for the next text typed there instead (nothing changes yet). TOC entries are
    /// skipped: they are generated.
    pub fn format_selection(&mut self, patch: &CharFormat) -> Result<ChangeSet, EditorError> {
        let Some((start, end)) = self.ordered_selection().filter(|(a, b)| a != b) else {
            let caret = self.clamp(self.selection.head);
            let base = self
                .pending
                .take()
                .filter(|(at, _)| *at == caret)
                .map(|(_, f)| f)
                .unwrap_or_default();
            self.pending = Some((caret, base.overlay(patch)));
            return Ok(ChangeSet::default());
        };
        let (Some(si), Some(ei)) = (self.doc.index_of(start.block), self.doc.index_of(end.block))
        else {
            return Ok(ChangeSet::default());
        };
        let mut b = EditBuilder::default();
        for index in si..=ei {
            let Some(p) = self.doc.paragraph_at(index) else {
                continue;
            };
            if p.props.toc_target.is_some() {
                continue;
            }
            let from = if index == si { start.offset } else { 0 };
            let to = if index == ei { end.offset } else { p.len() };
            if from >= to {
                continue;
            }
            let runs = p.runs_with(from..to, patch);
            if runs != p.runs() {
                let block = p.id;
                b.apply(&mut self.doc, Operation::SetRuns { block, runs })?;
            }
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Bold on unless the whole selection is already bold, then off.
    pub fn toggle_bold(&mut self) -> Result<ChangeSet, EditorError> {
        let on = self.char_summary().is_bold() != Some(true);
        self.format_selection(&CharFormat {
            weight: Some(if on { BOLD } else { REGULAR }),
            ..CharFormat::default()
        })
    }

    pub fn toggle_italic(&mut self) -> Result<ChangeSet, EditorError> {
        let on = self.char_summary().italic != Some(true);
        self.format_selection(&CharFormat {
            italic: Some(on),
            ..CharFormat::default()
        })
    }

    pub fn toggle_underline(&mut self) -> Result<ChangeSet, EditorError> {
        let on = self.char_summary().underline != Some(true);
        self.format_selection(&CharFormat {
            underline: Some(on),
            ..CharFormat::default()
        })
    }

    pub fn toggle_strikethrough(&mut self) -> Result<ChangeSet, EditorError> {
        let on = self.char_summary().strikethrough != Some(true);
        self.format_selection(&CharFormat {
            strikethrough: Some(on),
            ..CharFormat::default()
        })
    }

    /// The selection's resolved character formatting (see [`CharSummary`]). O(selected runs
    /// and paragraphs); a caret is O(1).
    pub fn char_summary(&self) -> CharSummary {
        let styles = &self.doc.styles;
        let resolved = |p: &Paragraph, f: &CharFormat| styles.resolve_char(p.props.style).apply(f);
        let Some((start, end)) = self.ordered_selection().filter(|(a, b)| a != b) else {
            let caret = self.clamp(self.selection.head);
            let Some(p) = self.doc.paragraph(caret.block) else {
                return CharSummary::default();
            };
            let mut f = p.format_for_typing_at(caret.offset);
            if let Some((at, patch)) = &self.pending
                && *at == caret
            {
                f = f.overlay(patch);
            }
            return CharSummary::of(&resolved(p, &f));
        };
        let (Some(si), Some(ei)) = (self.doc.index_of(start.block), self.doc.index_of(end.block))
        else {
            return CharSummary::default();
        };
        let mut summary: Option<CharSummary> = None;
        for index in si..=ei {
            let Some(p) = self.doc.paragraph_at(index) else {
                continue;
            };
            let from = if index == si { start.offset } else { 0 };
            let to = if index == ei { end.offset } else { p.len() };
            if from >= to {
                continue;
            }
            // Cut the range at run boundaries; each piece has one resolved format.
            let mut cuts = vec![from, to];
            for r in p.runs() {
                for at in [r.range.start, r.range.end] {
                    if from < at && at < to {
                        cuts.push(at);
                    }
                }
            }
            cuts.sort_unstable();
            cuts.dedup();
            for w in cuts.windows(2) {
                let r = resolved(p, &p.format_of(w[0]));
                match summary.as_mut() {
                    Some(sum) => sum.merge(&r),
                    None => summary = Some(CharSummary::of(&r)),
                }
            }
        }
        summary.unwrap_or_default()
    }

    /// Superscript (or subscript) on unless the whole selection already is, then back to
    /// the baseline.
    pub fn toggle_vertical(&mut self, v: VerticalAlign) -> Result<ChangeSet, EditorError> {
        let on = self.char_summary().vertical != Some(v);
        self.format_selection(&CharFormat {
            vertical: Some(if on { v } else { VerticalAlign::Baseline }),
            ..CharFormat::default()
        })
    }

    // ------------------------------------------------------------ paragraph formatting

    /// Indices of the paragraphs the selection touches (the caret's paragraph for a caret).
    fn selected_paragraphs(&self) -> Option<(usize, usize)> {
        let (start, end) = self.ordered_selection()?;
        let si = self.doc.index_of(start.block)?;
        let ei = self.doc.index_of(end.block)?;
        Some((si.min(ei), si.max(ei)))
    }

    /// Rewrites the properties of every selected paragraph with `f` (given its resolved
    /// formatting) as one undoable step. Unchanged paragraphs record nothing.
    fn edit_paragraphs(
        &mut self,
        f: impl Fn(&ResolvedPara, ParaProps) -> ParaProps,
    ) -> Result<ChangeSet, EditorError> {
        let Some((si, ei)) = self.selected_paragraphs() else {
            return Ok(ChangeSet::default());
        };
        let mut b = EditBuilder::default();
        for index in si..=ei {
            let Some(p) = self.doc.paragraph_at(index) else {
                continue;
            };
            let resolved = self.doc.styles.resolve_props(&p.props);
            let props = f(&resolved, p.props);
            if props != p.props {
                b.toc |= props.outline_level != p.props.outline_level;
                let block = p.id;
                b.apply(&mut self.doc, Operation::SetParaProps { block, props })?;
            }
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Lays `patch` over the selected paragraphs' direct formatting as one undoable step.
    /// A value equal to what the style gives is stored anyway: it is what the user chose.
    pub fn format_paragraphs(&mut self, patch: &ParaFormat) -> Result<ChangeSet, EditorError> {
        self.edit_paragraphs(|_, props| ParaProps {
            format: props.format.overlay(patch),
            ..props
        })
    }

    /// Moves the left indent of each selected paragraph by one [`INDENT_STEP_PT`] step
    /// (`direction` +1 or -1), never past the margin.
    pub fn indent(&mut self, direction: i8) -> Result<ChangeSet, EditorError> {
        let delta = f32::from(direction.signum()) * INDENT_STEP_PT;
        self.edit_paragraphs(|r, props| {
            if let Some(item) = props.list {
                // In a list, indenting changes the item's level (Pages, Word).
                let level = (i16::from(item.level) + i16::from(direction.signum()))
                    .clamp(0, i16::from(MAX_LIST_LEVEL)) as u8;
                return ParaProps {
                    list: Some(ListItem { level, ..item }),
                    ..props
                };
            }
            let left = (r.indent_left_pt + delta).max(0.0);
            ParaProps {
                format: ParaFormat {
                    indent_left_pt: Some(left),
                    ..props.format
                },
                ..props
            }
        })
    }

    /// Page break before, on or off, for the selected paragraphs.
    pub fn set_page_break_before(&mut self, on: bool) -> Result<ChangeSet, EditorError> {
        self.edit_paragraphs(|_, props| ParaProps {
            page_break_before: on,
            ..props
        })
    }

    /// Applies paragraph style `style` to the selected paragraphs: their direct paragraph
    /// formatting is cleared (as in Pages; character formatting stays), and their heading
    /// level follows the style, so the table of contents picks up new headings.
    pub fn set_paragraph_style(&mut self, style: StyleId) -> Result<ChangeSet, EditorError> {
        if self.doc.styles.get(style).is_none() {
            return Err(EditorError::Model(ModelError::StyleNotFound(style)));
        }
        let level = self.doc.styles.outline_level(style);
        self.edit_paragraphs(|_, props| {
            if props.toc_target.is_some() {
                return props;
            }
            ParaProps {
                style,
                format: ParaFormat::default(),
                outline_level: level,
                ..props
            }
        })
    }

    /// "Update style to match": folds the first selected paragraph's own formatting into
    /// its paragraph style, then clears it from that paragraph, as one undoable step. Its
    /// character formatting moves into the style only when it is uniform across the whole
    /// paragraph. Other paragraphs of the style change with it, keeping their own overrides.
    pub fn update_style_from_selection(&mut self) -> Result<ChangeSet, EditorError> {
        let Some((si, _)) = self.selected_paragraphs() else {
            return Ok(ChangeSet::default());
        };
        let Some(p) = self.doc.paragraph_at(si) else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.styles.get(p.props.style).cloned() else {
            return Ok(ChangeSet::default());
        };
        let uniform = uniform_run_format(p);
        if p.props.format.is_empty() && uniform.is_none() {
            return Ok(ChangeSet::default());
        }
        let (block, id, props) = (p.id, p.props.style, p.props);
        let mut style = old.clone();
        style.para = old.para.overlay(&props.format);
        let mut b = EditBuilder::default();
        if let Some(f) = &uniform {
            style.char = old.char.overlay(f);
            b.apply(
                &mut self.doc,
                Operation::SetRuns {
                    block,
                    runs: Vec::new(),
                },
            )?;
        }
        b.apply(
            &mut self.doc,
            Operation::SetStyle {
                id,
                style: Box::new(style),
            },
        )?;
        if !props.format.is_empty() {
            b.apply(
                &mut self.doc,
                Operation::SetParaProps {
                    block,
                    props: ParaProps {
                        format: ParaFormat::default(),
                        ..props
                    },
                },
            )?;
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Turns hyphenation on or off for the whole document (one undoable step). Paragraphs
    /// that set their own hyphenation keep it.
    pub fn set_document_hyphenation(&mut self, on: bool) -> Result<ChangeSet, EditorError> {
        let mut para = self.doc.styles.para_defaults;
        if para.hyphenate == on {
            return Ok(ChangeSet::default());
        }
        para.hyphenate = on;
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetParaDefaults { para })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// The document's page setup. Every section shares it until per-section geometry arrives.
    pub fn page_setup(&self) -> PageSetup {
        self.doc.page_setup()
    }

    /// Sets paper, orientation and margins for the whole document as one undoable step. The
    /// app draws every page from section 0's setup, so applying it to every section keeps the
    /// document consistent (and round-trippable once the format crates land).
    /// Replaces the whole-document page numbering as one undo step.
    pub fn set_page_numbering(
        &mut self,
        numbering: PageNumbering,
    ) -> Result<ChangeSet, EditorError> {
        if self.doc.has_page_numbering() && self.doc.page_numbering() == numbering {
            return Ok(ChangeSet::default());
        }
        let numbering = numbering.clamped();
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::SetPageNumbering {
                numbering: Some(numbering),
            },
        )?;
        // The format exporters carry page numbers on sections; mirror the document-wide
        // setting into every section in the same undo step, so a save keeps it.
        for (index, section) in self.doc.sections.clone().iter().enumerate() {
            let mut updated = section.clone();
            updated.page_numbers = numbering.show.then_some(numbering.position);
            updated.hide_first_page = numbering.hide_first_page;
            updated.first_page_number = Some(numbering.start_at);
            if updated != *section {
                b.apply(
                    &mut self.doc,
                    Operation::SetSection {
                        index,
                        section: updated,
                    },
                )?;
            }
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    pub fn set_page_setup(&mut self, setup: PageSetup) -> Result<ChangeSet, EditorError> {
        if self.doc.sections.iter().all(|s| s.page == setup) {
            return Ok(ChangeSet::default());
        }
        let sections: Vec<Section> = self
            .doc
            .sections
            .iter()
            .cloned()
            .map(|mut section| {
                section.page = setup;
                section
            })
            .collect();
        let mut b = EditBuilder::default();
        for (index, section) in sections.into_iter().enumerate() {
            b.apply(&mut self.doc, Operation::SetSection { index, section })?;
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// The document's watermark (the first section's; the app sets them all together).
    pub fn watermark(&self) -> Option<&libertum_model::Watermark> {
        self.doc.sections.first().and_then(|s| s.watermark.as_ref())
    }

    /// Sets (or, with `None`, removes) the watermark on every section, as one undoable step.
    pub fn set_watermark(
        &mut self,
        watermark: Option<libertum_model::Watermark>,
    ) -> Result<ChangeSet, EditorError> {
        if self.doc.sections.iter().all(|s| s.watermark == watermark) {
            return Ok(ChangeSet::default());
        }
        let sections: Vec<Section> = self
            .doc
            .sections
            .iter()
            .cloned()
            .map(|mut section| {
                section.watermark = watermark.clone();
                section
            })
            .collect();
        let mut b = EditBuilder::default();
        for (index, section) in sections.into_iter().enumerate() {
            b.apply(&mut self.doc, Operation::SetSection { index, section })?;
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Turns the document's default ligature use on (standard ligatures) or off, one undoable
    /// step. Runs that set their own ligatures keep them.
    pub fn set_document_ligatures(
        &mut self,
        ligatures: Ligatures,
    ) -> Result<ChangeSet, EditorError> {
        let mut char = self.doc.styles.defaults.clone();
        if char.ligatures == ligatures {
            return Ok(ChangeSet::default());
        }
        char.ligatures = ligatures;
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetCharDefaults { char })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Makes the selected paragraphs items of a `style` list, or (`None`) removes them from
    /// their lists, as one undoable step. They join the list of the paragraph before the
    /// selection when it has the same style (so a list continues), and otherwise start a new
    /// one. Items keep their levels; TOC entries are skipped.
    pub fn set_list(&mut self, style: Option<ListStyle>) -> Result<ChangeSet, EditorError> {
        let Some((si, _)) = self.selected_paragraphs() else {
            return Ok(ChangeSet::default());
        };
        let group = style.map(|style| {
            let before = si
                .checked_sub(1)
                .and_then(|i| self.doc.paragraph_at(i))
                .and_then(|p| p.props.list)
                .filter(|l| l.style == style);
            let current = self
                .doc
                .paragraph_at(si)
                .and_then(|p| p.props.list)
                .filter(|l| l.style == style);
            before
                .or(current)
                .map_or_else(|| self.doc.alloc_id().0, |l| l.group)
        });
        self.edit_paragraphs(|_, props| {
            if props.toc_target.is_some() {
                return props;
            }
            let list = match (style, group) {
                (Some(style), Some(group)) => Some(ListItem {
                    style,
                    level: props.list.map_or(0, |l| l.level),
                    group,
                }),
                _ => None,
            };
            ParaProps { list, ..props }
        })
    }

    /// True when the caret is in an empty list item.
    fn caret_in_empty_list_item(&self) -> bool {
        let caret = self.clamp(self.selection.head);
        self.doc
            .paragraph(caret.block)
            .is_some_and(|p| p.is_empty() && p.props.list.is_some())
    }

    /// True when Tab should change the list level: the caret is at the start of a list item,
    /// or the selection covers list items.
    pub fn tab_changes_list_level(&self) -> bool {
        let Some((si, ei)) = self.selected_paragraphs() else {
            return false;
        };
        let in_list = |i: usize| {
            self.doc
                .paragraph_at(i)
                .is_some_and(|p| p.props.list.is_some())
        };
        if self.has_selection() {
            return (si..=ei).any(in_list);
        }
        self.clamp(self.selection.head).offset == 0 && in_list(si)
    }

    /// The selected paragraphs' resolved formatting (see [`ParaSummary`]). O(selected
    /// paragraphs).
    pub fn para_summary(&self) -> ParaSummary {
        let Some((si, ei)) = self.selected_paragraphs() else {
            return ParaSummary::default();
        };
        let styles = &self.doc.styles;
        let mut summary: Option<ParaSummary> = None;
        for index in si..=ei {
            let Some(p) = self.doc.paragraph_at(index) else {
                continue;
            };
            let r = styles.resolve_props(&p.props);
            match summary.as_mut() {
                Some(sum) => sum.merge(&p.props, &r),
                None => {
                    let mut sum = ParaSummary::of(&p.props, &r);
                    sum.differs_from_style =
                        !p.props.format.is_empty() || uniform_run_format(p).is_some();
                    summary = Some(sum);
                }
            }
        }
        summary.unwrap_or_default()
    }

    /// Properties for the paragraph Return starts at `at`: at the end of a paragraph whose
    /// style names a next style (a heading followed by body text), that style with no direct
    /// formatting; otherwise [`ParaProps::for_split_tail`].
    fn split_tail_props(&self, at: DocPosition) -> Option<ParaProps> {
        let p = self.doc.paragraph(at.block)?;
        let mut tail = p.props.for_split_tail(at.offset, p.len());
        let next = self.doc.styles.next_style(p.props.style);
        if at.offset >= p.len() && next != p.props.style {
            tail.style = next;
            tail.outline_level = self.doc.styles.outline_level(next);
            tail.format = ParaFormat::default();
        }
        Some(tail)
    }

    /// The pending caret formatting, if the caret is still where it was chosen.
    fn take_pending(&mut self) -> Option<CharFormat> {
        let caret = self.clamp(self.selection.head);
        self.pending
            .take()
            .filter(|(at, _)| *at == caret && self.selection.is_collapsed())
            .map(|(_, f)| f)
    }

    /// Index of the section the caret is in.
    pub fn caret_section(&self) -> usize {
        self.doc
            .index_of(self.clamp(self.selection.head).block)
            .map_or(0, |i| self.doc.section_of(i))
    }

    /// Replaces section `index` as one undoable step (Insert > Page Number).
    pub fn set_section(
        &mut self,
        index: usize,
        section: Section,
    ) -> Result<ChangeSet, EditorError> {
        if self.doc.sections.get(index) == Some(&section) {
            return Ok(ChangeSet::default());
        }
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetSection { index, section })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Number of text columns in section `index`, clamped to 1..=12, as one undoable step.
    pub fn set_columns(&mut self, index: usize, columns: u8) -> Result<ChangeSet, EditorError> {
        self.edit_section_with(index, EditKind::Other, |section| {
            section.set_columns(columns)
        })
    }

    /// Space between columns in section `index`, points, as one undoable step.
    pub fn set_column_gutter(&mut self, index: usize, pt: f32) -> Result<ChangeSet, EditorError> {
        self.edit_section_with(index, EditKind::Other, |section| {
            section.set_column_gutter(pt);
        })
    }

    // ------------------------------------------------------------ headers and footers

    /// A copy of section `index` (the default section when it does not exist).
    fn section_at(&self, index: usize) -> Section {
        self.doc.sections.get(index).cloned().unwrap_or_default()
    }

    /// Applies `change` to section `index` as one undoable step. Consecutive typed changes to
    /// the same section coalesce in the history, so typing a header is one undo step.
    fn edit_section_with(
        &mut self,
        index: usize,
        kind: EditKind,
        change: impl FnOnce(&mut Section),
    ) -> Result<ChangeSet, EditorError> {
        let mut section = self.section_at(index);
        change(&mut section);
        if self.doc.sections.get(index) == Some(&section) {
            return Ok(ChangeSet::default());
        }
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetSection { index, section })?;
        self.commit(&mut b, kind, Instant::now())?;
        Ok(b.dirty)
    }

    /// Sets the text of section `index`'s header or footer variant, as one undoable step.
    /// Turning a header on by typing into it creates the default variant if there is none.
    pub fn set_header_footer_text(
        &mut self,
        index: usize,
        kind: HfKind,
        variant: HfVariant,
        text: String,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_section_with(index, EditKind::Typing, |section| {
            let set = section.header_footer_mut(kind);
            if set.default.is_none() {
                set.default = Some(HeaderFooter::new(""));
            }
            set.get_mut(variant)
                .get_or_insert_with(|| HeaderFooter::new(""))
                .text = text;
        })
    }

    /// Shows or hides section `index`'s header or footer (the Document pane's checkbox).
    /// Turning it off keeps the first-page and even-page text for when it is turned back on.
    pub fn set_header_footer_enabled(
        &mut self,
        index: usize,
        kind: HfKind,
        on: bool,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_section_with(index, EditKind::Other, |section| {
            let set = section.header_footer_mut(kind);
            if on {
                if set.default.is_none() {
                    set.default = Some(HeaderFooter::new(""));
                }
            } else {
                set.default = None;
            }
        })
    }

    /// Switches the section's first page to its own header and footer (Word's "Different
    /// first page"). Turning it on gives both an empty first-page entry to type into.
    pub fn set_different_first_page(
        &mut self,
        index: usize,
        on: bool,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_section_with(index, EditKind::Other, |section| {
            for kind in [HfKind::Header, HfKind::Footer] {
                let set = section.header_footer_mut(kind);
                set.different_first = on;
                if on {
                    set.first.get_or_insert_with(|| HeaderFooter::new(""));
                }
            }
        })
    }

    /// Switches even pages to their own header and footer (Word's "Different odd & even
    /// pages"). Turning it on gives both an empty even-page entry to type into.
    pub fn set_different_even_odd(
        &mut self,
        index: usize,
        on: bool,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_section_with(index, EditKind::Other, |section| {
            for kind in [HfKind::Header, HfKind::Footer] {
                let set = section.header_footer_mut(kind);
                set.different_even_odd = on;
                if on {
                    set.even.get_or_insert_with(|| HeaderFooter::new(""));
                }
            }
        })
    }

    /// Sets the distance from the page edge to the header (top) or footer (bottom) for the
    /// whole document, in points, as one undoable step.
    pub fn set_header_footer_distance(
        &mut self,
        kind: HfKind,
        pt: f32,
    ) -> Result<ChangeSet, EditorError> {
        let pt = pt.max(0.0);
        if self
            .doc
            .sections
            .iter()
            .all(|s| (s.header_footer_distance(kind) - pt).abs() < 0.01)
        {
            return Ok(ChangeSet::default());
        }
        let sections: Vec<Section> = self
            .doc
            .sections
            .iter()
            .cloned()
            .map(|mut section| {
                section.set_header_footer_distance(kind, pt);
                section
            })
            .collect();
        let mut b = EditBuilder::default();
        for (index, section) in sections.into_iter().enumerate() {
            b.apply(&mut self.doc, Operation::SetSection { index, section })?;
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Typing, deleting or splitting inside a generated table-of-contents entry is refused.
    fn guard_caret(&self) -> Result<(), EditorError> {
        if self.has_selection() {
            if let Some((start, end)) = self.ordered_selection()
                && let (Some(si), Some(ei)) =
                    (self.doc.index_of(start.block), self.doc.index_of(end.block))
            {
                return self.guard_range(si, ei);
            }
            return Ok(());
        }
        if self.is_toc(self.clamp(self.selection.head).block) {
            return Err(EditorError::ReadOnly);
        }
        Ok(())
    }

    /// Deleting blocks `si..=ei` may not touch a TOC entry or merge away a section start.
    fn guard_range(&self, si: usize, ei: usize) -> Result<(), EditorError> {
        for index in si..=ei {
            let Some(p) = self.doc.paragraph_at(index) else {
                continue;
            };
            if p.props.toc_target.is_some() {
                return Err(EditorError::ReadOnly);
            }
            if index > si && p.props.section_start {
                return Err(EditorError::SectionBoundary);
            }
        }
        Ok(())
    }

    fn guard_merge(&self, first: BlockId, second: BlockId) -> Result<(), EditorError> {
        if self.is_toc(first) || self.is_toc(second) {
            return Err(EditorError::ReadOnly);
        }
        if self.doc.para_props(second).is_some_and(|p| p.section_start) {
            return Err(EditorError::SectionBoundary);
        }
        Ok(())
    }

    fn is_toc(&self, id: BlockId) -> bool {
        self.doc
            .para_props(id)
            .is_some_and(|p| p.toc_target.is_some())
    }

    /// Keeps the table of contents in step with the chapter headings, inside the same
    /// transaction as the edit that changed them, so undo restores both together (a separate
    /// update would shift block indices under the undo history).
    ///
    /// The TOC is the first contiguous run of entry paragraphs. Only chapter headings
    /// (outline level 1) are listed; an empty placeholder entry (target `BlockId(0)`) keeps
    /// the TOC's place when there are none. Page numbers are not stored: layout resolves
    /// them like a page-number field. Runs only when the edit was structural or touched a
    /// heading, so ordinary typing stays O(1).
    fn sync_toc(&mut self, b: &mut EditBuilder) -> Result<(), EditorError> {
        let relevant = b.dirty.structural
            || b.toc
            || b.dirty
                .dirty
                .iter()
                .any(|id| self.doc.para_props(*id).is_some_and(|p| p.is_heading()));
        if !relevant {
            return Ok(());
        }
        let mut start = None;
        let mut entries: Vec<(BlockId, BlockId, String, ParaProps)> = Vec::new();
        for (i, block) in self.doc.body().iter().enumerate() {
            let Some(p) = block.as_paragraph() else {
                continue;
            };
            match p.props.toc_target {
                Some(target) => {
                    start.get_or_insert(i);
                    entries.push((p.id, target, p.text().to_owned(), p.props));
                }
                None if start.is_some() => break,
                None => {}
            }
        }
        let (Some(start), Some(template)) = (start, entries.first().map(|e| e.3)) else {
            return Ok(());
        };
        // Every heading level the document styles produce (Heading 1 to 4 and imported
        // deeper levels) is listed; the entry's own left indent shows its depth.
        // The "Contents" title sits just above the first entry and may use a heading style
        // for its look; it is the list's title, not an entry in it.
        let title_block = start
            .checked_sub(1)
            .and_then(|index| self.doc.body().at(index))
            .and_then(Block::as_paragraph)
            .map(|p| p.id);
        let indent_for = |level: u8| -> Option<f32> {
            (level > 1).then_some(f32::from(level.saturating_sub(1)) * 18.0)
        };
        let mut desired: Vec<(BlockId, String, u8)> = self
            .doc
            .body()
            .iter()
            .filter_map(Block::as_paragraph)
            .filter(|p| p.props.toc_target.is_none() && Some(p.id) != title_block)
            .filter_map(|p| {
                p.props
                    .outline_level
                    .filter(|level| (1..=9).contains(level))
                    .map(|level| (p.id, p.text().to_owned(), level))
            })
            .collect();
        if desired.is_empty() {
            desired.push((BlockId(0), String::new(), 1));
        }
        let unchanged = entries.len() == desired.len()
            && entries.iter().zip(&desired).all(|(e, d)| {
                e.1 == d.0 && e.2 == d.1 && e.3.format.indent_left_pt == indent_for(d.2)
            });
        if unchanged {
            return Ok(());
        }
        if entries.len() == desired.len() {
            for ((id, target, text, props), (want_target, want_text, level)) in
                entries.iter().zip(&desired)
            {
                let indent = indent_for(*level);
                if target != want_target || props.format.indent_left_pt != indent {
                    b.apply(
                        &mut self.doc,
                        Operation::SetParaProps {
                            block: *id,
                            props: ParaProps {
                                toc_target: Some(*want_target),
                                format: ParaFormat {
                                    indent_left_pt: indent,
                                    ..props.format
                                },
                                ..*props
                            },
                        },
                    )?;
                }
                if text != want_text {
                    if !text.is_empty() {
                        b.apply(
                            &mut self.doc,
                            Operation::DeleteRange {
                                block: *id,
                                range: 0..text.len(),
                            },
                        )?;
                    }
                    if !want_text.is_empty() {
                        b.apply(
                            &mut self.doc,
                            Operation::InsertText {
                                block: *id,
                                offset: 0,
                                text: want_text.clone(),
                                format: None,
                            },
                        )?;
                    }
                }
            }
            return Ok(());
        }
        for (id, ..) in entries.iter().rev() {
            b.apply(&mut self.doc, Operation::RemoveBlock { block: *id })?;
        }
        for (k, (target, text, level)) in desired.into_iter().enumerate() {
            let id = self.doc.alloc_id();
            let mut props = template;
            props.toc_target = Some(target);
            props.outline_level = None;
            props.format.indent_left_pt = indent_for(level);
            let entry = Paragraph::new(id, text).with_props(props);
            b.apply(
                &mut self.doc,
                Operation::InsertBlock {
                    index: start + k,
                    block: Block::Paragraph(entry),
                },
            )?;
        }
        Ok(())
    }

    /// Brings the TOC up to date, then records the transaction.
    fn commit(
        &mut self,
        b: &mut EditBuilder,
        kind: EditKind,
        now: Instant,
    ) -> Result<(), EditorError> {
        self.sync_toc(b)?;
        b.record(&mut self.history, kind, now);
        Ok(())
    }

    // ------------------------------------------------------------ clipboard

    /// Copy: the selected text, or `None` for a caret.
    pub fn copy(&self) -> Option<String> {
        self.selected_text()
    }

    /// Cut: returns the removed text.
    pub fn cut(&mut self) -> Result<Option<String>, EditorError> {
        let Some(text) = self.selected_text() else {
            return Ok(None);
        };
        self.delete_selection_transaction(Instant::now())?;
        Ok(Some(text))
    }

    /// Paste plain text. Rich clipboard formats are out of scope for v1 (PLAN §4.3).
    pub fn paste(&mut self, text: &str) -> Result<ChangeSet, EditorError> {
        self.insert_text(text)
    }

    // ------------------------------------------------------------ movement

    pub fn move_left(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            if let Some((start, _)) = self.ordered_selection() {
                self.selection = Selection::caret(start);
            }
            return;
        }
        if let Some(pos) = self.previous_position(self.selection.head) {
            self.move_caret_to(pos, extend);
        }
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            if let Some((_, end)) = self.ordered_selection() {
                self.selection = Selection::caret(end);
            }
            return;
        }
        if let Some(pos) = self.next_position(self.selection.head) {
            self.move_caret_to(pos, extend);
        }
    }

    pub fn move_word_left(&mut self, extend: bool) {
        let caret = self.clamp(self.selection.head);
        let Some(text) = self.paragraph_text(caret.block) else {
            return;
        };
        let offset = prev_word_boundary(text, caret.offset);
        if offset == caret.offset {
            if let Some(prev) = self.previous_block(caret.block) {
                let end = self.paragraph_text(prev).map_or(0, str::len);
                self.move_caret_to(
                    DocPosition {
                        block: prev,
                        offset: end,
                    },
                    extend,
                );
            }
        } else {
            self.move_caret_to(
                DocPosition {
                    block: caret.block,
                    offset,
                },
                extend,
            );
        }
    }

    pub fn move_word_right(&mut self, extend: bool) {
        let caret = self.clamp(self.selection.head);
        let Some(text) = self.paragraph_text(caret.block) else {
            return;
        };
        let offset = next_word_boundary(text, caret.offset);
        if offset == caret.offset {
            if let Some(next) = self.next_block(caret.block) {
                self.move_caret_to(
                    DocPosition {
                        block: next,
                        offset: 0,
                    },
                    extend,
                );
            }
        } else {
            self.move_caret_to(
                DocPosition {
                    block: caret.block,
                    offset,
                },
                extend,
            );
        }
    }

    pub fn move_line_start(&mut self, extend: bool) {
        let caret = self.clamp(self.selection.head);
        self.move_caret_to(
            DocPosition {
                block: caret.block,
                offset: 0,
            },
            extend,
        );
    }

    pub fn move_line_end(&mut self, extend: bool) {
        let caret = self.clamp(self.selection.head);
        let len = self.paragraph_text(caret.block).map_or(0, str::len);
        self.move_caret_to(
            DocPosition {
                block: caret.block,
                offset: len,
            },
            extend,
        );
    }

    /// Paragraph-level up/down. Visual-line movement is provided by the app through layout
    /// hit-testing (`set_caret`), because only layout knows where a visual line starts.
    pub fn move_paragraph(&mut self, delta: isize, extend: bool) {
        let caret = self.clamp(self.selection.head);
        let Some(index) = self.doc.index_of(caret.block) else {
            return;
        };
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.doc.paragraph_count() {
            return;
        }
        let target = target as usize;
        let Some(p) = self.doc.paragraph_at(target) else {
            return;
        };
        let offset = floor_char_boundary(p.text(), caret.offset);
        self.move_caret_to(
            DocPosition {
                block: p.id,
                offset,
            },
            extend,
        );
    }

    pub fn move_doc_start(&mut self, extend: bool) {
        if let Some(id) = self.doc.first_id() {
            self.move_caret_to(
                DocPosition {
                    block: id,
                    offset: 0,
                },
                extend,
            );
        }
    }

    pub fn move_doc_end(&mut self, extend: bool) {
        if let Some(id) = self.doc.last_id() {
            let len = self.paragraph_text(id).map_or(0, str::len);
            self.move_caret_to(
                DocPosition {
                    block: id,
                    offset: len,
                },
                extend,
            );
        }
    }

    fn move_caret_to(&mut self, pos: DocPosition, extend: bool) {
        let pos = self.clamp(pos);
        if extend {
            if !self.same_cell(self.selection.anchor, pos) {
                self.preferred_x = None;
                return;
            }
            self.selection.head = pos;
        } else {
            self.selection = Selection::caret(pos);
        }
        // A horizontal/paragraph move resets the preferred x used by visual movement later.
        self.preferred_x = None;
    }

    /// The x position (page points) that Up/Down aim for, kept across a run of vertical moves
    /// so passing through a short line does not lose the column. Any other movement or edit
    /// clears it.
    pub fn preferred_x(&self) -> Option<f32> {
        self.preferred_x
    }

    // ------------------------------------------------------------ tables

    /// Inserts a fresh `rows`×`columns` table after the caret's block (or after the table the
    /// caret is in) and puts the caret in its first cell. A paragraph follows a table at the
    /// end of the document, so there is always somewhere to type below it.
    pub fn insert_table(
        &mut self,
        rows: usize,
        columns: usize,
        width_pt: f32,
    ) -> Result<ChangeSet, EditorError> {
        let caret = self.clamp(self.selection.head);
        let block_index =
            if let Some(cell) = self.doc.location_of(caret.block).and_then(|l| l.table) {
                self.doc.body().index_of(cell.table)
            } else {
                self.doc.body().index_of(caret.block)
            };
        let Some(block_index) = block_index else {
            return Ok(ChangeSet::default());
        };
        let index = block_index + 1;
        let table = self.doc.new_table(rows, columns, width_pt);
        let first = table
            .rows
            .first()
            .and_then(|row| row.cells.first())
            .and_then(|cell| cell.paragraphs.first())
            .map(|p| p.id);
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::InsertTable { index, table })?;
        if self.doc.body().at(index + 1).is_none() {
            let id = self.doc.alloc_id();
            b.apply(
                &mut self.doc,
                Operation::InsertBlock {
                    index: index + 1,
                    block: Block::Paragraph(Paragraph::new(id, "")),
                },
            )?;
        }
        b.record(&mut self.history, EditKind::Other, Instant::now());
        if let Some(id) = first {
            self.selection = Selection::caret(DocPosition {
                block: id,
                offset: 0,
            });
        }
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Tab (or Shift+Tab) in a table cell: move to the next (previous) cell; Tab in the last
    /// cell adds a row, as Word does.
    pub fn tab_in_table(&mut self, backwards: bool) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(table) = self.doc.table(cell.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        let target = if backwards {
            previous_cell(&table, cell.row, cell.cell)
        } else {
            next_cell(&table, cell.row, cell.cell)
        };
        if let Some((row, index)) = target {
            self.move_to_cell(cell.table, row, index, backwards);
            return Ok(ChangeSet::default());
        }
        if backwards {
            return Ok(ChangeSet::default());
        }
        let columns = table.columns().max(1);
        let mut cells = Vec::with_capacity(columns);
        for _ in 0..columns {
            let id = self.doc.alloc_id();
            cells.push(TableCell::new(Paragraph::new(id, "")));
        }
        let mut new = table.clone();
        let row = new.rows.len();
        new.rows.push(TableRow {
            cells,
            min_height_pt: 0.0,
            keep_together: true,
        });
        self.replace_table_structure(table, new, row, 0)
    }

    pub fn insert_row_below(&mut self) -> Result<ChangeSet, EditorError> {
        self.insert_row(true)
    }

    pub fn insert_row_above(&mut self) -> Result<ChangeSet, EditorError> {
        self.insert_row(false)
    }

    fn insert_row(&mut self, below: bool) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(cell.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        let columns = old.columns().max(1);
        let mut cells = Vec::with_capacity(columns);
        for _ in 0..columns {
            let id = self.doc.alloc_id();
            cells.push(TableCell::new(Paragraph::new(id, "")));
        }
        let row = if below {
            (cell.row + 1).min(old.rows.len())
        } else {
            cell.row
        };
        let mut new = old.clone();
        new.rows.insert(
            row,
            TableRow {
                cells,
                min_height_pt: 0.0,
                keep_together: true,
            },
        );
        let caret_cell = cell.cell.min(columns.saturating_sub(1));
        let caret_row = if below { row } else { row + 1 };
        self.replace_table_structure(old, new, caret_row, caret_cell)
    }

    pub fn insert_column_right(&mut self) -> Result<ChangeSet, EditorError> {
        self.insert_column(true)
    }

    pub fn insert_column_left(&mut self) -> Result<ChangeSet, EditorError> {
        self.insert_column(false)
    }

    fn insert_column(&mut self, right: bool) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(cell.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        let start = old.column_of(cell.row, cell.cell).unwrap_or(0);
        let at = if right { start + 1 } else { start };
        let width = old
            .widths_pt
            .get(start)
            .copied()
            .unwrap_or(72.0)
            .max(MIN_TABLE_COLUMN_PT);
        let paragraphs: Vec<Paragraph> = (0..old.rows.len())
            .map(|_| {
                let id = self.doc.alloc_id();
                Paragraph::new(id, "")
            })
            .collect();
        let mut new = old.clone();
        insert_column_into(&mut new, at, width, &mut paragraphs.into_iter());
        let target_column = at.min(new.columns().saturating_sub(1));
        let cell_index = new
            .cell_at_column(cell.row, target_column)
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.replace_table_structure(old, new, cell.row, cell_index)
    }

    pub fn delete_row(&mut self) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(cell.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        if old.rows.len() <= 1 {
            return self.delete_table();
        }
        let mut new = old.clone();
        new.rows.remove(cell.row);
        let row = cell.row.min(new.rows.len().saturating_sub(1));
        let cell_index = cell.cell.min(new.rows[row].cells.len().saturating_sub(1));
        self.replace_table_structure(old, new, row, cell_index)
    }

    pub fn delete_column(&mut self) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(cell.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        if old.columns() <= 1 {
            return self.delete_table();
        }
        let at = old.column_of(cell.row, cell.cell).unwrap_or(0);
        let mut new = old.clone();
        remove_column_from(&mut new, at);
        let target_column = at.min(new.columns().saturating_sub(1));
        let cell_index = new
            .cell_at_column(cell.row, target_column)
            .map(|(index, _)| index)
            .unwrap_or(0);
        self.replace_table_structure(old, new, cell.row, cell_index)
    }

    pub fn delete_table(&mut self) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let block_index = self.doc.body().index_of(cell.table).unwrap_or(0);
        let next = self
            .doc
            .body()
            .at(block_index + 1)
            .and_then(Block::as_paragraph)
            .map(|p| p.id);
        let prev = block_index
            .checked_sub(1)
            .and_then(|index| self.doc.body().at(index))
            .and_then(Block::as_paragraph)
            .map(|p| p.id);
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::RemoveTable { table: cell.table })?;
        if self.doc.paragraph_count() == 0 {
            let id = self.doc.alloc_id();
            b.apply(
                &mut self.doc,
                Operation::InsertBlock {
                    index: 0,
                    block: Block::Paragraph(Paragraph::new(id, "")),
                },
            )?;
        }
        b.record(&mut self.history, EditKind::Other, Instant::now());
        let target = next
            .filter(|id| self.doc.paragraph(*id).is_some())
            .or(prev)
            .or_else(|| self.doc.first_id());
        if let Some(id) = target {
            let len = self.doc.paragraph(id).map_or(0, Paragraph::len);
            self.selection = Selection::caret(DocPosition {
                block: id,
                offset: len,
            });
        }
        self.preferred_x = None;
        Ok(b.dirty)
    }

    fn replace_table_structure(
        &mut self,
        old: Table,
        new: Table,
        row: usize,
        cell: usize,
    ) -> Result<ChangeSet, EditorError> {
        let id = old.id;
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::ReplaceTable { table: id, new })?;
        b.record(&mut self.history, EditKind::Other, Instant::now());
        let target = self
            .doc
            .table(id)
            .and_then(|t| t.cell(row, cell))
            .and_then(|c| c.paragraphs.first())
            .map(|p| DocPosition {
                block: p.id,
                offset: 0,
            });
        if let Some(pos) = target {
            self.selection = Selection::caret(pos);
        }
        self.preferred_x = None;
        Ok(b.dirty)
    }

    fn move_to_cell(&mut self, table: BlockId, row: usize, cell: usize, to_end: bool) -> bool {
        let target = self
            .doc
            .table(table)
            .and_then(|t| t.cell(row, cell))
            .and_then(|c| {
                if to_end {
                    c.paragraphs.last()
                } else {
                    c.paragraphs.first()
                }
            })
            .map(|p| DocPosition {
                block: p.id,
                offset: if to_end { p.len() } else { 0 },
            });
        match target {
            Some(pos) => {
                self.selection = Selection::caret(pos);
                self.preferred_x = None;
                true
            }
            None => false,
        }
    }

    /// The previous cell when the caret is at the start of its cell's first paragraph.
    fn previous_cell_target(&self) -> Option<(BlockId, usize, usize)> {
        let caret = self.clamp(self.selection.head);
        let cell = self.doc.location_of(caret.block)?.table?;
        if caret.offset != 0 || cell.paragraph != 0 {
            return None;
        }
        let table = self.doc.table(cell.table)?;
        let (row, index) = previous_cell(table, cell.row, cell.cell)?;
        Some((cell.table, row, index))
    }

    /// The next cell when the caret is at the end of its cell's last paragraph.
    fn next_cell_target(&self) -> Option<(BlockId, usize, usize)> {
        let caret = self.clamp(self.selection.head);
        let cell = self.doc.location_of(caret.block)?.table?;
        let table = self.doc.table(cell.table)?;
        let paragraphs = table.cell(cell.row, cell.cell)?.paragraphs.len();
        let len = self.doc.paragraph(caret.block)?.len();
        if caret.offset < len || cell.paragraph + 1 < paragraphs {
            return None;
        }
        let (row, index) = next_cell(table, cell.row, cell.cell)?;
        Some((cell.table, row, index))
    }

    // ------------------------------------------------------------ undo/redo

    pub fn undo(&mut self) -> Result<Option<ChangeSet>, EditorError> {
        let set = self.history.undo(&mut self.doc)?;
        self.after_history_change();
        Ok(set)
    }

    pub fn redo(&mut self) -> Result<Option<ChangeSet>, EditorError> {
        let set = self.history.redo(&mut self.doc)?;
        self.after_history_change();
        Ok(set)
    }

    fn after_history_change(&mut self) {
        let head = self.clamp(self.selection.head);
        let anchor = self.clamp(self.selection.anchor);
        self.selection = Selection { anchor, head };
        self.preferred_x = None;
    }

    // ------------------------------------------------------------ helpers

    fn paragraph_text(&self, block: BlockId) -> Option<&str> {
        self.doc.paragraph(block).map(|p| p.text())
    }

    /// The cell the caret is in, if it is in a table.
    pub fn caret_cell(&self) -> Option<TableCellLocation> {
        let caret = self.clamp(self.selection.head);
        self.doc.location_of(caret.block)?.table
    }

    pub fn in_table(&self) -> bool {
        self.caret_cell().is_some()
    }

    /// Whether two positions are in the same place for selection purposes: both outside
    /// tables, or both in the same cell.
    fn same_cell(&self, a: DocPosition, b: DocPosition) -> bool {
        let cell = |pos: DocPosition| {
            self.doc
                .location_of(pos.block)
                .and_then(|l| l.table)
                .map(|c| (c.table, c.row, c.cell))
        };
        cell(a) == cell(b)
    }

    fn previous_block(&self, block: BlockId) -> Option<BlockId> {
        // Paragraphs of cells covered by a vertical merge are skipped: they are not laid out.
        let mut index = self.doc.index_of(block)?;
        loop {
            index = index.checked_sub(1)?;
            let id = self.doc.paragraph_at(index)?.id;
            if !self.is_covered(id) {
                return Some(id);
            }
        }
    }

    fn next_block(&self, block: BlockId) -> Option<BlockId> {
        let mut index = self.doc.index_of(block)?;
        loop {
            index += 1;
            let id = self.doc.paragraph_at(index)?.id;
            if !self.is_covered(id) {
                return Some(id);
            }
        }
    }

    fn previous_position(&self, pos: DocPosition) -> Option<DocPosition> {
        let pos = self.clamp(pos);
        let text = self.paragraph_text(pos.block)?;
        if pos.offset > 0 {
            return Some(DocPosition {
                block: pos.block,
                offset: prev_grapheme(text, pos.offset),
            });
        }
        let prev = self.previous_block(pos.block)?;
        let end = self.paragraph_text(prev).map_or(0, str::len);
        Some(DocPosition {
            block: prev,
            offset: end,
        })
    }

    fn next_position(&self, pos: DocPosition) -> Option<DocPosition> {
        let pos = self.clamp(pos);
        let text = self.paragraph_text(pos.block)?;
        if pos.offset < text.len() {
            return Some(DocPosition {
                block: pos.block,
                offset: next_grapheme(text, pos.offset),
            });
        }
        let next = self.next_block(pos.block)?;
        Some(DocPosition {
            block: next,
            offset: 0,
        })
    }

    fn compare(&self, a: DocPosition, b: DocPosition) -> Option<Ordering> {
        match (self.doc.index_of(a.block), self.doc.index_of(b.block)) {
            (Some(ia), Some(ib)) => Some(ia.cmp(&ib).then(a.offset.cmp(&b.offset))),
            _ => None,
        }
    }

    /// Clamps a position to an existing block and a character boundary.
    fn clamp(&self, pos: DocPosition) -> DocPosition {
        if let Some(p) = self.doc.paragraph(pos.block) {
            return DocPosition {
                block: pos.block,
                offset: floor_char_boundary(p.text(), pos.offset),
            };
        }
        // A block vanished (for example after an undo): fall back to the document edge.
        if let (Some(_first), Some(last)) = (self.doc.first_id(), self.doc.last_id()) {
            let len = self.doc.paragraph(last).map_or(0, |p| p.len());
            return DocPosition {
                block: last,
                offset: len,
            };
        }
        DocPosition {
            block: BlockId(0),
            offset: 0,
        }
    }
}

// ---------------------------------------------------------------- edit builder

#[derive(Default)]
struct EditBuilder {
    forward: Vec<Operation>,
    inverse: Vec<Operation>,
    dirty: ChangeSet,
    /// A paragraph stopped (or started) being a heading: the TOC needs a look.
    toc: bool,
}

impl EditBuilder {
    fn apply(&mut self, doc: &mut Document, op: Operation) -> Result<(), EditorError> {
        let set = ChangeSet::from_operation(&op);
        let inverse = doc.apply(op.clone())?;
        self.forward.push(op);
        self.inverse.push(inverse);
        self.dirty.merge(set);
        Ok(())
    }

    fn record(&mut self, history: &mut History, kind: EditKind, now: Instant) {
        let forward = std::mem::take(&mut self.forward);
        let inverse = std::mem::take(&mut self.inverse);
        if !forward.is_empty() {
            history.record(HistoryEntry::new(forward, inverse, kind), now);
        }
    }
}

// ---------------------------------------------------------------- text helpers

/// The direct character formatting of `p` when one run covers all of its (non-empty) text.
fn uniform_run_format(p: &Paragraph) -> Option<CharFormat> {
    match p.runs() {
        [Run { range, format }] if range.start == 0 && range.end == p.len() && !p.is_empty() => {
            Some(format.clone())
        }
        _ => None,
    }
}

/// The cell after `(row, cell)` in reading order.
fn next_cell(table: &Table, row: usize, cell: usize) -> Option<(usize, usize)> {
    // Cells covered by a vertical merge are skipped.
    let open = |r: usize, c: usize| !(r > 0 && table.rows[r].cells[c].merged_into_above);
    if let Some(r) = table.rows.get(row) {
        for c in cell + 1..r.cells.len() {
            if open(row, c) {
                return Some((row, c));
            }
        }
    }
    for (r, row_ref) in table.rows.iter().enumerate().skip(row + 1) {
        for c in 0..row_ref.cells.len() {
            if open(r, c) {
                return Some((r, c));
            }
        }
    }
    None
}

/// The cell before `(row, cell)` in reading order.
fn previous_cell(table: &Table, row: usize, cell: usize) -> Option<(usize, usize)> {
    let open = |r: usize, c: usize| !(r > 0 && table.rows[r].cells[c].merged_into_above);
    for c in (0..cell.min(table.rows.get(row).map_or(0, |r| r.cells.len()))).rev() {
        if open(row, c) {
            return Some((row, c));
        }
    }
    for r in (0..row).rev() {
        for c in (0..table.rows[r].cells.len()).rev() {
            if open(r, c) {
                return Some((r, c));
            }
        }
    }
    None
}

/// Inserts a grid column at `at`; a cell that spans across the insertion point is widened
/// instead, so every row still covers the same grid.
fn insert_column_into(
    table: &mut Table,
    at: usize,
    width_pt: f32,
    paragraphs: &mut dyn Iterator<Item = Paragraph>,
) {
    let at = at.min(table.widths_pt.len());
    table
        .widths_pt
        .insert(at, width_pt.max(MIN_TABLE_COLUMN_PT));
    for row in &mut table.rows {
        let mut column = 0usize;
        let mut action = None;
        for (index, cell) in row.cells.iter().enumerate() {
            let span = usize::from(cell.col_span.max(1));
            if at > column && at < column + span {
                action = Some((index, true));
                break;
            }
            if at <= column {
                action = Some((index, false));
                break;
            }
            column += span;
        }
        match action {
            Some((index, true)) => {
                row.cells[index].col_span = (usize::from(row.cells[index].col_span.max(1)) + 1)
                    .min(usize::from(u8::MAX)) as u8;
            }
            Some((index, false)) => {
                let paragraph = paragraphs
                    .next()
                    .unwrap_or_else(|| Paragraph::new(BlockId(0), ""));
                row.cells.insert(index, TableCell::new(paragraph));
            }
            None => {
                let paragraph = paragraphs
                    .next()
                    .unwrap_or_else(|| Paragraph::new(BlockId(0), ""));
                row.cells.push(TableCell::new(paragraph));
            }
        }
    }
}

/// Removes grid column `at`; a spanning cell shrinks by one column instead.
fn remove_column_from(table: &mut Table, at: usize) {
    if table.widths_pt.len() <= 1 {
        return;
    }
    let at = at.min(table.widths_pt.len() - 1);
    table.widths_pt.remove(at);
    for row in &mut table.rows {
        let mut column = 0usize;
        let mut target = None;
        for (index, cell) in row.cells.iter().enumerate() {
            let span = usize::from(cell.col_span.max(1));
            if at >= column && at < column + span {
                target = Some((index, span));
                break;
            }
            column += span;
        }
        if let Some((index, span)) = target {
            if span > 1 {
                row.cells[index].col_span = (span - 1) as u8;
            } else if row.cells.len() > 1 {
                row.cells.remove(index);
            }
        }
    }
}

fn first_position(doc: &Document) -> Option<DocPosition> {
    Some(DocPosition {
        block: doc.first_id()?,
        offset: 0,
    })
}

fn normalise_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Repairs a user-typed bookmark name to Word's rules: ASCII letters, digits and underscores,
/// starting with a letter, at most 40 characters. Whitespace and hyphens become underscores.
fn sanitise_bookmark_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else if ch.is_whitespace() || ch == '-' {
            out.push('_');
        }
    }
    if out.is_empty() {
        return "Bookmark".to_owned();
    }
    if !out.as_bytes()[0].is_ascii_alphabetic() {
        out.insert(0, 'B');
    }
    out.truncate(40);
    out
}

/// The byte offset before the grapheme that ends at `offset`.
pub fn prev_grapheme(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset.min(text.len()));
    text[..offset]
        .graphemes(true)
        .next_back()
        .map_or(offset, |g| offset - g.len())
}

/// The byte offset after the grapheme that starts at `offset`.
pub fn next_grapheme(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset.min(text.len()));
    text[offset..]
        .graphemes(true)
        .next()
        .map_or(offset, |g| offset + g.len())
}

/// The previous word boundary at or before `offset` (`offset` itself when already on one).
pub fn prev_word_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset.min(text.len()));
    let mut prev = 0;
    for (i, _) in text.split_word_bound_indices() {
        if i >= offset {
            break;
        }
        prev = i;
    }
    prev
}

/// The next word boundary after `offset` (`offset` itself when already on the last one).
pub fn next_word_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset.min(text.len()));
    for (i, _) in text.split_word_bound_indices() {
        if i > offset {
            return i;
        }
    }
    text.len()
}

#[cfg(test)]
mod tests;
