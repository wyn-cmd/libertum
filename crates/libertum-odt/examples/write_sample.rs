//! Writes a small ODT that exercises the exporter, for opening in another word processor.
//!
//! `cargo run -p libertum-odt --example write_sample -- target/sample.odt`

use std::path::PathBuf;

use libertum_model::{
    Align, Block, BlockId, CharFormat, Document, HeaderFooter, ListItem, ListStyle, Margins,
    PageNumberPos, PageSetup, ParaFormat, ParaProps, Paragraph, Run, Section, StyleId, Table,
    TableBorders, TableCell, TableRow,
};
use libertum_odt::Odt;

fn paragraph(text: &str, props: ParaProps, runs: Vec<Run>) -> Block {
    Block::Paragraph(Paragraph::with_runs(BlockId(0), text, runs).with_props(props))
}

fn cell(text: &str, fill: Option<[u8; 4]>) -> TableCell {
    TableCell {
        fill,
        ..TableCell::new(Paragraph::new(BlockId(0), text))
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/sample.odt"));

    let mut blocks = Vec::new();
    blocks.push(paragraph(
        "Libertum ODT sample",
        ParaProps {
            style: StyleId::TITLE,
            ..ParaProps::default()
        },
        Vec::new(),
    ));
    let text = "Plain text, bold text, italic text and a coloured word.";
    let bold_at = text.find("bold").expect("bold");
    let italic_at = text.find("italic").expect("italic");
    let colour_at = text.find("coloured").expect("coloured");
    blocks.push(paragraph(
        text,
        ParaProps::default(),
        vec![
            Run {
                range: bold_at..bold_at + 4,
                format: CharFormat::bold(),
            },
            Run {
                range: italic_at..italic_at + 6,
                format: CharFormat {
                    italic: Some(true),
                    ..CharFormat::default()
                },
            },
            Run {
                range: colour_at..colour_at + 8,
                format: CharFormat {
                    colour: Some([200, 30, 30, 255]),
                    ..CharFormat::default()
                },
            },
        ],
    ));
    blocks.push(paragraph(
        "A heading follows.",
        ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        },
        Vec::new(),
    ));
    let bullet = ListItem {
        style: ListStyle::Bullet,
        level: 0,
        group: 1,
    };
    for text in ["First bullet", "Second bullet"] {
        blocks.push(paragraph(
            text,
            ParaProps {
                list: Some(bullet),
                ..ParaProps::default()
            },
            Vec::new(),
        ));
    }
    blocks.push(paragraph(
        "A page break comes next.",
        ParaProps {
            page_break_before: true,
            format: ParaFormat {
                align: Some(Align::Centre),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
        Vec::new(),
    ));
    blocks.push(Block::Table(Table {
        style: None,
        id: BlockId(0),
        widths_pt: vec![120.0, 180.0],
        rows: vec![
            TableRow {
                cells: vec![
                    cell("Header A", Some([230, 240, 255, 255])),
                    cell("Header B", Some([230, 240, 255, 255])),
                ],
                min_height_pt: 20.0,
                keep_together: true,
            },
            TableRow {
                cells: vec![cell("Body A", None), cell("Body B", None)],
                min_height_pt: 0.0,
                keep_together: true,
            },
        ],
        header_rows: 1,
        borders: TableBorders::default(),
    }));
    blocks.push(paragraph(
        "A second section starts here.",
        ParaProps {
            section_start: true,
            ..ParaProps::default()
        },
        Vec::new(),
    ));

    let mut document = Document::from_blocks(blocks);
    let mut first = Section {
        page: PageSetup {
            margins: Margins::uniform(72.0),
            ..PageSetup::a4()
        },
        page_numbers: Some(PageNumberPos::TOP_RIGHT),
        ..Section::default()
    };
    first.headers.default = Some(HeaderFooter::new("Sample header"));
    first.footers.default = Some(HeaderFooter::new("Page {page} of {pages}"));
    let second = Section {
        page: PageSetup::letter(),
        ..Section::default()
    };
    document.sections = vec![first, second];

    Odt::new(document).save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
