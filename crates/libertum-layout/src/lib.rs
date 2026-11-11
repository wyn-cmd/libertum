//! Paragraph layout, pagination and layout caches for Libertum.
//!
//! This crate isolates parley behind Libertum's own types (PLAN §10, "parley API churn").
//! Layout consumes [`ParagraphSnapshot`]s converted from `libertum-model` paragraphs; the
//! model never depends on parley.
//!
//! Units: everything in this crate is in typographic points (1/72 in) at scale 1.0.
//! Rendering multiplies by a scale factor.

mod engine;
mod geometry;
mod paginate;
pub mod spike;

pub use engine::{
    Brush, EngineStats, FontReport, LayoutOptions, LineBox, ParaMetrics, ParagraphLayout,
    PlacedImage, TOC_NUMBER_RESERVE_PT,
};
pub use geometry::{Margins, PageGeometry};
pub use paginate::{
    CaretRect, CellDraw, CellSnapshot, Decoration, FlowPos, HitResult, INK_OVERHANG,
    LEADER_PITCH_PT, LayoutState, PageItem, PageLayout, PageNote, PaginationStats, ParaSlice,
    RowSnapshot, SelectionRect, TableDraw, TableSnapshot, column_layout, table_snapshots,
};
pub use spike::{ImageSnapshot, ParagraphSnapshot, SpanStyle, SpikeParagraph};

/// Re-export so downstream crates (render) use exactly the parley we use.
pub use parley;

#[cfg(test)]
mod tests;
