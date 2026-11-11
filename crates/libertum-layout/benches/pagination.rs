//! Criterion benchmarks for layout and pagination (PLAN §2), on the deterministic 1,000-page
//! spike corpus (`spike::generate_document(1000, 42)`).
//!
//! - `pagination/full_1000_pages`: `LayoutState::new` + `paginate_all` from scratch.
//! - `pagination/edit_mid_same_lines`: one character typed in the middle of a paragraph near
//!   the middle of a fully paginated document, where the paragraph keeps its line count
//!   (re-layout of one paragraph + incremental re-pagination that converges early).
//! - `pagination/edit_mid_adds_line`: as above, but the character pushes the paragraph onto
//!   one more line, so later page breaks ripple until a page absorbs the extra line.
//!
//! Only the edit is timed. Each edit is reverted outside the timed region so every iteration
//! starts from the same state.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use libertum_layout::spike::generate_document;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry, ParagraphSnapshot};

const PAGES: usize = 1_000;
const SEED: u64 = 42;
const TYPED: char = 'e';

fn line_count(state: &LayoutState, index: usize) -> Option<usize> {
    state.paragraph_layout(index).map(|l| l.lines.len())
}

/// Byte offset in the middle of the paragraph's text, on a char boundary.
fn mid_offset(text: &str) -> usize {
    let mut at = text.len() / 2;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Restores a paragraph's text and spans (the version is bumped by `edit_paragraph`).
fn restore(state: &mut LayoutState, index: usize, original: &ParagraphSnapshot) {
    state.edit_paragraph(index, |p| {
        p.text.clone_from(&original.text);
        p.spans.clone_from(&original.spans);
    });
}

/// Finds, starting at the middle paragraph and walking forward, a multi-line body paragraph
/// where inserting one character in the middle keeps (`adds_line == false`) or increases
/// (`adds_line == true`) its line count. Leaves the state unchanged.
fn find_paragraph(state: &mut LayoutState, adds_line: bool) -> Option<(usize, usize)> {
    let count = state.paragraph_count();
    for index in count / 2..count {
        let original = state.paragraphs().get(index)?.clone();
        if original.text.len() < 200 || original.forces_new_page() {
            continue;
        }
        let before = line_count(state, index)?;
        if before < 2 {
            continue;
        }
        let at = mid_offset(&original.text);
        state.edit_paragraph(index, |p| p.insert_char(at, TYPED));
        let after = line_count(state, index)?;
        restore(state, index, &original);
        let wanted = if adds_line { before + 1 } else { before };
        if after == wanted {
            return Some((index, at));
        }
    }
    None
}

fn paginated_state(paragraphs: Vec<ParagraphSnapshot>) -> LayoutState {
    let mut state = LayoutState::new(PageGeometry::a4(), paragraphs, LayoutOptions::default());
    state.paginate_all();
    state
}

fn bench_full_pagination(c: &mut Criterion, paragraphs: &[ParagraphSnapshot]) {
    let mut group = c.benchmark_group("pagination");
    group.sample_size(10);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(35));
    group.bench_function("full_1000_pages", |b| {
        b.iter_batched(
            || paragraphs.to_vec(),
            |paras| {
                let mut state =
                    LayoutState::new(PageGeometry::a4(), paras, LayoutOptions::default());
                black_box(state.paginate_all());
                // Returned so the drop happens outside the timed region.
                state
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn bench_incremental_edit(c: &mut Criterion, state: &mut LayoutState, name: &str, adds_line: bool) {
    let Some((index, at)) = find_paragraph(state, adds_line) else {
        eprintln!("pagination/{name}: no suitable paragraph found; skipped");
        return;
    };
    let Some(original) = state.paragraphs().get(index).cloned() else {
        return;
    };

    // One untimed probe, printed so the report shows how much work the edit does.
    if let Some(stats) = state.edit_paragraph(index, |p| p.insert_char(at, TYPED)) {
        eprintln!(
            "pagination/{name}: paragraph {index} on page {}, pages rebuilt {}, reused {}, \
             converged {}, total pages {}",
            state.page_of_para(index) + 1,
            stats.pages_rebuilt,
            stats.pages_reused,
            stats.converged,
            stats.total_pages,
        );
    }
    restore(state, index, &original);

    let mut group = c.benchmark_group("pagination");
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    group.bench_function(name, |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let start = Instant::now();
                let stats = state.edit_paragraph(index, |p| p.insert_char(at, TYPED));
                total += start.elapsed();
                black_box(stats);
                restore(state, index, &original);
            }
            total
        });
    });
    group.finish();
}

fn benches(c: &mut Criterion) {
    let paragraphs = generate_document(PAGES, SEED);
    bench_full_pagination(c, &paragraphs);

    let mut state = paginated_state(paragraphs);
    bench_incremental_edit(c, &mut state, "edit_mid_same_lines", false);
    bench_incremental_edit(c, &mut state, "edit_mid_adds_line", true);
}

criterion_group! {
    name = pagination;
    config = Criterion::default().without_plots();
    targets = benches
}
criterion_main!(pagination);
