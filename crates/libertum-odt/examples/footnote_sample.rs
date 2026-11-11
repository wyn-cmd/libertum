//! Writes a one-page ODT with a footnote, for checking the exported note in Word.
//!
//! `cargo run -p libertum-odt --example footnote_sample -- target/footnote.odt`

use std::path::PathBuf;

use libertum_model::{Block, Document, Paragraph};
use libertum_odt::Odt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/footnote.odt"));
    let mut paragraph = Paragraph::new(
        libertum_model::BlockId(0),
        "This paragraph carries a footnote.",
    );
    paragraph.set_note(Some("The exported note text.".into()));
    let document = Document::from_blocks(vec![Block::Paragraph(paragraph)]);
    Odt::new(document).save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
