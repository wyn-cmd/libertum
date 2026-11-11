//! Writes an ODT with an inline image, a bookmark with a text and a page cross-reference, and a
//! banded table, for checking the export in Word (and LibreOffice, where installed).
//!
//! `cargo run -p libertum-odt --example phase6_sample -- target/phase6-sample.odt`

use std::path::PathBuf;

use libertum_formats_common::media::sample_png;
use libertum_model::{
    Block, BlockId, Bookmark, CrossRef, CrossRefKind, Document, InlineImage, Paragraph, Table,
    TableBorders, TableCell, TableRow, TableStyle,
};
use libertum_odt::Odt;

fn paragraph(text: &str) -> Paragraph {
    Paragraph::new(BlockId(0), text)
}

fn cell(text: &str) -> TableCell {
    TableCell::new(paragraph(text))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/phase6-sample.odt"));

    let mut heading = paragraph("The anchor paragraph");
    heading.set_bookmarks(vec![Bookmark {
        name: "Anchor".into(),
        offset: 0,
    }]);

    let mut picture = paragraph("A picture sits here: and the text carries on.");
    picture.set_images(vec![InlineImage {
        offset: 22,
        media: sample_png(64, 32)?,
        width_pt: 96.0,
        height_pt: 48.0,
        alt: "A sample picture".into(),
        name: "sample".into(),
    }]);

    let text = "See The anchor paragraph on page 1.";
    let mut reference = paragraph(text);
    reference.set_cross_refs(vec![
        CrossRef {
            range: 4..24,
            target: "Anchor".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 33..34,
            target: "Anchor".into(),
            kind: CrossRefKind::Page,
        },
    ]);

    let (style, borders) = TableStyle::builtin("Banded Rows").ok_or("no built-in style")?;
    let mut table = Table {
        style: Some(style),
        id: BlockId(0),
        widths_pt: vec![120.0, 120.0],
        rows: (0..5)
            .map(|row| TableRow {
                cells: vec![cell(&format!("Row {row} A")), cell(&format!("Row {row} B"))],
                min_height_pt: 0.0,
                keep_together: true,
            })
            .collect(),
        header_rows: 1,
        borders: TableBorders::default(),
    };
    table.borders = borders;
    table.normalise();

    let document = Document::from_blocks(vec![
        Block::Paragraph(heading),
        Block::Paragraph(picture),
        Block::Paragraph(reference),
        Block::Table(table),
    ]);
    Odt::new(document).save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
