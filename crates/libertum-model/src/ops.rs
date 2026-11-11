//! Document operations. Every operation has an inverse, so a transaction can be undone
//! exactly (PLAN §4.1).

use std::ops::Range;

use crate::document::{ModelError, Section};
use crate::format::{ParagraphStyle, ResolvedChar, ResolvedPara, StyleId};
use crate::paragraph::{Block, BlockId, ParaProps, Run};
use crate::table::Table;
/// A document mutation. All text offsets are byte offsets on character boundaries.
#[derive(Clone, Debug, PartialEq)]
pub enum Operation {
    /// Inserts `text`. With `format: None` it takes the formatting of the character before
    /// it (typing); with `Some(runs)` it gets exactly those runs (ranges relative to `text`),
    /// which is how undoing a deletion restores the deleted text's formatting.
    InsertText {
        block: BlockId,
        offset: usize,
        text: String,
        format: Option<Vec<Run>>,
    },
    DeleteRange {
        block: BlockId,
        range: Range<usize>,
    },
    /// Splits `block` at `offset`; the new paragraph gets `new_id` and `props` (or
    /// [`ParaProps::for_split_tail`] when `None`). Undoing a merge passes the merged-away
    /// paragraph's own properties, so a heading or page break survives merge + undo.
    SplitBlock {
        block: BlockId,
        offset: usize,
        new_id: BlockId,
        props: Option<ParaProps>,
    },
    /// Merges `first` with the block after it.
    MergeBlocks {
        first: BlockId,
    },
    InsertBlock {
        index: usize,
        block: Block,
    },
    RemoveBlock {
        block: BlockId,
    },
    /// Inserts a whole table as a top-level block.
    InsertTable {
        index: usize,
        table: Table,
    },
    /// Removes a table; the inverse carries the removed table.
    RemoveTable {
        table: BlockId,
    },
    /// Replaces a table's structure (rows, columns, widths, borders) in one undoable step.
    ReplaceTable {
        table: BlockId,
        new: Table,
    },
    /// Replaces a paragraph's runs (direct character formatting).
    SetRuns {
        block: BlockId,
        runs: Vec<Run>,
    },
    /// Replaces a paragraph's properties.
    SetParaProps {
        block: BlockId,
        props: ParaProps,
    },
    /// Replaces the footnote text attached to a paragraph (`None` removes it).
    SetFootnote {
        block: BlockId,
        note: Option<String>,
    },
    /// Replaces every note anchored in a paragraph (multi-note edits and undo).
    SetNotes {
        block: BlockId,
        notes: Vec<crate::Note>,
    },
    /// Replaces every bookmark anchored in a paragraph (rename, delete and undo).
    SetBookmarks {
        block: BlockId,
        bookmarks: Vec<crate::Bookmark>,
    },
    /// Replaces every inline image anchored in a paragraph (insert, resize, delete, undo).
    SetImages {
        block: BlockId,
        images: Vec<crate::InlineImage>,
    },
    /// Replaces every cross-reference in a paragraph (insert, remove, refresh, undo).
    SetCrossRefs {
        block: BlockId,
        cross_refs: Vec<crate::CrossRef>,
    },
    /// Replaces section `index` (page numbering and the like).
    SetSection {
        index: usize,
        section: Section,
    },
    /// Replaces the document's default paragraph formatting (document-wide settings such as
    /// hyphenation).
    SetParaDefaults {
        para: ResolvedPara,
    },
    /// Replaces the document's default character formatting (document-wide settings such as
    /// ligatures).
    SetCharDefaults {
        char: ResolvedChar,
    },
    /// Replaces paragraph style `id` in the style sheet ("Update style to match").
    SetStyle {
        id: StyleId,
        style: Box<ParagraphStyle>,
    },
}

impl Operation {
    /// The primary block this operation concerns.
    pub fn block(&self) -> BlockId {
        match self {
            Operation::InsertText { block, .. }
            | Operation::DeleteRange { block, .. }
            | Operation::SplitBlock { block, .. }
            | Operation::MergeBlocks { first: block }
            | Operation::RemoveBlock { block }
            | Operation::SetParaProps { block, .. }
            | Operation::SetRuns { block, .. }
            | Operation::SetFootnote { block, .. }
            | Operation::SetNotes { block, .. }
            | Operation::SetBookmarks { block, .. }
            | Operation::SetImages { block, .. }
            | Operation::SetCrossRefs { block, .. } => *block,
            Operation::InsertBlock { block, .. } => block.id(),
            Operation::InsertTable { table, .. } => table.id,
            Operation::RemoveTable { table } | Operation::ReplaceTable { table, .. } => *table,
            Operation::SetSection { .. }
            | Operation::SetStyle { .. }
            | Operation::SetParaDefaults { .. }
            | Operation::SetCharDefaults { .. } => BlockId(0),
        }
    }

    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            Operation::SplitBlock { .. }
                | Operation::MergeBlocks { .. }
                | Operation::InsertBlock { .. }
                | Operation::RemoveBlock { .. }
                | Operation::InsertTable { .. }
                | Operation::RemoveTable { .. }
                | Operation::ReplaceTable { .. }
        )
    }

    pub fn validate(&self, _doc: &crate::Document) -> Result<(), ModelError> {
        Ok(())
    }
}
