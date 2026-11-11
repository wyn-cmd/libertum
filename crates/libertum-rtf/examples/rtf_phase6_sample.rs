//! Writes an RTF with a picture, a bookmark with a text and a page cross-reference, and a
//! banded table, for checking in Microsoft Word (and LibreOffice or Pages where available).
//!
//! Run with `cargo run -p libertum-rtf --example rtf_phase6_sample [-- <path>]`. The default
//! output is `target/phase6-sample.rtf` in the workspace.

use std::path::PathBuf;

use libertum_formats_common::media::sample_png;
use libertum_model::{
    Block, BlockId, Bookmark, CrossRef, CrossRefKind, Document, InlineImage, ParaProps, Paragraph,
    StyleId, Table, TableCell, TableRow, TableStyle,
};
use libertum_rtf::Rtf;

fn paragraph(text: &str) -> Paragraph {
    Paragraph::new(BlockId(0), text)
}

fn cell(text: &str) -> TableCell {
    TableCell::new(paragraph(text))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/phase6-sample.rtf")
        });

    let heading = Paragraph::new(BlockId(0), "Phase 6 sample").with_props(ParaProps {
        style: StyleId::HEADING_1,
        outline_level: Some(1),
        ..ParaProps::default()
    });

    let mut picture = paragraph("A picture follows: ");
    let mut image = InlineImage::fitted(picture.text().len(), sample_png(64, 32)?, 200.0);
    image.alt = "A blue and grey checker pattern".to_owned();
    image.name = "checker.png".to_owned();
    picture.set_images(vec![image]);

    let mut caption = paragraph("Figure 1: the checker pattern");
    caption.set_bookmarks(vec![Bookmark {
        name: "fig_checker".to_owned(),
        offset: 0,
    }]);

    let sentence = "See Figure 1: the checker pattern on page 1 for the picture.";
    let text_at = sentence.find("Figure 1").unwrap_or(0);
    let page_at = sentence.find("page 1").unwrap_or(0) + "page ".len();
    let mut reference = paragraph(sentence);
    reference.set_cross_refs(vec![
        CrossRef {
            range: text_at..text_at + "Figure 1: the checker pattern".len(),
            target: "fig_checker".to_owned(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: page_at..page_at + 1,
            target: "fig_checker".to_owned(),
            kind: CrossRefKind::Page,
        },
    ]);

    let (style, borders) = TableStyle::builtin("Banded Rows").ok_or("no Banded Rows style")?;
    let rows = (0..5)
        .map(|row| TableRow {
            cells: vec![
                cell(&if row == 0 {
                    "Item".to_owned()
                } else {
                    format!("Row {row}")
                }),
                cell(&if row == 0 {
                    "Value".to_owned()
                } else {
                    format!("{}", row * 10)
                }),
            ],
            min_height_pt: 0.0,
            keep_together: true,
        })
        .collect();
    let table = Table {
        style: Some(style),
        id: BlockId(0),
        widths_pt: vec![140.0, 100.0],
        rows,
        header_rows: 1,
        borders,
    };

    let document = Document::from_blocks(vec![
        Block::Paragraph(heading),
        Block::Paragraph(picture),
        Block::Paragraph(caption),
        Block::Paragraph(reference),
        Block::Table(table),
        Block::Paragraph(paragraph("End of the sample.")),
    ]);

    let rtf = Rtf::new(document);
    let bytes = rtf.to_bytes()?;
    std::fs::write(&path, &bytes)?;
    println!("wrote {} bytes to {}", bytes.len(), path.display());
    Ok(())
}
