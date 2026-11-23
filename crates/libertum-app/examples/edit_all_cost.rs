//! How long document-wide and early edits block the UI thread on a generated document
//! (default 500 pages). `DocView::sync` runs on the UI thread, so these numbers are what the
//! user waits for after Select All + Bold, a style change, or an edit near the top.
//! `cargo run --release -p libertum-app --example edit_all_cost -- 500`

use std::time::Instant;

use libertum_editor::Editor;
use libertum_layout::spike::ParagraphSnapshot;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let pages: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(500);
    let document = libertum_layout::spike::generate_model_document(pages, 11);
    let geometry = PageGeometry::from_setup(&document.page_setup());
    let mut state = LayoutState::from_document(&document, geometry, LayoutOptions::default());
    state.paginate_all();
    println!("{} pages laid out", state.page_count());
    let mut editor = Editor::new(document);

    // Select all, then bold: every paragraph is dirty.
    editor.select_all();
    let started = Instant::now();
    let change = editor.toggle_bold().expect("bold");
    println!(
        "select all + bold: editor {:.1} ms, {} dirty paragraphs",
        ms(started),
        change.dirty.len()
    );
    let started = Instant::now();
    let doc = editor.document();
    let items: Vec<(usize, ParagraphSnapshot)> = change
        .dirty
        .iter()
        .filter_map(|id| {
            let index = doc.index_of(*id)?;
            let p = doc.paragraph(*id)?;
            Some((index, ParagraphSnapshot::from_model(p, &doc.styles, None)))
        })
        .collect();
    let snapshots = ms(started);
    // What `DocView::sync` does for a text edit: one `replace_paragraph` per dirty paragraph.
    let mut per_paragraph = LayoutState::from_document(
        &Editor::new(libertum_layout::spike::generate_model_document(pages, 11))
            .document()
            .clone(),
        geometry,
        LayoutOptions::default(),
    );
    per_paragraph.paginate_all();
    let started = Instant::now();
    for (index, snapshot) in items.iter().cloned() {
        per_paragraph.replace_paragraph(index, snapshot);
    }
    println!(
        "  one replace_paragraph per dirty paragraph (the app's path): {:.1} ms, pages {}",
        ms(started),
        per_paragraph.page_count()
    );

    let mut deferred = LayoutState::from_document(
        &Editor::new(libertum_layout::spike::generate_model_document(pages, 11))
            .document()
            .clone(),
        geometry,
        LayoutOptions::default(),
    );
    deferred.paginate_all();
    let started = Instant::now();
    let needed = deferred.replace_paragraphs_deferred(items.clone());
    let swap = ms(started);
    let started = Instant::now();
    deferred.paginate_until(4);
    let first_screen = ms(started);
    let started = Instant::now();
    deferred.paginate_all();
    println!(
        "  deferred (needed={needed}): swap {:.1} ms, first screen {:.1} ms (blocks), rest {:.1} ms (background), pages {}",
        swap,
        first_screen,
        ms(started),
        deferred.page_count()
    );

    let started = Instant::now();
    let stats = state.replace_paragraphs(items);
    println!(
        "  snapshots {:.1} ms; replace_paragraphs {:.1} ms, pages now {}",
        snapshots,
        ms(started),
        stats.map_or(0, |s| s.total_pages)
    );
    state.paginate_all();

    // One early paragraph changes height: how far does repagination run?
    let first = doc_first_snapshot(&editor);
    let started = Instant::now();
    let mut taller = first.clone();
    taller.space_before_pt += 40.0;
    taller.version += 1;
    let stats = state.replace_paragraph(0, taller);
    println!(
        "space before +40 pt on paragraph 0: {:.1} ms, pages {}",
        ms(started),
        stats.map_or(0, |s| s.total_pages)
    );
}

fn doc_first_snapshot(editor: &Editor) -> ParagraphSnapshot {
    let doc = editor.document();
    let p = doc
        .paragraphs()
        .next()
        .expect("generated document has paragraphs");
    ParagraphSnapshot::from_model(p, &doc.styles, None)
}
