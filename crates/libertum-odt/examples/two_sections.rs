//! Writes a two-section ODT with portrait then landscape A4 pages, for layout screenshots.
//!
//! `cargo run -p libertum-odt --example two_sections -- target/mixed-sections.odt`

use std::path::PathBuf;

use libertum_model::{
    Block, BlockId, Document, Margins, Orientation, PageSetup, PaperSize, ParaProps, Paragraph,
    Section,
};
use libertum_odt::Odt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/mixed-sections.odt"));
    let mut paragraphs = Vec::new();
    for i in 0..18 {
        paragraphs.push(Paragraph::new(
            BlockId(0),
            format!(
                "Portrait page line {}. The quick brown fox jumps over the lazy dog.",
                i + 1
            ),
        ));
    }
    paragraphs.push(
        Paragraph::new(
            BlockId(0),
            "Landscape section: this page uses A4 landscape with wider margins.",
        )
        .with_props(ParaProps {
            section_start: true,
            ..ParaProps::default()
        }),
    );
    for i in 0..18 {
        paragraphs.push(Paragraph::new(
            BlockId(0),
            format!(
                "Landscape page line {}. The quick brown fox jumps over the lazy dog.",
                i + 1
            ),
        ));
    }
    let mut document =
        Document::from_blocks(paragraphs.into_iter().map(Block::Paragraph).collect());
    document.sections = vec![
        Section {
            page: PageSetup {
                paper: PaperSize::A4,
                orientation: Orientation::Portrait,
                margins: Margins::uniform(72.0),
            },
            ..Section::default()
        },
        Section {
            page: PageSetup {
                paper: PaperSize::A4,
                orientation: Orientation::Landscape,
                margins: Margins::uniform(72.0),
            },
            ..Section::default()
        },
    ];
    Odt::new(document).save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
