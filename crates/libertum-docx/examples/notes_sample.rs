//! Writes a DOCX with two footnotes and an endnote for Word validation.
//!
//! `cargo run -p libertum-docx --example notes_sample -- target/notes.docx`

use std::path::PathBuf;

use libertum_docx::Docx;
use libertum_model::{Block, Document, Note, NoteKind, Paragraph};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/notes.docx"));
    let mut paragraph = Paragraph::new(libertum_model::BlockId(0), "A paragraph with notes.");
    paragraph.set_notes(vec![
        Note {
            kind: NoteKind::Footnote,
            offset: 2,
            text: "First footnote.".into(),
        },
        Note {
            kind: NoteKind::Footnote,
            offset: 12,
            text: "Second footnote.".into(),
        },
        Note {
            kind: NoteKind::Endnote,
            offset: 20,
            text: "The endnote.".into(),
        },
    ]);
    let document = Document::from_blocks(vec![Block::Paragraph(paragraph)]);
    Docx::new(document).save(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}
