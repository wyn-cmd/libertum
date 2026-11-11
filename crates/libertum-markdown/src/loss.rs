//! The "what will be lost" report: formatting a document holds that Markdown cannot express,
//! so saving as Markdown would drop it.
//!
//! The report is checked against what `write.rs` really emits. It counts only things that are
//! *set* (direct formatting, a paragraph style, a non-default section setting), never inherited
//! defaults, and what the Markdown importer itself sets (the 6 pt gap after a paragraph, a
//! quote's indent, a table column's alignment) is not counted, so a document opened from
//! Markdown and not edited reports nothing.
//!
//! Not reported, because it is not formatting Markdown has a place for either way: space
//! before and after paragraphs, the keep-together and widow settings, hyphenation, section
//! names, and the distance of headers from the page edge.

use std::collections::BTreeMap;
use std::fmt;

use libertum_model::{
    Align, Block, CharFormat, Document, HfKind, InlineImage, Ligatures, ListStyle, PageSetup,
    Paragraph, StyleId, Table, TableBorders, VerticalAlign,
};

use crate::parse::IMAGE_READING_WIDTH_PT;

/// One kind of formatting a Markdown save drops. The variants are stable; match on them to
/// pick wording or an icon, or use the [`Display`](fmt::Display) text as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LossKind {
    /// Font families other than a monospace one (which becomes a code span). Counted per run.
    FontFamily,
    /// Text sizes. Per run.
    FontSize,
    /// Weights other than regular and bold (600 and over is written as bold). Per run.
    FontWeight,
    /// Text colour. Per run.
    TextColour,
    /// Highlighting. Per run.
    Highlight,
    /// Underlining. Per run.
    Underline,
    /// Superscript and subscript. Per run.
    SuperSubscript,
    /// Letter spacing. Per run.
    LetterSpacing,
    /// Ligature settings. Per run.
    Ligatures,
    /// Bold, italic or strikethrough on text inside a code block, a raw HTML block or a
    /// rule, which Markdown writes as it is. Per run.
    Emphasis,
    /// Alignment other than start (Markdown keeps only table column alignment). Per
    /// paragraph.
    Alignment,
    /// Left, right or first-line indents (a quote's indent is written as `>`). Per paragraph.
    Indent,
    /// Line spacing. Per paragraph.
    LineSpacing,
    /// Paragraph borders. Per paragraph.
    ParagraphBorders,
    /// Tab stops. Per paragraph.
    TabStops,
    /// Paragraph styles Markdown has no name for (anything but body, headings and quotes).
    /// Per paragraph.
    ParagraphStyle,
    /// Lettered and outline list numbering, written as 1. 2. 3. Per list paragraph.
    ListStyle,
    /// A page break before a paragraph.
    PageBreak,
    /// A section break.
    SectionBreak,
    /// Paper size, orientation or margins that differ from the default. Per section.
    PageSetup,
    /// More than one text column. Per section.
    Columns,
    /// Headers and footers. Per header or footer text.
    HeadersFooters,
    /// Page numbers shown in the margin, restarted or hidden on the first page. Per section.
    PageNumbers,
    /// Footnotes and endnotes. Per note.
    Footnotes,
    /// Bookmarks. Per bookmark.
    Bookmarks,
    /// Cross-references: the text stays, the live reference does not. Per reference.
    CrossReferences,
    /// Displayed image size (Markdown has no size). Per image.
    ImageSize,
    /// Merged table cells (spans across columns or rows). Per cell.
    TableSpans,
    /// Table cell shading. Per cell.
    TableFills,
    /// A named table style. Per table.
    TableStyle,
    /// Table borders other than the default. Per table.
    TableBorders,
    /// Column widths that differ from one another. Per table.
    TableColumnWidths,
    /// Minimum row heights. Per row.
    TableRowHeights,
    /// A cell with more than one paragraph (written joined on one line). Per cell.
    TableMultiParagraph,
    /// A second or later header row (only the first stays a header). Per table.
    TableHeaderRows,
}

impl LossKind {
    /// A short British-English noun phrase for the kind.
    pub fn label(self) -> &'static str {
        match self {
            LossKind::FontFamily => "Font changes",
            LossKind::FontSize => "Text sizes",
            LossKind::FontWeight => "Font weights other than regular and bold",
            LossKind::TextColour => "Text colour",
            LossKind::Highlight => "Highlighting",
            LossKind::Underline => "Underlining",
            LossKind::SuperSubscript => "Superscript and subscript",
            LossKind::LetterSpacing => "Letter spacing",
            LossKind::Ligatures => "Ligature settings",
            LossKind::Emphasis => "Bold, italic and strikethrough inside code blocks",
            LossKind::Alignment => "Paragraph alignment",
            LossKind::Indent => "Indents",
            LossKind::LineSpacing => "Line spacing",
            LossKind::ParagraphBorders => "Paragraph borders",
            LossKind::TabStops => "Tab stops",
            LossKind::ParagraphStyle => {
                "Paragraph styles other than body text, headings and quotes"
            }
            LossKind::ListStyle => "Lettered and outline list numbering (written as 1, 2, 3)",
            LossKind::PageBreak => "Page breaks",
            LossKind::SectionBreak => "Section breaks",
            LossKind::PageSetup => "Paper size, orientation and margins",
            LossKind::Columns => "Text columns",
            LossKind::HeadersFooters => "Headers and footers",
            LossKind::PageNumbers => "Page number settings",
            LossKind::Footnotes => "Footnotes and endnotes",
            LossKind::Bookmarks => "Bookmarks",
            LossKind::CrossReferences => "Cross-references (the text stays, the link does not)",
            LossKind::ImageSize => "Image sizes",
            LossKind::TableSpans => "Merged table cells",
            LossKind::TableFills => "Table cell shading",
            LossKind::TableStyle => "Table styles",
            LossKind::TableBorders => "Table borders",
            LossKind::TableColumnWidths => "Table column widths",
            LossKind::TableRowHeights => "Table row heights",
            LossKind::TableMultiParagraph => {
                "Table cells with more than one paragraph (joined into one line)"
            }
            LossKind::TableHeaderRows => "Extra table header rows (only the first stays a header)",
        }
    }
}

impl fmt::Display for LossKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Something a Markdown save would drop, and how many times it occurs. What is counted (a
/// run, a paragraph, a cell, a table, a section) is stated on each [`LossKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loss {
    pub kind: LossKind,
    pub count: usize,
}

impl fmt::Display for Loss {
    /// For example `Text colour: 3 places`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let places = if self.count == 1 { "place" } else { "places" };
        write!(f, "{}: {} {places}", self.kind, self.count)
    }
}

/// Lists the formatting in `document` that saving it as Markdown would drop, in the order of
/// [`LossKind`]. Empty when nothing would be lost. Pure: it reads the document only.
pub fn loss_report(document: &Document) -> Vec<Loss> {
    let mut counts: BTreeMap<LossKind, usize> = BTreeMap::new();
    let mut add = |kind: LossKind, n: usize| {
        if n > 0 {
            *counts.entry(kind).or_insert(0) += n;
        }
    };

    for block in document.body().iter() {
        match block {
            Block::Paragraph(paragraph) => {
                scan_paragraph(document, paragraph, Place::Body, &mut add);
            }
            Block::Table(table) => scan_table(document, table, &mut add),
        }
    }

    for section in &document.sections {
        if section.page != PageSetup::default() {
            add(LossKind::PageSetup, 1);
        }
        if section.columns > 1 {
            add(LossKind::Columns, 1);
        }
        for kind in [HfKind::Header, HfKind::Footer] {
            add(
                LossKind::HeadersFooters,
                section.header_footer(kind).non_empty().count(),
            );
        }
        if section.page_numbers.is_some()
            || section.first_page_number.is_some()
            || section.hide_first_page
        {
            add(LossKind::PageNumbers, 1);
        }
    }

    counts
        .into_iter()
        .map(|(kind, count)| Loss { kind, count })
        .collect()
}

/// Where a paragraph sits, which decides what Markdown can say about it.
#[derive(Clone, Copy)]
enum Place {
    Body,
    /// A table cell; the column's alignment as the first row gives it.
    Cell(Option<Align>),
}

fn scan_table(document: &Document, table: &Table, add: &mut impl FnMut(LossKind, usize)) {
    if table.style.is_some() {
        add(LossKind::TableStyle, 1);
    }
    if table.borders != TableBorders::default() {
        add(LossKind::TableBorders, 1);
    }
    if table.header_rows > 1 {
        add(LossKind::TableHeaderRows, 1);
    }
    if let Some(first) = table.widths_pt.first()
        && table
            .widths_pt
            .iter()
            .any(|width| (width - first).abs() > 0.5)
    {
        add(LossKind::TableColumnWidths, 1);
    }

    // Column alignment lives in the first row, as Markdown keeps it in the delimiter row.
    let mut column_aligns: Vec<Option<Align>> = Vec::new();
    if let Some(first) = table.rows.first() {
        for cell in &first.cells {
            column_aligns.push(
                cell.paragraphs
                    .first()
                    .and_then(|paragraph| paragraph.props.format.align),
            );
            for _ in 1..cell.col_span.max(1) {
                column_aligns.push(None);
            }
        }
    }

    for row in &table.rows {
        if row.min_height_pt > 0.0 {
            add(LossKind::TableRowHeights, 1);
        }
        let mut column = 0usize;
        for cell in &row.cells {
            if cell.col_span > 1 || cell.merged_into_above {
                add(LossKind::TableSpans, 1);
            }
            if cell.fill.is_some() {
                add(LossKind::TableFills, 1);
            }
            if cell.paragraphs.len() > 1 {
                add(LossKind::TableMultiParagraph, 1);
            }
            let align = column_aligns.get(column).copied().flatten();
            for paragraph in &cell.paragraphs {
                scan_paragraph(document, paragraph, Place::Cell(align), add);
            }
            column += usize::from(cell.col_span.max(1));
        }
    }
}

fn scan_paragraph(
    document: &Document,
    paragraph: &Paragraph,
    place: Place,
    add: &mut impl FnMut(LossKind, usize),
) {
    let style_name = document
        .styles
        .get(paragraph.props.style)
        .map(|style| style.name.as_str())
        .unwrap_or("");
    let plain = matches!(
        style_name,
        "Markdown Code" | "Markdown Raw" | "Markdown Rule"
    );

    // Character formatting, per run and kind.
    for run in paragraph.runs() {
        scan_char(&run.format, plain, add);
    }

    // Paragraph formatting.
    let format = &paragraph.props.format;
    if let Some(align) = format.align
        && align != Align::Start
    {
        let kept = match place {
            Place::Cell(column) => {
                column == Some(align) && matches!(align, Align::Centre | Align::End)
            }
            Place::Body => false,
        };
        if !kept {
            add(LossKind::Alignment, 1);
        }
    }
    let quote = style_name == "Quote";
    let nonzero = |value: Option<f32>| value.is_some_and(|v| v != 0.0);
    if (!quote && nonzero(format.indent_left_pt))
        || nonzero(format.indent_right_pt)
        || nonzero(format.indent_first_pt)
    {
        add(LossKind::Indent, 1);
    }
    if format.line_spacing.is_some() {
        add(LossKind::LineSpacing, 1);
    }
    if let Some(borders) = format.borders
        && (borders.sides.top || borders.sides.right || borders.sides.bottom || borders.sides.left)
    {
        add(LossKind::ParagraphBorders, 1);
    }
    if format.tab_stops.is_some_and(|stops| !stops.is_empty()) {
        add(LossKind::TabStops, 1);
    }
    if paragraph.props.page_break_before {
        add(LossKind::PageBreak, 1);
    }
    if paragraph.props.section_start {
        add(LossKind::SectionBreak, 1);
    }

    if matches!(place, Place::Body) {
        if let Some(item) = paragraph.props.list
            && matches!(item.style, ListStyle::Lettered | ListStyle::Harvard)
        {
            add(LossKind::ListStyle, 1);
        }
        let expressible = paragraph.props.style == StyleId::BODY
            || paragraph.props.outline_level.is_some()
            || paragraph.props.list.is_some()
            || quote
            || plain;
        if !expressible {
            add(LossKind::ParagraphStyle, 1);
        }
    }

    add(LossKind::Footnotes, paragraph.notes().len());
    add(LossKind::Bookmarks, paragraph.bookmarks().len());
    add(LossKind::CrossReferences, paragraph.cross_refs().len());
    add(
        LossKind::ImageSize,
        paragraph
            .images()
            .iter()
            .filter(|image| image_size_differs(image))
            .count(),
    );
}

/// Markdown has no image size, so an image comes back at its natural size fitted to the
/// reading width. A size that is already that is not a loss.
fn image_size_differs(image: &InlineImage) -> bool {
    let fitted = InlineImage::fitted(0, image.media.clone(), IMAGE_READING_WIDTH_PT);
    (image.width_pt - fitted.width_pt).abs() > 0.5
        || (image.height_pt - fitted.height_pt).abs() > 0.5
}

fn is_monospace(family: &str) -> bool {
    let family = family.to_ascii_lowercase();
    family.contains("courier") || family.contains("mono")
}

fn scan_char(format: &CharFormat, plain: bool, add: &mut impl FnMut(LossKind, usize)) {
    if plain {
        // Inside a code block, raw text or rule the runs are not written at all.
        if format.weight.is_some_and(|w| w >= 600)
            || format.italic == Some(true)
            || format.strikethrough == Some(true)
        {
            add(LossKind::Emphasis, 1);
        }
    } else {
        if format.family.as_deref().is_some_and(|f| !is_monospace(f)) {
            add(LossKind::FontFamily, 1);
        }
        if format.weight.is_some_and(|w| w < 600 && w != 400) {
            add(LossKind::FontWeight, 1);
        }
    }
    if format.size_pt.is_some() {
        add(LossKind::FontSize, 1);
    }
    if format.underline == Some(true) {
        add(LossKind::Underline, 1);
    }
    if format.colour.is_some() {
        add(LossKind::TextColour, 1);
    }
    if matches!(format.highlight, Some(Some(_))) {
        add(LossKind::Highlight, 1);
    }
    if matches!(
        format.vertical,
        Some(VerticalAlign::Superscript | VerticalAlign::Subscript)
    ) {
        add(LossKind::SuperSubscript, 1);
    }
    if format
        .letter_spacing_pt
        .is_some_and(|spacing| spacing != 0.0)
    {
        add(LossKind::LetterSpacing, 1);
    }
    if matches!(format.ligatures, Some(Ligatures::None | Ligatures::All)) {
        add(LossKind::Ligatures, 1);
    }
}
