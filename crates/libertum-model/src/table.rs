//! Tables (PLAN §4.1, §4.4).
//!
//! A table is a top-level [`Block`](crate::paragraph::Block) with a grid of columns and rows;
//! each cell holds one or more paragraphs. Cells do not contain tables (Word allows nesting,
//! which v1 does not model: the importers refuse a nested table rather than flatten it).
//!
//! Column widths are in points and independent of the page. Layout fits the table to the
//! column it lands in: a wider table is scaled down proportionally, a narrower one keeps its
//! widths. Rows are the pagination unit; a row never splits across pages in v1.

use crate::paragraph::{BlockId, Paragraph};

/// Narrowest column layout keeps, points (0.25 in). A narrower column is widened to this, as
/// Word does when a table is dragged below the minimum.
pub const MIN_TABLE_COLUMN_PT: f32 = 18.0;

/// Default distance between cell text and the cell border, points (0.075 in, Word's 108 twips).
pub const DEFAULT_CELL_MARGIN_PT: f32 = 5.4;

/// Table borders. One width and colour for the outer and inner lines.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TableBorders {
    pub width_pt: f32,
    pub colour: [u8; 4],
    pub outer: bool,
    pub inner: bool,
}

impl Default for TableBorders {
    fn default() -> Self {
        Self {
            width_pt: 0.75,
            colour: [0, 0, 0, 255],
            outer: true,
            inner: true,
        }
    }
}

impl TableBorders {
    /// No borders at all (a "borderless" table, as Word's Table Grid-less style).
    pub fn none() -> Self {
        Self {
            width_pt: 0.0,
            ..Self::default()
        }
    }

    pub fn is_visible(&self) -> bool {
        self.width_pt > 0.0 && (self.outer || self.inner)
    }
}

/// A named table style: the look a table takes beyond its borders (DOCX `w:tblStyle`).
///
/// The style travels with the table instead of living in the style sheet, so a table is
/// self-describing and replacing it is one undoable operation. Fills are resolved per row
/// when the table is laid out ([`Table::resolved_fill`]), so banding follows rows as they
/// are inserted and removed; a cell's own `fill` always wins.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableStyle {
    /// Name shown in the Table menu and written to the file (DOCX style name).
    pub name: String,
    /// Fill of the header rows (the table's `header_rows`).
    pub header_fill: Option<[u8; 4]>,
    /// Fill of every second body row (the 2nd, 4th, ... row after the header rows).
    pub band_fill: Option<[u8; 4]>,
}

impl TableStyle {
    /// Name of the built-in style with grid lines and no fills.
    pub const GRID: &'static str = "Grid";

    /// The built-in styles, in menu order, each with the borders it sets on the table.
    pub fn builtins() -> Vec<(TableStyle, TableBorders)> {
        const HEADER: [u8; 4] = [217, 217, 217, 255];
        const BAND: [u8; 4] = [242, 242, 242, 255];
        let style = |name: &str, header_fill, band_fill| TableStyle {
            name: name.to_owned(),
            header_fill,
            band_fill,
        };
        let outline = TableBorders {
            inner: false,
            ..TableBorders::default()
        };
        vec![
            (style("Plain", None, None), TableBorders::none()),
            (style(Self::GRID, None, None), TableBorders::default()),
            (
                style("Header Row", Some(HEADER), None),
                TableBorders::default(),
            ),
            (
                style("Banded Rows", Some(HEADER), Some(BAND)),
                TableBorders::default(),
            ),
            (style("Banded, No Grid", Some(HEADER), Some(BAND)), outline),
        ]
    }

    /// The built-in style called `name` (case-insensitive) and its borders.
    pub fn builtin(name: &str) -> Option<(TableStyle, TableBorders)> {
        Self::builtins()
            .into_iter()
            .find(|(style, _)| style.name.eq_ignore_ascii_case(name))
    }
}

/// A table block.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Table {
    pub id: BlockId,
    /// One width per grid column, points.
    pub widths_pt: Vec<f32>,
    pub rows: Vec<TableRow>,
    /// Number of leading rows repeated at the top of each page the table continues on.
    pub header_rows: u8,
    pub borders: TableBorders,
    /// Named style (header and banded fills); `None` is an unstyled table.
    pub style: Option<TableStyle>,
}

/// One row of cells.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableRow {
    /// Left to right. A cell with `col_span > 1` covers several grid columns.
    pub cells: Vec<TableCell>,
    /// Minimum height, points (DOCX `w:trHeight`); the row grows to fit its text.
    pub min_height_pt: f32,
    /// Never split across pages. Table rows are never split in v1, so this is always true
    /// for rows laid out by Libertum; it is kept so ODT's `keep-together` can round-trip.
    pub keep_together: bool,
}

/// One cell.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TableCell {
    /// Grid columns covered (DOCX `w:gridSpan`), at least 1.
    pub col_span: u8,
    /// A vertical merge continuation: this cell has no content of its own and is covered by
    /// the cell above (DOCX `w:vMerge` without `w:val="restart"`).
    pub merged_into_above: bool,
    /// Cell text. Always at least one paragraph.
    pub paragraphs: Vec<Paragraph>,
    /// Background fill (DOCX `w:shd`); `None` is transparent.
    pub fill: Option<[u8; 4]>,
}

impl TableCell {
    pub fn new(paragraph: Paragraph) -> Self {
        Self {
            col_span: 1,
            merged_into_above: false,
            paragraphs: vec![paragraph],
            fill: None,
        }
    }

    /// The cell's text, paragraphs joined with newlines (document order / copy).
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (i, p) in self.paragraphs.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(p.text());
        }
        out
    }

    pub fn word_count(&self) -> usize {
        self.paragraphs.iter().map(Paragraph::word_count).sum()
    }

    pub fn content_eq(&self, other: &Self) -> bool {
        self.col_span == other.col_span
            && self.merged_into_above == other.merged_into_above
            && self.fill == other.fill
            && self.paragraphs.len() == other.paragraphs.len()
            && self
                .paragraphs
                .iter()
                .zip(other.paragraphs.iter())
                .all(|(a, b)| a.content_eq(b))
    }
}

impl TableRow {
    pub fn word_count(&self) -> usize {
        self.cells.iter().map(TableCell::word_count).sum()
    }

    pub fn content_eq(&self, other: &Self) -> bool {
        self.min_height_pt == other.min_height_pt
            && self.keep_together == other.keep_together
            && self.cells.len() == other.cells.len()
            && self
                .cells
                .iter()
                .zip(other.cells.iter())
                .all(|(a, b)| a.content_eq(b))
    }

    /// Grid columns the row covers (the sum of its cells' spans).
    pub fn span(&self) -> usize {
        self.cells
            .iter()
            .map(|c| usize::from(c.col_span.max(1)))
            .sum()
    }
}

impl Table {
    pub fn columns(&self) -> usize {
        self.widths_pt.len()
    }

    pub fn width_pt(&self) -> f32 {
        self.widths_pt.iter().sum()
    }

    pub fn cell(&self, row: usize, col: usize) -> Option<&TableCell> {
        self.rows.get(row)?.cells.get(col)
    }

    pub fn cell_mut(&mut self, row: usize, col: usize) -> Option<&mut TableCell> {
        self.rows.get_mut(row)?.cells.get_mut(col)
    }

    /// Grid column a cell starts at (index of the cell, not the span).
    pub fn column_of(&self, row: usize, cell: usize) -> Option<usize> {
        let row = self.rows.get(row)?;
        if cell >= row.cells.len() {
            return None;
        }
        Some(
            row.cells
                .iter()
                .take(cell)
                .map(|c| usize::from(c.col_span.max(1)))
                .sum(),
        )
    }

    /// The cell covering grid column `col` in `row`, with its starting column.
    pub fn cell_at_column(&self, row: usize, col: usize) -> Option<(usize, &TableCell)> {
        let row = self.rows.get(row)?;
        let mut at = 0usize;
        for (index, cell) in row.cells.iter().enumerate() {
            let span = usize::from(cell.col_span.max(1));
            if col < at + span {
                return Some((index, cell));
            }
            at += span;
        }
        None
    }

    /// X offset of a cell's left edge from the table's left edge, points.
    pub fn cell_x_pt(&self, row: usize, cell: usize) -> Option<f32> {
        let start = self.column_of(row, cell)?;
        Some(self.widths_pt.iter().take(start).sum())
    }

    /// Width of a cell, points (its span's columns).
    pub fn cell_width_pt(&self, row: usize, cell: usize) -> Option<f32> {
        let start = self.column_of(row, cell)?;
        let span = usize::from(self.rows.get(row)?.cells.get(cell)?.col_span.max(1));
        Some(self.widths_pt.iter().skip(start).take(span).sum())
    }

    pub fn word_count(&self) -> usize {
        self.rows.iter().map(TableRow::word_count).sum()
    }

    /// The fill the table style gives row `row` (header or band), ignoring cell fills.
    pub fn style_fill(&self, row: usize) -> Option<[u8; 4]> {
        let style = self.style.as_ref()?;
        let header = usize::from(self.header_rows);
        if row < header {
            style.header_fill
        } else if (row - header) % 2 == 1 {
            style.band_fill
        } else {
            None
        }
    }

    /// The fill drawn for a cell of row `row`: its own, else the table style's.
    pub fn resolved_fill(&self, row: usize, cell: &TableCell) -> Option<[u8; 4]> {
        cell.fill.or_else(|| self.style_fill(row))
    }

    /// Applies a style: sets it and its borders, and marks the first row as a header row
    /// when the style fills header rows and the table has none.
    pub fn apply_style(&mut self, style: Option<TableStyle>, borders: TableBorders) {
        if style.as_ref().is_some_and(|s| s.header_fill.is_some())
            && self.header_rows == 0
            && !self.rows.is_empty()
        {
            self.header_rows = 1;
        }
        self.style = style;
        self.borders = borders;
    }

    pub fn content_eq(&self, other: &Self) -> bool {
        self.widths_pt == other.widths_pt
            && self.style == other.style
            && self.header_rows == other.header_rows
            && self.borders == other.borders
            && self.rows.len() == other.rows.len()
            && self
                .rows
                .iter()
                .zip(other.rows.iter())
                .all(|(a, b)| a.content_eq(b))
    }

    /// Clamps widths, spans and the header-row count to what the table can hold.
    pub fn normalise(&mut self) {
        let columns = self.widths_pt.len().max(1);
        if self.widths_pt.is_empty() {
            self.widths_pt.push(72.0);
        }
        for width in &mut self.widths_pt {
            if !width.is_finite() || *width < MIN_TABLE_COLUMN_PT {
                *width = MIN_TABLE_COLUMN_PT;
            }
        }
        for row in &mut self.rows {
            if row.cells.is_empty() {
                continue;
            }
            let mut remaining = columns;
            for cell in &mut row.cells {
                let span = usize::from(cell.col_span.max(1));
                let span = span.min(remaining.max(1));
                cell.col_span = span as u8;
                remaining = remaining.saturating_sub(span);
            }
        }
        self.header_rows = self.header_rows.min(self.rows.len() as u8);
    }
}
