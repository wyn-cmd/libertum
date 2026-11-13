//! Writes the demo document as a normal PDF and as PDF/A, for checking the export in a
//! viewer (`pdftotext`, `pdfinfo`, `qpdf`, a PDF/A validator). The sample also carries a
//! running header, a page-number footer and a table, so those paths are exercised too.
//!
//!     cargo run -p libertum-render --example export_pdf

use libertum_layout::spike::generate_model_document;
use libertum_model::{Block, HeadersFooters, Paragraph};

fn sample() -> libertum_model::Document {
    let mut document = generate_model_document(2, 42);
    if let Some(section) = document.sections.first_mut() {
        section.headers = HeadersFooters::with_default("RUNNINGHEAD");
        section.footers = HeadersFooters::with_default("PAGE {page} OF {pages}");
        section.watermark = libertum_model::Watermark::new("DRAFT", true);
    }
    let mut table = document.new_table(2, 2, 400.0);
    let ids: Vec<_> = (0..4).map(|_| document.alloc_id()).collect();
    for (cell, id) in table
        .rows
        .iter_mut()
        .flat_map(|row| row.cells.iter_mut())
        .zip(ids)
    {
        cell.paragraphs[0] = Paragraph::new(id, "TABLECELL");
    }
    document.append_blocks(vec![Block::Table(table)]);
    document
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("target")?;
    for (path, archival) in [
        ("target/pdf-sample.pdf", false),
        ("target/pdf-sample-a.pdf", true),
    ] {
        let bytes = libertum_render::pdf::export(&sample(), archival, "Libertum PDF sample")?;
        std::fs::write(path, bytes)?;
        println!("wrote {path}");
    }
    Ok(())
}
