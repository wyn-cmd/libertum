//! `libertum-model` — document tree, operations, transactions and undo/redo (PLAN §4.1).
//!
//! No I/O, no UI, no layout. Paragraphs own their own text; every mutation goes through an
//! [`Operation`] whose inverse restores the previous state exactly.

mod bookmark;
mod document;
mod format;
mod header_footer;
mod history;
mod lists;
mod media;
mod note;
mod ops;
mod page;
mod paragraph;
mod table;

pub use bookmark::Bookmark;
pub use document::{
    BlockList, ChangeSet, DEFAULT_COLUMN_GUTTER_PT, DEFAULT_HEADER_FOOTER_DISTANCE_PT, Document,
    MAX_COLUMNS, ModelError, PageNumberHorizontal, PageNumberPos, PageNumberVertical, ParaLocation,
    Section, TableCellLocation,
};
pub use format::{
    BOLD, BorderSides, Borders, CharFormat, Ligatures, LineSpacing, MAX_TAB_STOPS, ParaFormat,
    ParagraphStyle, REGULAR, ResolvedChar, ResolvedPara, StyleId, StyleSheet, TabStop, TabStopKind,
    TabStops, VerticalAlign,
};
pub use header_footer::{HeaderFooter, HeadersFooters, HfKind, HfVariant};
pub use history::{EditKind, History, HistoryEntry, TYPING_PAUSE};
pub use lists::{
    LIST_HANGING_PT, LIST_INDENT_PT, ListItem, ListStyle, MAX_LIST_LEVEL, number_items,
};
pub use media::{
    CrossRef, CrossRefKind, ImageFormat, InlineImage, MAX_IMAGE_PT, MIN_IMAGE_PT, Media,
    MediaError, test_png_header,
};
pub use note::{Note, NoteKind};
pub use ops::Operation;
pub use page::{Margins, Orientation, PageSetup, PaperSize};
pub use paragraph::{
    Align, Block, BlockId, ParaProps, Paragraph, Run, ceil_char_boundary, floor_char_boundary,
};
pub use table::{
    DEFAULT_CELL_MARGIN_PT, MIN_TABLE_COLUMN_PT, Table, TableBorders, TableCell, TableRow,
    TableStyle,
};

#[cfg(test)]
mod tests;
