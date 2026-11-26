//! The "what will be lost" report for an HTML save, per flavour.
//!
//! A full save keeps the look of the text (fonts, sizes, colours, spacing, borders, page
//! colour) but not the page furniture an endless HTML page has no place for. A plain save
//! keeps the structure (headings, emphasis, lists, quotes, tables, images, notes) and drops
//! the look. Counts are per paragraph, run, section, table or field, as each kind says.

use std::collections::BTreeMap;

use libertum_model::{Block, Document, HfKind, Paragraph, ResolvedChar, StyleId};

use crate::HtmlFlavour;
use crate::style::Baseline;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LossKind {
    /// Headers and footers (a pageless HTML page has none). Per header or footer.
    HeadersFooters,
    /// Page numbers. Per document.
    PageNumbers,
    /// More than one text column. Per section.
    Columns,
    /// A watermark. Per section.
    Watermark,
    /// Section breaks (written as page breaks in a full save). Per break.
    SectionBreak,
    /// Tab stops. Per paragraph.
    TabStops,
    /// Generated table-of-contents entries (left out). Per entry.
    TableOfContents,
    /// Cross-references and date fields: the text stays, the live field does not. Per field.
    Fields,
    /// Paragraph style names Libertum's HTML has no place for (their look is kept in a full
    /// save). Per paragraph.
    ParagraphStyleName,
    /// Named table styles (a full save keeps their cell shading). Per table.
    TableStyle,
    /// Minimum table row heights. Per row.
    TableRowHeights,
    /// Heading levels 7 to 9 (HTML stops at 6). Per heading.
    DeepHeadings,
    /// Plain only: fonts, sizes, colours, highlighting, letter spacing and ligatures. Per
    /// run.
    CharacterFormatting,
    /// Plain only: indents, spacing, line spacing and borders (alignment is kept). Per
    /// paragraph.
    ParagraphFormatting,
    /// Plain only: page breaks. Per break.
    PageBreak,
    /// Plain only: the page colour. Per document.
    PageColour,
    /// Plain only: paper size and margins. Per document.
    PageSetup,
    /// Plain only: table borders, shading and column widths. Per table.
    TableFormatting,
}

impl LossKind {
    pub fn label(self) -> &'static str {
        match self {
            LossKind::HeadersFooters => "Headers and footers",
            LossKind::PageNumbers => "Page numbers",
            LossKind::Columns => "Text columns",
            LossKind::Watermark => "Watermark",
            LossKind::SectionBreak => "Section breaks",
            LossKind::TabStops => "Tab stops",
            LossKind::TableOfContents => "Table of contents entries",
            LossKind::Fields => "Cross-references and date fields (the text stays)",
            LossKind::ParagraphStyleName => "Paragraph style names",
            LossKind::TableStyle => "Table style names",
            LossKind::TableRowHeights => "Table row heights",
            LossKind::DeepHeadings => "Heading levels below 6",
            LossKind::CharacterFormatting => "Fonts, sizes and colours",
            LossKind::ParagraphFormatting => "Indents and spacing",
            LossKind::PageBreak => "Page breaks",
            LossKind::PageColour => "Page colour",
            LossKind::PageSetup => "Paper size and margins",
            LossKind::TableFormatting => "Table borders, shading and widths",
        }
    }
}

/// One kind of loss and how many times it occurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loss {
    pub kind: LossKind,
    pub count: usize,
}

/// What saving `document` as HTML in `flavour` would drop, most important first.
pub fn loss_report(document: &Document, flavour: HtmlFlavour) -> Vec<Loss> {
    let plain = flavour == HtmlFlavour::Plain;
    let mut counts: BTreeMap<LossKind, usize> = BTreeMap::new();
    let mut add = |kind: LossKind, n: usize| {
        if n > 0 {
            *counts.entry(kind).or_default() += n;
        }
    };
    for section in &document.sections {
        for kind in [HfKind::Header, HfKind::Footer] {
            add(
                LossKind::HeadersFooters,
                section.header_footer(kind).non_empty().count(),
            );
        }
        add(LossKind::Columns, usize::from(section.columns > 1));
        add(
            LossKind::Watermark,
            usize::from(section.watermark.is_some()),
        );
    }
    add(
        LossKind::PageNumbers,
        usize::from(document.page_numbering().show),
    );
    let first = document.sections.first();
    if plain {
        add(
            LossKind::PageColour,
            usize::from(first.is_some_and(|s| s.page_colour.is_some())),
        );
        add(
            LossKind::PageSetup,
            usize::from(first.is_some_and(|s| s.page != libertum_model::PageSetup::default())),
        );
    }
    let d = &document.styles.defaults;
    let baseline = Baseline::new(&d.family, d.size_pt, d.colour);
    let check = |p: &Paragraph, add: &mut dyn FnMut(LossKind, usize)| {
        add(LossKind::SectionBreak, usize::from(p.props.section_start));
        add(
            LossKind::TabStops,
            usize::from(p.props.format.tab_stops.is_some_and(|t| !t.is_empty())),
        );
        add(
            LossKind::TableOfContents,
            usize::from(p.props.toc_target.is_some()),
        );
        add(
            LossKind::Fields,
            p.cross_refs().len() + p.date_fields().len(),
        );
        add(
            LossKind::DeepHeadings,
            usize::from(p.props.outline_level.is_some_and(|l| l > 6)),
        );
        let name = document
            .styles
            .get(p.props.style)
            .map(|s| s.name.as_str())
            .unwrap_or("");
        if baseline.style_named(name).is_none() {
            add(LossKind::ParagraphStyleName, 1);
        }
        if plain {
            add(LossKind::PageBreak, usize::from(p.props.page_break_before));
            // Alignment is kept (as the `align` attribute); the rest of the format is not.
            let mut rest = p.props.format;
            rest.align = None;
            if !rest.is_empty() {
                add(LossKind::ParagraphFormatting, 1);
            }
            let style = document.styles.resolve_char(p.props.style);
            let base = baseline.sheet.resolve_char(StyleId::BODY);
            let looks = |c: &ResolvedChar| {
                c.family != base.family
                    || (c.size_pt - base.size_pt).abs() > 0.01
                    || c.colour[..3] != base.colour[..3]
                    || c.highlight.is_some()
                    || c.letter_spacing_pt.abs() > 0.001
                    || c.ligatures != base.ligatures
            };
            let runs = p
                .runs()
                .iter()
                .filter(|r| p.props.outline_level.is_none() && looks(&style.apply(&r.format)))
                .count();
            add(LossKind::CharacterFormatting, runs);
        }
    };
    for block in document.body().iter() {
        match block {
            Block::Paragraph(p) => check(p, &mut add),
            Block::Table(table) => {
                add(LossKind::TableStyle, usize::from(table.style.is_some()));
                add(
                    LossKind::TableRowHeights,
                    table.rows.iter().filter(|r| r.min_height_pt > 0.0).count(),
                );
                if plain {
                    let fills = table.rows.iter().enumerate().any(|(r, row)| {
                        row.cells
                            .iter()
                            .any(|c| table.resolved_fill(r, c).is_some())
                    });
                    let widths = table
                        .widths_pt
                        .windows(2)
                        .any(|w| (w[0] - w[1]).abs() > 0.5);
                    add(LossKind::TableFormatting, usize::from(fills || widths));
                }
                for row in &table.rows {
                    for cell in &row.cells {
                        for p in &cell.paragraphs {
                            check(p, &mut add);
                        }
                    }
                }
            }
        }
    }
    counts
        .into_iter()
        .map(|(kind, count)| Loss { kind, count })
        .collect()
}
