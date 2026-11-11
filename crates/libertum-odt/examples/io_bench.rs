//! Measures ODT open and save for a 500-page-sized text document.
//!
//! `cargo run --release -p libertum-odt --example io_bench`
//!
//! The document is generated (20,000 paragraphs, about 500 pages of body text), written to
//! memory, and read back. Timings are for the format layer only; the app also paginates and
//! lays out in the background while opening.

use std::time::Instant;

use libertum_model::{BlockId, Document, Paragraph};
use libertum_odt::Odt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let count = 20_000usize;
    let paragraphs = (0..count)
        .map(|index| {
            Paragraph::new(
                BlockId(0),
                format!(
                    "Paragraph {index} of a long document, with enough words to take more than one line on an A4 page at twelve points and normal margins."
                ),
            )
        })
        .collect();
    let document = Document::from_paragraphs(paragraphs);
    println!("{count} paragraphs, {} words", document.word_count());

    let start = Instant::now();
    let bytes = Odt::new(document).to_bytes()?;
    let save = start.elapsed();
    println!("save {save:.2?}, {} KiB", bytes.len() / 1024);

    let start = Instant::now();
    let parsed = Odt::parse(&bytes)?;
    let open = start.elapsed();
    println!(
        "open {open:.2?}, {} paragraphs, {} words",
        parsed.document().paragraph_count(),
        parsed.document().word_count()
    );
    Ok(())
}
