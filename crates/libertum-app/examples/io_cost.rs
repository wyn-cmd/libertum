//! Open and save times on a generated document (default 500 pages), per format, against the
//! PLAN §2 budgets: first page visible < 1.5 s, full pagination < 8 s, save < 2 s.
//! `cargo run --release -p libertum-app --example io_cost -- 500`

use std::time::Instant;

use libertum_docx::Docx;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
use libertum_native::Native;
use libertum_odt::Odt;

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let pages: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(500);
    let document = libertum_layout::spike::generate_model_document(pages, 11);

    let started = Instant::now();
    let docx = Docx::new(document.clone()).to_bytes().expect("write docx");
    println!(
        "DOCX save ({} KB): {:.0} ms",
        docx.len() / 1024,
        ms(started)
    );
    let started = Instant::now();
    let odt = Odt::new(document.clone()).to_bytes().expect("write odt");
    println!("ODT  save ({} KB): {:.0} ms", odt.len() / 1024, ms(started));
    let started = Instant::now();
    let native = Native::new(document.clone())
        .to_bytes()
        .expect("write lbtd");
    println!(
        "LBTD save ({} KB): {:.0} ms",
        native.len() / 1024,
        ms(started)
    );

    let started = Instant::now();
    let parsed = Docx::parse(&docx).expect("read docx");
    println!("DOCX parse: {:.0} ms", ms(started));
    let started = Instant::now();
    let _ = Odt::parse(&odt).expect("read odt");
    println!("ODT  parse: {:.0} ms", ms(started));
    let started = Instant::now();
    let _ = Native::parse(&native).expect("read lbtd");
    println!("LBTD parse: {:.0} ms", ms(started));

    // What the app does after parsing: lay out until the first page exists, then the rest.
    let document = parsed.document();
    let geometry = PageGeometry::from_setup(&document.page_setup());
    let started = Instant::now();
    let mut state = LayoutState::from_document(document, geometry, LayoutOptions::default());
    println!("layout state built: {:.0} ms", ms(started));
    let started = Instant::now();
    state.paginate_all();
    println!(
        "full pagination: {:.0} ms ({} pages)",
        ms(started),
        state.page_count()
    );
}
