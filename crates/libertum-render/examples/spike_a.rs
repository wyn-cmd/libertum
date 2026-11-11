//! Spike A: lay out, paginate and rasterise a ~500-page generated document on the CPU and
//! time it against the PLAN §2 budgets. Run with:
//!
//!     cargo run --release -p libertum-render --example spike_a
//!
//! Writes PNGs of pages 1 and 250 to `<workspace>/target/spike_a/`.
// Examples are allowed to print.
#![allow(clippy::print_stdout)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use libertum_layout::spike::generate_document;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
use libertum_render::{DEFAULT_PAGE_CACHE_BYTES, PageRenderer};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn rss_mb() -> f64 {
    memory_stats::memory_stats().map_or(f64::NAN, |m| m.physical_mem as f64 / (1024.0 * 1024.0))
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).ceil() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn new_state(paras: Vec<libertum_layout::SpikeParagraph>) -> LayoutState {
    LayoutState::new(PageGeometry::a4(), paras, LayoutOptions::default())
}

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/spike_a")
        .components()
        .collect::<PathBuf>();
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        println!("warning: cannot create {}: {e}", out_dir.display());
    }

    let mem_start = rss_mb();
    let paras = generate_document(500, 42);
    let chars: usize = paras.iter().map(|p| p.text.len()).sum();
    let words: usize = paras
        .iter()
        .map(|p| p.text.split_whitespace().count())
        .sum();
    println!(
        "document: {} paragraphs, {} chars, {} words",
        paras.len(),
        chars,
        words
    );

    // ---- (a) full layout + pagination (cold: includes system font discovery) ----------------
    let t = Instant::now();
    let mut state = new_state(paras.clone());
    let t_init = t.elapsed();
    let report = state.font_report();
    let t = Instant::now();
    let st = state.paginate_all();
    let t_paginate = t.elapsed();
    let mem_laid_out = rss_mb();
    println!(
        "fonts: {} families, resolved = {:?}, fallback = {}",
        report.family_count, report.resolved_family, report.using_fallback
    );
    println!(
        "(a) full layout+pagination: {:.0} ms  [engine init incl. font discovery {:.0} ms + paginate {:.0} ms]  -> {} pages, {} parley layouts",
        ms(t_init + t_paginate),
        ms(t_init),
        ms(t_paginate),
        st.total_pages,
        state.engine_stats().layouts_built
    );

    // ---- (b) first page visible: fresh state, paginate to page 1, render page 1 -------------
    let t = Instant::now();
    let mut fresh = new_state(paras.clone());
    let t_fresh_init = t.elapsed();
    let t = Instant::now();
    let st1 = fresh.paginate_until(1);
    let t_first_layout = t.elapsed();
    let mut renderer = PageRenderer::default();
    let t = Instant::now();
    let first = renderer.render_page(&fresh, 0, 1.0);
    let t_first_render = t.elapsed();
    println!(
        "(b) first page visible: {:.0} ms  [init {:.0} + layout to page 1 {:.0} ({} paragraphs shaped) + render {:.0}]  ok={}",
        ms(t_fresh_init + t_first_layout + t_first_render),
        ms(t_fresh_init),
        ms(t_first_layout),
        st1.paragraphs_laid_out,
        ms(t_first_render),
        first.is_some()
    );
    drop(fresh);

    // ---- (c) render at 1.0 and 2.0, average over 20 pages -----------------------------------
    // Pages 100..120 (paragraph layouts for them may have been evicted from the LRU).
    let mut renderer = PageRenderer::default();
    let pages: Vec<usize> = (100..120).collect();
    let mut rows = Vec::new();
    for &scale in &[1.0f32, 2.0] {
        for pass in ["pass 1 (glyph cache cold for this scale)", "pass 2 (warm)"] {
            let t = Instant::now();
            for &p in &pages {
                let pm = renderer.render_page(&state, p, scale);
                std::hint::black_box(&pm);
            }
            let avg = ms(t.elapsed()) / pages.len() as f64;
            rows.push((scale, pass, avg));
        }
    }
    for (scale, pass, avg) in &rows {
        println!("(c) render page @ {scale:.1}x, {pass}: {avg:.2} ms/page");
    }
    let rs = renderer.stats();
    println!(
        "    glyph cache: {} entries, {:.1} MB, hit rate {:.1}%",
        rs.glyph_cache_entries,
        rs.glyph_cache_bytes as f64 / 1_048_576.0,
        100.0 * rs.glyph_hits as f64 / (rs.glyph_hits + rs.glyph_misses).max(1) as f64
    );

    // ---- PNGs for visual inspection (written before the keystroke loop edits the text) ------
    for (idx, label) in [(0usize, "page001"), (249usize, "page250")] {
        for scale in [1.0f32, 2.0] {
            if let Some(pm) = renderer.render_page(&state, idx, scale) {
                let path = out_dir.join(format!("{label}_{scale:.0}x.png"));
                match pm.save_png(&path) {
                    Ok(()) => println!("wrote {}", path.display()),
                    Err(e) => println!("PNG write failed for {}: {e}", path.display()),
                }
            }
        }
    }

    // ---- (d) simulated keystroke near page 250 ----------------------------------------------
    let target_page = 249.min(state.page_count().saturating_sub(1));
    let para_idx = state.pages()[target_page]
        .slices
        .first()
        .map_or(0, |s| s.para)
        + 1;
    let iterations = 200;
    let mut tot = Vec::new();
    let mut t_relayout = Vec::new();
    let mut t_render = Vec::new();
    let mut rebuilt = Vec::new();
    for i in 0..iterations {
        let start = Instant::now();
        let at = state.paragraphs()[para_idx].text.len() / 2 + (i % 7);
        let stats = state
            .edit_paragraph(para_idx, |p| p.insert_char(at, typed(i)))
            .unwrap_or_default();
        let after_layout = Instant::now();
        let page = state.page_of_para(para_idx);
        let pm = renderer.render_page(&state, page, 1.0);
        let end = Instant::now();
        std::hint::black_box(&pm);
        tot.push(ms(end - start));
        t_relayout.push(ms(after_layout - start));
        t_render.push(ms(end - after_layout));
        rebuilt.push(stats.pages_rebuilt as f64);
    }
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let sorted = |v: &[f64]| {
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        s
    };
    let st_tot = sorted(&tot);
    println!(
        "(d) keystroke (re-layout + incremental repagination + render page @1.0x): mean {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        mean(&tot),
        percentile(&st_tot, 0.95),
        st_tot.last().copied().unwrap_or(f64::NAN)
    );
    println!(
        "    of which layout+repaginate mean {:.2} ms (p95 {:.2}), render mean {:.2} ms (p95 {:.2}); pages rebuilt per key: mean {:.1}, max {:.0}",
        mean(&t_relayout),
        percentile(&sorted(&t_relayout), 0.95),
        mean(&t_render),
        percentile(&sorted(&t_render), 0.95),
        mean(&rebuilt),
        rebuilt.iter().copied().fold(0.0, f64::max)
    );
    // A keystroke that adds or removes a line shifts every later break by one line, so pages
    // keep re-breaking until some page has slack to absorb it ("ripple"). Report both cases.
    let (ripple, calm): (Vec<usize>, Vec<usize>) = (0..iterations).partition(|&i| rebuilt[i] > 3.0);
    let avg_of = |idx: &[usize], v: &[f64]| {
        if idx.is_empty() {
            f64::NAN
        } else {
            idx.iter().map(|&i| v[i]).sum::<f64>() / idx.len() as f64
        }
    };
    println!(
        "    keystrokes converging within 3 pages: {} (mean {:.2} ms); rippling > 3 pages: {} (mean {:.2} ms, pages rebuilt mean {:.0})",
        calm.len(),
        avg_of(&calm, &tot),
        ripple.len(),
        avg_of(&ripple, &tot),
        avg_of(&ripple, &rebuilt)
    );
    // Same at 2.0x (HiDPI).
    let mut tot2 = Vec::new();
    for i in 0..iterations {
        let start = Instant::now();
        let at = state.paragraphs()[para_idx].text.len() / 2 + (i % 7);
        let _ = state.edit_paragraph(para_idx, |p| p.insert_char(at, typed(i + 200)));
        let page = state.page_of_para(para_idx);
        let pm = renderer.render_page(&state, page, 2.0);
        std::hint::black_box(&pm);
        tot2.push(ms(start.elapsed()));
    }
    let st2 = sorted(&tot2);
    println!(
        "    same @2.0x: mean {:.2} ms, p95 {:.2} ms",
        mean(&tot2),
        percentile(&st2, 0.95)
    );

    // ---- (e) memory -------------------------------------------------------------------------
    // Fill the page cache the way scrolling would: 2.0x pages through the cache.
    let mut cached = PageRenderer::new(DEFAULT_PAGE_CACHE_BYTES);
    for p in 200..230 {
        let _ = cached.cached_page(&state, p, 2.0);
    }
    let cs = cached.stats();
    let mem_end = rss_mb();
    println!(
        "(e) working set: start {:.0} MB, after layout of {} pages {:.0} MB, after rendering+caching {:.0} MB (page cache {} pages = {:.0} MB of {:.0} MB cap)",
        mem_start,
        state.page_count(),
        mem_laid_out,
        mem_end,
        cs.page_cache_pages,
        cs.page_cache_bytes as f64 / 1_048_576.0,
        DEFAULT_PAGE_CACHE_BYTES as f64 / 1_048_576.0
    );
    let es = state.engine_stats();
    println!(
        "    layout cache: {} full layouts, {} metric tables",
        es.cached_layouts, es.cached_metrics
    );
}

/// Typing simulation: letters with a space every sixth key, so words break naturally.
fn typed(i: usize) -> char {
    if i % 6 == 5 {
        ' '
    } else {
        char::from(b'a' + (i % 26) as u8)
    }
}
