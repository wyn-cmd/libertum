//! Writes a DOCX with a picture, a bookmark with a text and a page cross-reference, and a
//! banded table, for checking in Microsoft Word.
//!
//! `cargo run -p libertum-docx --example docx_phase6_sample -- target/phase6-sample.docx`

use std::path::PathBuf;

use libertum_docx::Docx;
use libertum_formats_common::media::sample_png;
use libertum_model::{
    Block, BlockId, Bookmark, CharFormat, CrossRef, CrossRefKind, Document, InlineImage, Paragraph,
    Run, Table, TableBorders, TableCell, TableRow, TableStyle,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/phase6-sample.docx"));

    let mut heading = Paragraph::new(BlockId(1), "Introduction");
    heading.set_bookmarks(vec![Bookmark {
        name: "Intro".into(),
        offset: 0,
    }]);

    let mut body = Paragraph::with_runs(
        BlockId(2),
        "See Introduction on page 1 for the picture here.",
        vec![Run {
            range: 4..16,
            format: CharFormat::bold(),
        }],
    );
    body.set_cross_refs(vec![
        CrossRef {
            range: 4..16,
            target: "Intro".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 25..26,
            target: "Intro".into(),
            kind: CrossRefKind::Page,
        },
    ]);
    body.set_images(vec![InlineImage {
        offset: 42,
        media: sample_png(32, 16)?,
        width_pt: 96.0,
        height_pt: 48.0,
        alt: "A checker pattern".into(),
        name: "checker.png".into(),
    }]);

    let style = TableStyle::builtin("Banded Rows")
        .ok_or("the built-in Banded Rows style is missing")?
        .0;
    let mut next = 11u64;
    let rows: Vec<TableRow> = (0..5)
        .map(|row| TableRow {
            cells: (0..2)
                .map(|column| {
                    let cell = TableCell::new(Paragraph::new(
                        BlockId(next),
                        format!("row {row}, column {column}"),
                    ));
                    next += 1;
                    cell
                })
                .collect(),
            min_height_pt: 0.0,
            keep_together: true,
        })
        .collect();
    let table = Table {
        id: BlockId(10),
        widths_pt: vec![150.0, 150.0],
        rows,
        header_rows: 1,
        borders: TableBorders::default(),
        style: Some(style),
    };

    let document = Document::from_blocks(vec![
        Block::Paragraph(heading),
        Block::Paragraph(body),
        Block::Table(table),
    ]);
    Docx::new(document).save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
