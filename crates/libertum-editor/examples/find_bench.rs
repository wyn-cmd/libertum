//! Times Find on a long generated document: the first search for a query, and stepping
//! through matches with the position count the Find bar shows.
//!
//! `cargo run --release -p libertum-editor --example find_bench`

use std::time::Instant;

use libertum_editor::{DocPosition, Editor, SearchOptions};

fn main() {
    let pages = std::env::var("PAGES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let doc = libertum_layout::spike::generate_model_document(pages, 7);
    let paragraphs = doc.paragraph_count();
    let words = doc.word_count();
    let editor = Editor::new(doc);
    let options = SearchOptions::default();
    let Some(first_id) = editor.document().first_id() else {
        return;
    };
    let start = DocPosition {
        block: first_id,
        offset: 0,
    };
    for query in ["dolor", "résumé", "zzzz"] {
        let begun = Instant::now();
        let first = editor.find(query, options, start, true);
        let (total, _) = editor.match_position(query, options, first.as_ref());
        let first_ms = begun.elapsed().as_secs_f64() * 1000.0;

        // Step through up to 200 matches the way the Find bar does: find, then count.
        let begun = Instant::now();
        let mut from = start;
        let mut steps = 0u32;
        for _ in 0..200 {
            let Some(found) = editor.find(query, options, from, true) else {
                break;
            };
            let _ = editor.match_position(query, options, Some(&found));
            from = DocPosition {
                block: found.block,
                offset: found.range.end,
            };
            steps += 1;
        }
        let step_ms = if steps == 0 {
            0.0
        } else {
            begun.elapsed().as_secs_f64() * 1000.0 / f64::from(steps)
        };
        println!(
            "pages={pages} paragraphs={paragraphs} words={words} query={query:?} matches={total} first_ms={first_ms:.2} per_step_ms={step_ms:.4}"
        );
    }
}
