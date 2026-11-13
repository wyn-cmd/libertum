//! Prints how many pages Libertum's own pagination makes of a document, for comparing with
//! LibreOffice's page count (the Phase 5 acceptance check, run by CI).
//!
//! `cargo run --release -p libertum-app --example page_count -- tests/fixtures/odt/rich.odt`
//!
//! The count is the only thing written to stdout, so a shell job can read it directly;
//! anything that goes wrong is written to stderr with a non-zero exit.

use std::path::{Path, PathBuf};

use libertum_docx::Docx;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
use libertum_model::Document;
use libertum_native::Native;
use libertum_odt::Odt;
use libertum_rtf::Rtf;

fn main() {
    let Some(path) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: page_count <document.odt|.docx|.rtf|.lbtd>");
        std::process::exit(2);
    };
    match open(&path) {
        Ok(document) => {
            let geometry = PageGeometry::from_setup(&document.page_setup());
            let mut state =
                LayoutState::from_document(&document, geometry, LayoutOptions::default());
            state.paginate_all();
            println!("{}", state.page_count());
        }
        Err(error) => {
            eprintln!("page_count: {error}");
            std::process::exit(1);
        }
    }
}

/// Opens a document in any format the app reads, so the same tool serves every fixture.
fn open(path: &Path) -> Result<Document, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "odt" => Odt::open(path)
            .map(|odt| odt.document().clone())
            .map_err(|error| error.to_string()),
        "docx" => Docx::open(path)
            .map(|docx| docx.document().clone())
            .map_err(|error| error.to_string()),
        "rtf" => Rtf::open(path)
            .map(|rtf| rtf.document().clone())
            .map_err(|error| error.to_string()),
        "lbtd" => Native::open(path)
            .map(|native| native.document().clone())
            .map_err(|error| error.to_string()),
        other => Err(format!(
            "unknown document type \".{other}\" (expected .odt, .docx, .rtf or .lbtd)"
        )),
    }
}
