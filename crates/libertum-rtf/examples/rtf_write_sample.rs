//! Writes a rich sample RTF for external validation (Word, LibreOffice, Pages).
//!
//! Run with `cargo run -p libertum-rtf --example rtf_write_sample -- <path>`.

use std::path::PathBuf;

use libertum_model::{
    Align, BlockId, CharFormat, Document, HeaderFooter, HeadersFooters, LineSpacing, ListItem,
    ListStyle, Margins, Orientation, PageSetup, PaperSize, ParaFormat, ParaProps, Paragraph, Run,
    Section, StyleId, StyleSheet, Table, TableBorders, TableCell, TableRow, VerticalAlign,
};
use libertum_rtf::Rtf;

fn paragraph(text: &str, props: ParaProps, runs: Vec<Run>) -> Paragraph {
    Paragraph::with_runs(BlockId(0), text, runs).with_props(props)
}

fn cell(text: &str) -> TableCell {
    TableCell::new(paragraph(text, ParaProps::default(), Vec::new()))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target-rtf/libertum_rtf_sample.rtf"));

    let mut styles = StyleSheet::with_defaults("Georgia", 12.0);
    styles.paragraph[StyleId::HEADING_1.0 as usize].char.colour = Some([0x1A, 0x1A, 0x1A, 255]);

    let mut blocks = Vec::new();
    blocks.push(libertum_model::Block::Paragraph(paragraph(
        "Libertum RTF Sample",
        ParaProps {
            style: StyleId::TITLE,
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(libertum_model::Block::Paragraph(paragraph(
        "Chapter One",
        ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        },
        Vec::new(),
    )));

    let text = "Bold, coloured and super text with a	tab and a page break.";
    let bold_at = text.find("Bold").unwrap_or(0);
    let colour_at = text.find("coloured").unwrap_or(0);
    let super_at = text.find("super").unwrap_or(0);
    blocks.push(libertum_model::Block::Paragraph(paragraph(
        text,
        ParaProps {
            format: ParaFormat {
                align: Some(Align::Justify),
                space_after_pt: Some(6.0),
                line_spacing: Some(LineSpacing::Multiple(1.25)),
                keep_together: Some(true),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
        vec![
            Run {
                range: bold_at..bold_at + 4,
                format: CharFormat::bold(),
            },
            Run {
                range: colour_at..colour_at + 8,
                format: CharFormat {
                    colour: Some([0xAA, 0x00, 0x00, 255]),
                    highlight: Some(Some([0xFF, 0xFF, 0x00, 255])),
                    ..CharFormat::default()
                },
            },
            Run {
                range: super_at..super_at + 5,
                format: CharFormat {
                    vertical: Some(VerticalAlign::Superscript),
                    ..CharFormat::default()
                },
            },
        ],
    )));
    blocks.push(libertum_model::Block::Paragraph(paragraph(
        "This paragraph starts a new page.",
        ParaProps {
            page_break_before: true,
            ..ParaProps::default()
        },
        Vec::new(),
    )));

    let bullet = ListItem {
        style: ListStyle::Bullet,
        level: 0,
        group: 1,
    };
    let numbered = ListItem {
        style: ListStyle::Numbered,
        level: 0,
        group: 2,
    };
    let nested = ListItem {
        style: ListStyle::Numbered,
        level: 1,
        group: 2,
    };
    for (label, item) in [
        ("First bullet", bullet),
        ("Second bullet", bullet),
        ("First number", numbered),
        ("Nested number", nested),
        ("Second number", numbered),
    ] {
        blocks.push(libertum_model::Block::Paragraph(paragraph(
            label,
            ParaProps {
                list: Some(item),
                ..ParaProps::default()
            },
            Vec::new(),
        )));
    }

    let table = Table {
        style: None,
        id: BlockId(0),
        widths_pt: vec![140.0, 200.0],
        rows: vec![
            TableRow {
                cells: vec![cell("Head A"), cell("Head B")],
                min_height_pt: 0.0,
                keep_together: true,
            },
            TableRow {
                cells: vec![cell("Body A"), cell("Body B")],
                min_height_pt: 0.0,
                keep_together: true,
            },
        ],
        header_rows: 1,
        borders: TableBorders {
            width_pt: 1.0,
            colour: [0, 0, 0, 255],
            outer: true,
            inner: true,
        },
    };
    blocks.push(libertum_model::Block::Table(table));
    blocks.push(libertum_model::Block::Paragraph(paragraph(
        "Second section.",
        ParaProps {
            section_start: true,
            ..ParaProps::default()
        },
        Vec::new(),
    )));

    let mut document = Document::from_blocks(blocks);
    document.styles = styles;

    let mut first = Section {
        page: PageSetup {
            paper: PaperSize::A4,
            orientation: Orientation::Landscape,
            margins: Margins {
                top: 54.0,
                right: 45.0,
                bottom: 45.0,
                left: 54.0,
            },
        },
        first_page_number: Some(1),
        columns: 2,
        column_gutter_pt: 24.0,
        header_distance_pt: 30.0,
        footer_distance_pt: 36.0,
        ..Section::default()
    };
    first.headers = HeadersFooters {
        default: Some(HeaderFooter::new("Libertum sample")),
        first: Some(HeaderFooter::new("First page")),
        different_first: true,
        ..HeadersFooters::default()
    };
    let mut footer = HeaderFooter::new("Page {page} of {pages}");
    footer.align = Align::End;
    first.footers = HeadersFooters {
        default: Some(footer),
        ..HeadersFooters::default()
    };
    let second = Section {
        page: PageSetup {
            paper: PaperSize::Letter,
            orientation: Orientation::Portrait,
            margins: Margins::uniform(54.0),
        },
        ..Section::default()
    };
    document.sections = vec![first, second];

    let rtf = Rtf::new(document);
    rtf.save(&path)?;
    let bytes = rtf.to_bytes()?;
    println!(
        "wrote {} ({} bytes) to {}",
        path.display(),
        bytes.len(),
        path.display()
    );
    Ok(())
}
