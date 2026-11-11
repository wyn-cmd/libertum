//! Criterion benchmarks for editing (PLAN §2 and the Phase 1 acceptance criteria), on the
//! deterministic 1,000-page model document (`spike::generate_model_document(1000, 42)`).
//!
//! - `editing/type_char_mid_1000_pages`: one `Editor::insert_text` of a single character in
//!   the middle of a paragraph near the middle of the document. Only the insert is timed;
//!   it is undone outside the timed region so every iteration starts from the same state.
//! - `editing/type_1000_then_undo_1000`: 1,000 single-character inserts, each recorded as its
//!   own undo step (the clock passed to `insert_text_at` advances past the typing pause), then
//!   1,000 undos. The editor is built from a clone of the document outside the timed region.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use libertum_editor::{DocPosition, Editor};
use libertum_layout::spike::generate_model_document;
use libertum_model::{Document, TYPING_PAUSE};

const PAGES: usize = 1_000;
const SEED: u64 = 42;
const STEPS: usize = 1_000;

/// A caret in the middle of the first non-trivial paragraph at or after the document middle.
fn middle_caret(doc: &Document) -> Option<DocPosition> {
    (doc.len() / 2..doc.len()).find_map(|index| {
        let p = doc.paragraph_at(index)?;
        let text = p.text();
        if text.len() < 200 {
            return None;
        }
        let mut offset = text.len() / 2;
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        Some(DocPosition {
            block: p.id,
            offset,
        })
    })
}

fn bench_type_one_char(c: &mut Criterion, doc: &Document, caret: DocPosition) {
    let mut editor = Editor::new(doc.clone());
    let mut group = c.benchmark_group("editing");
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    group.bench_function("type_char_mid_1000_pages", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                editor.set_caret(caret, false);
                let start = Instant::now();
                let result = editor.insert_text("e");
                total += start.elapsed();
                assert!(result.is_ok(), "insert failed: {result:?}");
                black_box(result.ok());
                let undone = editor.undo();
                assert!(matches!(undone, Ok(Some(_))), "undo failed: {undone:?}");
            }
            total
        });
    });
    group.finish();
}

fn bench_type_then_undo(c: &mut Criterion, doc: &Document, caret: DocPosition) {
    let mut group = c.benchmark_group("editing");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(10));
    group.bench_function("type_1000_then_undo_1000", |b| {
        b.iter_batched_ref(
            || {
                let mut editor = Editor::new(doc.clone());
                editor.set_caret(caret, false);
                editor
            },
            |editor| {
                let t0 = Instant::now();
                let gap = TYPING_PAUSE * 2;
                for i in 0..STEPS {
                    // Letters only, so whitespace never splits or joins steps; the clock gap
                    // keeps every keystroke a separate undo step.
                    let ch = ["a", "b", "c", "d", "e"][i % 5];
                    let result = editor.insert_text_at(t0 + gap * (i as u32), ch);
                    assert!(result.is_ok(), "insert failed: {result:?}");
                }
                assert_eq!(editor.history().undo_len(), STEPS);
                for _ in 0..STEPS {
                    let undone = editor.undo();
                    assert!(matches!(undone, Ok(Some(_))), "undo failed: {undone:?}");
                }
                assert!(!editor.can_undo());
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn benches(c: &mut Criterion) {
    let doc = generate_model_document(PAGES, SEED);
    let Some(caret) = middle_caret(&doc) else {
        eprintln!("editing: no paragraph of 200+ bytes after the middle; skipped");
        return;
    };
    bench_type_one_char(c, &doc, caret);
    bench_type_then_undo(c, &doc, caret);
}

criterion_group! {
    name = editing;
    config = Criterion::default().without_plots();
    targets = benches
}
criterion_main!(editing);
