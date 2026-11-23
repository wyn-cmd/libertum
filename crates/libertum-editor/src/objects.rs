//! Inline images, cross-references, table styles and cell merging (PLAN Phase 6).
//!
//! Every command here is one undoable transaction built from model operations, like the
//! rest of the editor. None of them is on the keystroke path: they run on explicit actions
//! (a menu item, a pane button, a save), so the cross-reference commands may scan the
//! document.

use std::sync::Arc;
use std::time::Instant;

use libertum_model::{
    BlockId, ChangeSet, CrossRef, CrossRefKind, DateTimeField, DateTimeFormat, DateTimeSource,
    EditKind, InlineImage, MAX_IMAGE_PT, MIN_IMAGE_PT, Media, MediaLink, Operation, Paragraph,
    Ruby, Run, Table, TableCell, TableStyle,
};

use super::{DocPosition, EditBuilder, Editor, EditorError, Selection};

/// Shown for a page reference whose target is not paginated yet or no longer exists.
pub const UNKNOWN_PAGE: &str = "?";

impl Editor {
    // ------------------------------------------------------------ ruby

    /// The text the Ruby field applies to: the selection when it lies in one paragraph, else
    /// the ruby the caret is in. Returns the paragraph, the byte range and the reading now set
    /// over it (empty when there is none).
    pub fn ruby_target(&self) -> Option<(BlockId, std::ops::Range<usize>, String)> {
        let (start, end) = self.ordered_selection()?;
        let paragraph = self.doc.paragraph(start.block)?;
        if start.block == end.block && start.offset < end.offset {
            let range = start.offset.min(paragraph.len())..end.offset.min(paragraph.len());
            if range.start >= range.end {
                return None;
            }
            // The reading already over exactly this text, if any.
            let current = paragraph
                .ruby()
                .iter()
                .find(|r| r.range == range)
                .map(|r| r.text.clone())
                .unwrap_or_default();
            return Some((start.block, range, current));
        }
        if start == end {
            let r = paragraph.ruby_at(start.offset)?;
            return Some((start.block, r.range.clone(), r.text.clone()));
        }
        None
    }

    /// Sets the reading over the Ruby field's target (see [`ruby_target`](Self::ruby_target)),
    /// replacing any ruby that overlaps it; an empty reading removes it. One undo step.
    pub fn set_ruby(&mut self, reading: &str) -> Result<ChangeSet, EditorError> {
        let Some((block, range, _)) = self.ruby_target() else {
            return Ok(ChangeSet::default());
        };
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        if paragraph.props.toc_target.is_some() {
            return Err(EditorError::ReadOnly);
        }
        let mut ruby: Vec<Ruby> = paragraph
            .ruby()
            .iter()
            .filter(|r| r.range.end <= range.start || r.range.start >= range.end)
            .cloned()
            .collect();
        if !reading.is_empty() {
            ruby.push(Ruby::new(range, reading));
        }
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetRuby { block, ruby })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    // ------------------------------------------------------------ images

    /// Inserts `media` as an inline image at the caret, at its natural size scaled down to
    /// fit `max_width_pt` (the text column). `name` is the file name it came from.
    pub fn insert_image(
        &mut self,
        media: Arc<Media>,
        max_width_pt: f32,
        name: &str,
    ) -> Result<ChangeSet, EditorError> {
        self.insert_image_with(media, max_width_pt, name, "")
    }

    /// Inserts a picture that stands for an online picture or video: `media` is its preview
    /// (or a placeholder), `link` the address AO3's HTML will use, `alt` its description.
    pub fn insert_linked_media(
        &mut self,
        media: Arc<Media>,
        max_width_pt: f32,
        link: &MediaLink,
        alt: &str,
    ) -> Result<ChangeSet, EditorError> {
        self.insert_image_with(media, max_width_pt, &link.name(), alt)
    }

    /// Swaps the picture of image `index` in `block` for `media` (a preview that has now been
    /// downloaded), keeping its width and the new picture's proportions.
    pub fn replace_image_media(
        &mut self,
        block: BlockId,
        index: usize,
        media: Arc<Media>,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_image(block, index, |image| {
            let (w, h) = (
                media.width_px().max(1) as f32,
                media.height_px().max(1) as f32,
            );
            image.height_pt = image.width_pt * h / w;
            image.media = media;
        })
    }

    fn insert_image_with(
        &mut self,
        media: Arc<Media>,
        max_width_pt: f32,
        name: &str,
        alt: &str,
    ) -> Result<ChangeSet, EditorError> {
        let caret = self.clamp(self.selection.head);
        let Some(paragraph) = self.doc.paragraph(caret.block) else {
            return Ok(ChangeSet::default());
        };
        if paragraph.props.toc_target.is_some() {
            return Err(EditorError::ReadOnly);
        }
        let mut image = InlineImage::fitted(caret.offset, media, max_width_pt);
        image.name = name.to_owned();
        image.alt = alt.to_owned();
        let mut images = paragraph.images().to_vec();
        // After any image already at this offset, so repeated inserts keep their order.
        let at = images.partition_point(|i| i.offset <= caret.offset);
        images.insert(at, image);
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::SetImages {
                block: caret.block,
                images,
            },
        )?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// The images of the caret's paragraph, with the paragraph's id (the inspector's Image
    /// group lists and edits these).
    pub fn caret_images(&self) -> (BlockId, &[InlineImage]) {
        let block = self.clamp(self.selection.head).block;
        let images = self.doc.paragraph(block).map_or(&[][..], Paragraph::images);
        (block, images)
    }

    /// Number of inline images in the document (O(paragraphs); tests and the status line).
    pub fn image_count(&self) -> usize {
        self.doc.paragraphs().map(|p| p.images().len()).sum()
    }

    /// Sets the width of image `index` in `block`, keeping its proportions.
    pub fn resize_image(
        &mut self,
        block: BlockId,
        index: usize,
        width_pt: f32,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_image(block, index, |image| {
            let width = if width_pt.is_finite() {
                width_pt.clamp(MIN_IMAGE_PT, MAX_IMAGE_PT)
            } else {
                image.width_pt
            };
            image.height_pt = width * image.aspect();
            image.width_pt = width;
        })
    }

    /// Sets the alternative text of image `index` in `block`.
    pub fn set_image_alt(
        &mut self,
        block: BlockId,
        index: usize,
        alt: &str,
    ) -> Result<ChangeSet, EditorError> {
        self.edit_image(block, index, |image| image.alt = alt.to_owned())
    }

    /// Removes image `index` from `block`.
    pub fn remove_image(&mut self, block: BlockId, index: usize) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        if index >= paragraph.images().len() {
            return Ok(ChangeSet::default());
        }
        let mut images = paragraph.images().to_vec();
        images.remove(index);
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetImages { block, images })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    fn edit_image(
        &mut self,
        block: BlockId,
        index: usize,
        change: impl FnOnce(&mut InlineImage),
    ) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        let mut images = paragraph.images().to_vec();
        let Some(image) = images.get_mut(index) else {
            return Ok(ChangeSet::default());
        };
        let before = image.clone();
        change(image);
        image.clamp_size();
        if *image == before {
            return Ok(ChangeSet::default());
        }
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetImages { block, images })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    // ------------------------------------------------------------ cross-references

    /// Every cross-reference in document order, with its paragraph and index in it.
    pub fn cross_refs(&self) -> Vec<(BlockId, usize, CrossRef)> {
        let mut out = Vec::new();
        for paragraph in self.doc.paragraphs() {
            for (index, cross_ref) in paragraph.cross_refs().iter().enumerate() {
                out.push((paragraph.id, index, cross_ref.clone()));
            }
        }
        out
    }

    /// What a reference to bookmark `target` shows now. `page_of` gives the page number of
    /// a paragraph (the app asks the layout); `None` there shows [`UNKNOWN_PAGE`]. Returns
    /// `None` when no bookmark has that name.
    ///
    /// A text reference shows the bookmark's paragraph, without the text of any
    /// cross-reference inside it, so a reference to its own paragraph cannot grow each time
    /// it is refreshed.
    pub fn cross_ref_text(
        &self,
        target: &str,
        kind: CrossRefKind,
        page_of: &dyn Fn(BlockId) -> Option<u32>,
    ) -> Option<String> {
        let paragraph = self
            .doc
            .paragraphs()
            .find(|p| p.bookmark(target).is_some())?;
        Some(match kind {
            CrossRefKind::Page => {
                page_of(paragraph.id).map_or_else(|| UNKNOWN_PAGE.to_owned(), |n| n.to_string())
            }
            CrossRefKind::Text => {
                let text = paragraph.text();
                let mut out = String::with_capacity(text.len());
                let mut at = 0usize;
                for r in paragraph.cross_refs() {
                    out.push_str(text.get(at..r.range.start).unwrap_or(""));
                    at = r.range.end;
                }
                out.push_str(text.get(at..).unwrap_or(""));
                let out = out.trim();
                if out.is_empty() {
                    target.to_owned()
                } else {
                    out.to_owned()
                }
            }
        })
    }

    /// Inserts a cross-reference to bookmark `target` at the caret (replacing a selection
    /// is not supported: the reference goes at the caret). Does nothing when the bookmark
    /// does not exist.
    pub fn insert_cross_ref(
        &mut self,
        target: &str,
        kind: CrossRefKind,
        page_of: &dyn Fn(BlockId) -> Option<u32>,
    ) -> Result<ChangeSet, EditorError> {
        let caret = self.clamp(self.selection.head);
        let Some(paragraph) = self.doc.paragraph(caret.block) else {
            return Ok(ChangeSet::default());
        };
        if paragraph.props.toc_target.is_some() {
            return Err(EditorError::ReadOnly);
        }
        let Some(text) = self.cross_ref_text(target, kind, page_of) else {
            return Ok(ChangeSet::default());
        };
        // Inside an existing reference the new one would overlap it: put it after.
        let offset = paragraph
            .cross_ref_at(caret.offset)
            .filter(|r| r.range.start < caret.offset)
            .map_or(caret.offset, |r| r.range.end);
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::InsertText {
                block: caret.block,
                offset,
                text: text.clone(),
                format: None,
            },
        )?;
        let mut cross_refs = self
            .doc
            .paragraph(caret.block)
            .map(|p| p.cross_refs().to_vec())
            .unwrap_or_default();
        cross_refs.push(CrossRef {
            range: offset..offset + text.len(),
            target: target.to_owned(),
            kind,
        });
        b.apply(
            &mut self.doc,
            Operation::SetCrossRefs {
                block: caret.block,
                cross_refs,
            },
        )?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.selection = Selection::caret(DocPosition {
            block: caret.block,
            offset: offset + text.len(),
        });
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Turns cross-reference `index` of `block` back into plain text (the text stays).
    pub fn remove_cross_ref(
        &mut self,
        block: BlockId,
        index: usize,
    ) -> Result<ChangeSet, EditorError> {
        let Some(paragraph) = self.doc.paragraph(block) else {
            return Ok(ChangeSet::default());
        };
        if index >= paragraph.cross_refs().len() {
            return Ok(ChangeSet::default());
        }
        let mut cross_refs = paragraph.cross_refs().to_vec();
        cross_refs.remove(index);
        let mut b = EditBuilder::default();
        b.apply(&mut self.doc, Operation::SetCrossRefs { block, cross_refs })?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        Ok(b.dirty)
    }

    /// Brings every cross-reference's text up to date (bookmark text and page numbers), as
    /// one undoable step. References whose bookmark is gone keep their last text. Returns an
    /// empty change set when nothing was stale.
    pub fn refresh_cross_refs(
        &mut self,
        page_of: &dyn Fn(BlockId) -> Option<u32>,
    ) -> Result<ChangeSet, EditorError> {
        // (paragraph, [(index, new text)]) for every stale reference.
        let mut stale: Vec<(BlockId, Vec<(usize, String)>)> = Vec::new();
        for paragraph in self.doc.paragraphs() {
            let mut changes = Vec::new();
            for (index, r) in paragraph.cross_refs().iter().enumerate() {
                let Some(wanted) = self.cross_ref_text(&r.target, r.kind, page_of) else {
                    continue;
                };
                if paragraph.text().get(r.range.clone()) != Some(wanted.as_str()) {
                    changes.push((index, wanted));
                }
            }
            if !changes.is_empty() {
                stale.push((paragraph.id, changes));
            }
        }
        if stale.is_empty() {
            return Ok(ChangeSet::default());
        }
        let mut b = EditBuilder::default();
        let caret = self.selection;
        for (block, changes) in stale {
            let Some(paragraph) = self.doc.paragraph(block) else {
                continue;
            };
            let old = paragraph.cross_refs().to_vec();
            // The new ranges, front to back, as the replaced texts shift what follows.
            let mut new_refs = old.clone();
            let mut shift = 0isize;
            for (index, r) in old.iter().enumerate() {
                let start = (r.range.start as isize + shift) as usize;
                let len = match changes.iter().find(|(i, _)| *i == index) {
                    Some((_, text)) => {
                        shift += text.len() as isize - r.range.len() as isize;
                        text.len()
                    }
                    None => r.range.len(),
                };
                new_refs[index].range = start..start + len;
            }
            // Deleting a reference's text drops the reference, and undoing the deletion only
            // brings the text back. Setting the list to itself first makes the undo's last
            // step put the old references back over the restored text.
            b.apply(
                &mut self.doc,
                Operation::SetCrossRefs {
                    block,
                    cross_refs: old.clone(),
                },
            )?;
            // Replace the texts back to front, so earlier offsets stay valid.
            for (index, text) in changes.iter().rev() {
                let range = old[*index].range.clone();
                // The new text keeps the formatting of the old text's first character.
                let format = paragraph_format_at(self.doc.paragraph(block), range.start);
                b.apply(
                    &mut self.doc,
                    Operation::DeleteRange {
                        block,
                        range: range.clone(),
                    },
                )?;
                if !text.is_empty() {
                    let runs = format
                        .map(|format| {
                            vec![Run {
                                range: 0..text.len(),
                                format,
                            }]
                        })
                        .unwrap_or_default();
                    b.apply(
                        &mut self.doc,
                        Operation::InsertText {
                            block,
                            offset: range.start,
                            text: text.clone(),
                            format: Some(runs),
                        },
                    )?;
                }
            }
            b.apply(
                &mut self.doc,
                Operation::SetCrossRefs {
                    block,
                    cross_refs: new_refs,
                },
            )?;
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        // The caret keeps its block; its offset is clamped to the new text.
        self.selection = Selection {
            anchor: self.clamp(caret.anchor),
            head: self.clamp(caret.head),
        };
        Ok(b.dirty)
    }

    // ------------------------------------------------------------ table styles and merging

    /// Name of the style of the table the caret is in.
    pub fn table_style_name(&self) -> Option<String> {
        let cell = self.caret_cell()?;
        self.doc
            .table(cell.table)?
            .style
            .as_ref()
            .map(|style| style.name.clone())
    }

    /// Applies the built-in table style `name` to the caret's table (`None` removes the
    /// style and leaves the borders as they are). Unknown names do nothing.
    pub fn set_table_style(&mut self, name: Option<&str>) -> Result<ChangeSet, EditorError> {
        let Some(cell) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(cell.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        let mut new = old.clone();
        match name {
            None => new.style = None,
            Some(name) => {
                let Some((style, borders)) = TableStyle::builtin(name) else {
                    return Ok(ChangeSet::default());
                };
                new.apply_style(Some(style), borders);
            }
        }
        if new == old {
            return Ok(ChangeSet::default());
        }
        self.replace_table_keeping_caret(old, new)
    }

    /// Whether the caret's cell can merge with the cell to its right.
    pub fn can_merge_cell_right(&self) -> bool {
        self.merged_right().is_some()
    }

    /// Whether the caret's cell can merge with the cell below it.
    pub fn can_merge_cell_below(&self) -> bool {
        self.merged_below().is_some()
    }

    /// Whether the caret's cell is a merged cell that can be split.
    pub fn can_split_cell(&self) -> bool {
        self.split().is_some()
    }

    /// Merges the caret's cell with the cell to its right: one cell across both, holding
    /// the text of both.
    pub fn merge_cell_right(&mut self) -> Result<ChangeSet, EditorError> {
        match self.merged_right() {
            Some((old, new)) => self.replace_table_keeping_caret(old, new),
            None => Ok(ChangeSet::default()),
        }
    }

    /// Merges the caret's cell with the cell below it: the cell below becomes a
    /// continuation of this one and its text moves up.
    pub fn merge_cell_below(&mut self) -> Result<ChangeSet, EditorError> {
        let Some((old, mut new, fresh)) = self.merged_below() else {
            return Ok(ChangeSet::default());
        };
        if let Some((row, cell)) = fresh {
            let id = self.doc.alloc_id();
            if let Some(slot) = new.rows.get_mut(row).and_then(|r| r.cells.get_mut(cell)) {
                slot.paragraphs = vec![Paragraph::new(id, "")];
            }
        }
        self.replace_table_keeping_caret(old, new)
    }

    /// Splits the caret's merged cell: a cell merged downwards is separated from the cells
    /// below it first; otherwise a cell across several columns becomes one cell per column.
    pub fn split_cell(&mut self) -> Result<ChangeSet, EditorError> {
        let Some((old, mut new, fresh)) = self.split() else {
            return Ok(ChangeSet::default());
        };
        // Fresh cells need real paragraph ids.
        for (row, cell) in fresh {
            let id = self.doc.alloc_id();
            if let Some(slot) = new.rows.get_mut(row).and_then(|r| r.cells.get_mut(cell)) {
                *slot = TableCell::new(Paragraph::new(id, ""));
            }
        }
        self.replace_table_keeping_caret(old, new)
    }

    /// Width of the grid column the caret's cell starts at, points.
    pub fn caret_column_width(&self) -> Option<f32> {
        let at = self.caret_cell()?;
        let table = self.doc.table(at.table)?;
        let column = table.column_of(at.row, at.cell)?;
        table.widths_pt.get(column).copied()
    }

    /// Sets the width of the grid column the caret's cell starts at, points. The table keeps
    /// its overall width: the other columns share the difference in proportion, and no column
    /// is left narrower than [`MIN_COLUMN_WIDTH_PT`]. A one-column table simply takes the new
    /// width.
    pub fn set_caret_column_width(&mut self, width_pt: f32) -> Result<ChangeSet, EditorError> {
        let Some(at) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(at.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        let Some(column) = old.column_of(at.row, at.cell) else {
            return Ok(ChangeSet::default());
        };
        let mut new = old.clone();
        if !resize_column(&mut new.widths_pt, column, width_pt) {
            return Ok(ChangeSet::default());
        }
        self.replace_table_keeping_caret(old, new)
    }

    /// Gives every column of the caret's table the same width, keeping the table's overall
    /// width.
    pub fn equalise_table_columns(&mut self) -> Result<ChangeSet, EditorError> {
        let Some(at) = self.caret_cell() else {
            return Ok(ChangeSet::default());
        };
        let Some(old) = self.doc.table(at.table).cloned() else {
            return Ok(ChangeSet::default());
        };
        let mut new = old.clone();
        if !equalise_widths(&mut new.widths_pt) {
            return Ok(ChangeSet::default());
        }
        self.replace_table_keeping_caret(old, new)
    }

    /// The caret's table before and after merging the caret's cell with its right
    /// neighbour. Cells that take part in a vertical merge are left alone, so merged areas
    /// stay rectangular.
    fn merged_right(&self) -> Option<(Table, Table)> {
        let at = self.caret_cell()?;
        let old = self.doc.table(at.table)?.clone();
        let row = old.rows.get(at.row)?;
        let left = row.cells.get(at.cell)?;
        let right = row.cells.get(at.cell + 1)?;
        let column = old.column_of(at.row, at.cell)?;
        let right_column = column + usize::from(left.col_span.max(1));
        if left.merged_into_above
            || right.merged_into_above
            || continues_below(&old, at.row, column)
            || continues_below(&old, at.row, right_column)
        {
            return None;
        }
        let span = usize::from(left.col_span.max(1)) + usize::from(right.col_span.max(1));
        if span > usize::from(u8::MAX) {
            return None;
        }
        let mut new = old.clone();
        let cells = &mut new.rows[at.row].cells;
        let removed = cells.remove(at.cell + 1);
        let target = &mut cells[at.cell];
        target.col_span = span as u8;
        absorb_paragraphs(target, removed.paragraphs);
        Some((old, new))
    }

    /// The caret's table before and after merging the caret's cell with the cell below it
    /// (below any cells it already covers). The cell below must cover the same columns.
    /// The last value is the (row, cell) of the covered cell when its stand-in paragraph is
    /// a placeholder that needs a fresh id.
    #[allow(clippy::type_complexity)]
    fn merged_below(&self) -> Option<(Table, Table, Option<(usize, usize)>)> {
        let at = self.caret_cell()?;
        let old = self.doc.table(at.table)?.clone();
        let origin = old.cell(at.row, at.cell)?;
        if origin.merged_into_above {
            return None;
        }
        let column = old.column_of(at.row, at.cell)?;
        let span = origin.col_span.max(1);
        // The first row below that this cell does not already cover.
        let mut target_row = at.row + 1;
        while continuation_at(&old, target_row, column).is_some() {
            target_row += 1;
        }
        let (target_cell, below) = old.cell_at_column(target_row, column)?;
        if old.column_of(target_row, target_cell)? != column
            || below.col_span.max(1) != span
            || below.merged_into_above
        {
            return None;
        }
        let mut new = old.clone();
        let below = &mut new.rows[target_row].cells[target_cell];
        below.merged_into_above = true;
        // The covered cell keeps one empty paragraph (a cell is never empty); everything
        // with content moves into the cell above under its own id.
        let has_content = |p: &Paragraph| !p.is_empty() || !p.images().is_empty();
        let (moved, mut empty): (Vec<Paragraph>, Vec<Paragraph>) =
            std::mem::take(&mut below.paragraphs)
                .into_iter()
                .partition(has_content);
        let mut fresh = None;
        match empty.pop() {
            Some(stand_in) => below.paragraphs = vec![stand_in],
            None => {
                below.paragraphs = vec![Paragraph::new(BlockId(0), "")];
                fresh = Some((target_row, target_cell));
            }
        }
        absorb_paragraphs(&mut new.rows[at.row].cells[at.cell], moved);
        Some((old, new, fresh))
    }

    /// The caret's table before and after splitting the caret's cell, and the (row, cell)
    /// slots of `new` that hold placeholder cells needing fresh paragraph ids.
    #[allow(clippy::type_complexity)]
    fn split(&self) -> Option<(Table, Table, Vec<(usize, usize)>)> {
        let at = self.caret_cell()?;
        let old = self.doc.table(at.table)?.clone();
        let cell = old.cell(at.row, at.cell)?;
        let column = old.column_of(at.row, at.cell)?;
        let mut new = old.clone();
        // A vertical merge comes apart first: the covered cells become cells again.
        let mut row = at.row + 1;
        let mut separated = false;
        while let Some(index) = continuation_at(&new, row, column) {
            new.rows[row].cells[index].merged_into_above = false;
            separated = true;
            row += 1;
        }
        if separated {
            return Some((old, new, Vec::new()));
        }
        let span = usize::from(cell.col_span.max(1));
        if span <= 1 {
            return None;
        }
        let mut fresh = Vec::new();
        let cells = &mut new.rows[at.row].cells;
        cells[at.cell].col_span = 1;
        for extra in 1..span {
            let slot = at.cell + extra;
            // A placeholder: `split_cell` gives it a paragraph with a real id.
            cells.insert(slot, TableCell::new(Paragraph::new(BlockId(0), "")));
            fresh.push((at.row, slot));
        }
        Some((old, new, fresh))
    }

    /// Replaces the caret's table and keeps the caret in its paragraph when that still
    /// exists (otherwise the clamp moves it to a valid place).
    fn replace_table_keeping_caret(
        &mut self,
        old: Table,
        new: Table,
    ) -> Result<ChangeSet, EditorError> {
        let caret = self.selection.head;
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::ReplaceTable { table: old.id, new },
        )?;
        b.record(&mut self.history, EditKind::Other, Instant::now());
        let head = self.clamp(caret);
        self.selection = Selection::caret(head);
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// True when `block` is a paragraph of a cell covered by a vertical merge: it is not
    /// laid out, so the caret must never rest in it.
    pub(crate) fn is_covered(&self, block: BlockId) -> bool {
        let Some(at) = self.doc.location_of(block).and_then(|l| l.table) else {
            return false;
        };
        at.row > 0
            && self
                .doc
                .table(at.table)
                .and_then(|t| t.cell(at.row, at.cell))
                .is_some_and(|c| c.merged_into_above)
    }
}

/// Direct formatting of the character at `offset`, if it has any.
fn paragraph_format_at(
    paragraph: Option<&Paragraph>,
    offset: usize,
) -> Option<libertum_model::CharFormat> {
    let format = paragraph?.format_of(offset);
    (!format.is_empty()).then_some(format)
}

/// Index of the cell of `row` that starts at grid column `column` and continues the cell
/// above it.
fn continuation_at(table: &Table, row: usize, column: usize) -> Option<usize> {
    let (index, cell) = table.cell_at_column(row, column)?;
    (row > 0 && cell.merged_into_above && table.column_of(row, index)? == column).then_some(index)
}

/// True when the cell starting at grid column `column` of `row` is continued by the row
/// below.
fn continues_below(table: &Table, row: usize, column: usize) -> bool {
    continuation_at(table, row + 1, column).is_some()
}

/// Moves `paragraphs` into `cell`, dropping empty ones; an empty `cell` is replaced by them.
fn absorb_paragraphs(cell: &mut TableCell, paragraphs: Vec<Paragraph>) {
    let has_content = |p: &Paragraph| !p.is_empty() || !p.images().is_empty();
    let incoming: Vec<Paragraph> = paragraphs.into_iter().filter(has_content).collect();
    if incoming.is_empty() {
        return;
    }
    if cell.paragraphs.iter().any(has_content) {
        cell.paragraphs.extend(incoming);
    } else {
        cell.paragraphs = incoming;
    }
}

/// Smallest width a table column can be set to, points (a quarter inch).
pub const MIN_COLUMN_WIDTH_PT: f32 = 18.0;
/// Largest width a table column can be set to, points (well beyond any page).
pub const MAX_COLUMN_WIDTH_PT: f32 = 4_000.0;

/// Resizes `column` of `widths` and shares the difference between the other columns in
/// proportion, so the table keeps its total width. Returns whether anything changed.
fn resize_column(widths: &mut [f32], column: usize, width_pt: f32) -> bool {
    if widths.is_empty() || column >= widths.len() || !width_pt.is_finite() {
        return false;
    }
    let total: f32 = widths.iter().sum();
    if widths.len() == 1 {
        let width = width_pt.clamp(MIN_COLUMN_WIDTH_PT, MAX_COLUMN_WIDTH_PT);
        let changed = (width - widths[0]).abs() > 0.01;
        widths[0] = width;
        return changed;
    }
    let others = total - widths[column];
    let least = MIN_COLUMN_WIDTH_PT * (widths.len() - 1) as f32;
    let width = width_pt.clamp(
        MIN_COLUMN_WIDTH_PT,
        (total - least).max(MIN_COLUMN_WIDTH_PT),
    );
    let rest = (total - width).max(least);
    // Other columns keep their proportions to each other.
    let factor = if others > 0.0 { rest / others } else { 1.0 };
    let before: Vec<f32> = widths.to_vec();
    for (index, value) in widths.iter_mut().enumerate() {
        if index == column {
            *value = width;
        } else {
            *value = (*value * factor).max(MIN_COLUMN_WIDTH_PT);
        }
    }
    before
        .iter()
        .zip(widths.iter())
        .any(|(old, new)| (old - new).abs() > 0.01)
}

/// Gives every column the same width, keeping the total. Returns whether anything changed.
fn equalise_widths(widths: &mut [f32]) -> bool {
    if widths.is_empty() {
        return false;
    }
    let each = widths.iter().sum::<f32>() / widths.len() as f32;
    let changed = widths.iter().any(|width| (width - each).abs() > 0.01);
    widths.fill(each);
    changed
}

// ---------------------------------------------------------------- date and time fields

/// Writes `seconds` (unix time, UTC) as a local date, time or both in `format`.
///
/// The clock and the time zone belong to the machine, not the document, so this lives here
/// rather than in `libertum-model`; a timestamp out of range falls back to "now" instead of
/// panicking.
pub fn format_date_time(seconds: i64, format: DateTimeFormat) -> String {
    use chrono::TimeZone as _;
    let at = chrono::Local
        .timestamp_opt(seconds, 0)
        .single()
        .unwrap_or_else(chrono::Local::now);
    let pattern = match format {
        DateTimeFormat::DateLong => "%-d %B %Y",
        DateTimeFormat::DateMedium => "%-d %b %Y",
        DateTimeFormat::DateShort => "%d/%m/%Y",
        DateTimeFormat::DateIso => "%Y-%m-%d",
        DateTimeFormat::DateTimeLong => "%-d %B %Y at %H:%M",
        DateTimeFormat::DateTimeMedium => "%-d %b %Y, %H:%M",
        DateTimeFormat::DateTimeIso => "%Y-%m-%d %H:%M",
        DateTimeFormat::Time24 => "%H:%M",
        DateTimeFormat::Time24Seconds => "%H:%M:%S",
        DateTimeFormat::Time12 => "%-I:%M %P",
        DateTimeFormat::Time12Seconds => "%-I:%M:%S %P",
    };
    at.format(pattern).to_string()
}

impl Editor {
    /// Inserts a date and time at the caret, as one undo step.
    ///
    /// An [`Updated`](DateTimeSource::Updated) field shows the document's last save (or
    /// `now_secs` when it has never been saved) and is rewritten on every save; an
    /// [`Inserted`](DateTimeSource::Inserted) field keeps the moment it was inserted.
    pub fn insert_date_time(
        &mut self,
        now_secs: i64,
        source: DateTimeSource,
        format: DateTimeFormat,
    ) -> Result<ChangeSet, EditorError> {
        let stamp = match source {
            DateTimeSource::Inserted => now_secs,
            DateTimeSource::Updated => self.doc.saved_at().unwrap_or(now_secs),
        };
        let text = format_date_time(stamp, format);
        let caret = self.clamp(self.selection.head);
        let Some(paragraph) = self.doc.paragraph(caret.block) else {
            return Ok(ChangeSet::default());
        };
        if paragraph.props.toc_target.is_some() {
            return Err(EditorError::ReadOnly);
        }
        // Inside an existing date field the new one would overlap it: put it after.
        let offset = paragraph
            .date_field_at(caret.offset)
            .filter(|f| f.range.start < caret.offset)
            .map_or(caret.offset, |f| f.range.end);
        let mut b = EditBuilder::default();
        b.apply(
            &mut self.doc,
            Operation::InsertText {
                block: caret.block,
                offset,
                text: text.clone(),
                format: None,
            },
        )?;
        let mut fields = self
            .doc
            .paragraph(caret.block)
            .map(|p| p.date_fields().to_vec())
            .unwrap_or_default();
        fields.push(DateTimeField::new(
            offset..offset + text.len(),
            source,
            format,
        ));
        b.apply(
            &mut self.doc,
            Operation::SetDateTimeFields {
                block: caret.block,
                date_fields: fields,
            },
        )?;
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.selection = Selection::caret(DocPosition {
            block: caret.block,
            offset: offset + text.len(),
        });
        self.preferred_x = None;
        Ok(b.dirty)
    }

    /// Rewrites every "updated date and time" field to the document's last save (or
    /// `now_secs` when it has none), as one undoable step. Called before a save. Returns an
    /// empty change set when nothing was stale.
    pub fn refresh_date_fields(&mut self, now_secs: i64) -> Result<ChangeSet, EditorError> {
        let stamp = self.doc.saved_at().unwrap_or(now_secs);
        // (paragraph, [(index, new text)]) for every stale field.
        let mut stale: Vec<(BlockId, Vec<(usize, String)>)> = Vec::new();
        for paragraph in self.doc.paragraphs() {
            let mut changes = Vec::new();
            for (index, field) in paragraph.date_fields().iter().enumerate() {
                if !field.follows_save() {
                    continue;
                }
                let wanted = format_date_time(stamp, field.format);
                if paragraph.text().get(field.range.clone()) != Some(wanted.as_str()) {
                    changes.push((index, wanted));
                }
            }
            if !changes.is_empty() {
                stale.push((paragraph.id, changes));
            }
        }
        if stale.is_empty() {
            return Ok(ChangeSet::default());
        }
        let mut b = EditBuilder::default();
        let caret = self.selection;
        for (block, changes) in stale {
            let Some(paragraph) = self.doc.paragraph(block) else {
                continue;
            };
            let old = paragraph.date_fields().to_vec();
            // The new ranges, front to back, as the replaced texts shift what follows.
            let mut new_fields = old.clone();
            let mut shift = 0isize;
            for (index, field) in old.iter().enumerate() {
                let start = (field.range.start as isize + shift) as usize;
                let len = match changes.iter().find(|(i, _)| *i == index) {
                    Some((_, text)) => {
                        shift += text.len() as isize - field.range.len() as isize;
                        text.len()
                    }
                    None => field.range.len(),
                };
                new_fields[index].range = start..start + len;
            }
            // Setting the list to itself first makes the undo's last step restore the old
            // fields over the restored text (the same trick cross-references use).
            b.apply(
                &mut self.doc,
                Operation::SetDateTimeFields {
                    block,
                    date_fields: old.clone(),
                },
            )?;
            for (index, text) in changes.iter().rev() {
                let range = old[*index].range.clone();
                let format = paragraph_format_at(self.doc.paragraph(block), range.start);
                b.apply(
                    &mut self.doc,
                    Operation::DeleteRange {
                        block,
                        range: range.clone(),
                    },
                )?;
                if !text.is_empty() {
                    let runs = format
                        .map(|format| {
                            vec![Run {
                                range: 0..text.len(),
                                format,
                            }]
                        })
                        .unwrap_or_default();
                    b.apply(
                        &mut self.doc,
                        Operation::InsertText {
                            block,
                            offset: range.start,
                            text: text.clone(),
                            format: Some(runs),
                        },
                    )?;
                }
            }
            b.apply(
                &mut self.doc,
                Operation::SetDateTimeFields {
                    block,
                    date_fields: new_fields,
                },
            )?;
        }
        self.commit(&mut b, EditKind::Other, Instant::now())?;
        self.selection = Selection {
            anchor: self.clamp(caret.anchor),
            head: self.clamp(caret.head),
        };
        Ok(b.dirty)
    }
}
