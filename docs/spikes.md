# Phase 0 spike results

Sections are added per spike. This file currently holds **Spike A (rendering)**, **Spike C (parley features)**, **Spike D (GPU rendering)** and **Spike E (tab stops)**, written by the layout/render work. Spike B (IME) is owned by the app work and is not covered here.

## Spike E: tab stops on parley 0.12 (inline boxes)

**Question.** parley 0.12 has no tab stops, so `\t` is shaped as ordinary whitespace (measured: `a\tb` gets one 18.63 pt line with three positioned glyphs, i.e. the tab has a space-like advance and no stop). STATUS proposed one in-flow inline box per tab, whose width is solved after a first layout pass. Is that workable, for which tab kinds, and at what cost?

**Method.** `crates/libertum-layout/examples/spike_tabs.rs`, release build, A4 content width 451.276 pt, Cambria 11 pt, default stops every 36 pt (0.5 in). For each sample:
1. remove the tab characters from the text given to parley, recording each tab's byte offset;
2. put one `InlineBoxKind::InFlow` box at each offset, all widths 0;
3. break the paragraph, read each box's `x` and width from the line items, and compute the width that puts the following text on the next stop:
   - left: `stop - x`;
   - right: `stop - x - width(text after the tab)`;
   - centre: `stop - x - width(text after the tab) / 2`;
   - decimal: `stop - x - width(text before the '.')`;
4. rebuild with the new widths and repeat until every width changes by less than 0.01 pt (cap 5 passes).

The text after a tab is measured with `Cursor::from_byte_index(..).geometry(..)` minus the box's own width, i.e. from the paragraph's own layout, not a separate measurement pass. A tab whose following text wrapped to another line cannot be measured this way and was counted as a failure.

**Results.**

| Sample | Tabs | Lines | Passes | Worst alignment error | Verdict |
|---|---|---|---|---|---|
| left, one line ("Item\tWidgets\t42\tEach") | 3 | 1 | 4 | 0.000 pt | pass |
| left, wrapped (9 columns) | 8 | 2 | 5 (cap) | 25.979 pt | **fails to converge** |
| right, explicit stop at 144 pt | 1 | 1 | 2 | 0.000 pt | pass |
| centre, explicit stop at 216 pt | 1 | 1 | 2 | 0.000 pt | pass |
| decimal, explicit stop at 288 pt | 1 | 1 | 2 | 0.000 pt | pass |
| tab at paragraph start and end | 2 | 1 | 2 | 0.000 pt | pass |

**Why the wrapped case oscillates.** The last tab of the long row ends up at the right margin with its text wrapping to the next line. Its width is then recomputed from the `x` it still has on the *first* line, which pushes the following text around, which changes whether the tab's text wraps. The pass trace alternates between two states (widths 32.93 and 6.95 pt for the last tab). Word resolves this by treating a tab that would pass the right margin as a line break; the fixed-point iteration above has no such rule. A correct implementation needs a line-aware rule: if the next stop lies past the right margin, the tab must move to the next line (or be dropped and the line break forced), and the stop is then the first stop of the new line. This also fixes the ordinary case of a tab-padded line whose last cell wraps.

**Cost** (release, this machine, single run):

- 1,000 paragraphs with three tabs, 4 passes each: 69.5 µs per paragraph.
- 5,000 paragraphs with three tabs: 588 ms, against 137 ms for the same text laid out once with zero-width boxes — **4.3×** the single-pass cost.
- The cost grows with passes: left tabs fix about one tab per pass, so a paragraph with `k` tabs takes up to `k + 1` passes; right/centre/decimal converge in 2 passes.

Only paragraphs containing a tab need this, so typing stays cheap (one paragraph, ~70 µs on top of a 0.3 ms layout), but full pagination of a table-heavy document (every paragraph with tabs) would pay the 4×. The two levers are: compute all left-tab widths in one pass by projecting `x` forward across the tabs already solved on the same line (the trace shows each pass fixes exactly one tab), and measure right/centre/decimal segment widths once in a separate mini-layout (independent of the box widths), which removes their second pass.

**Other findings.**

- In-flow boxes take part in line breaking: a box that does not fit moves to the next line, and text wraps after it. That is the behaviour tabs need.
- An inline box occupies no character. The spike first removed the tab characters from parley's text and would have needed an offset map for cursor movement, selection and hit testing; the implementation instead keeps the tab in the text at a 0.01 pt glyph, which measures the same in practice and needs no map.
- A trailing tab (paragraph ends with `\t`) gets a width but no following text; it adds trailing advance. Word ignores a trailing tab; the box can be skipped when it is the last item.
- `Cursor::from_byte_index` at the paragraph end is a valid way to measure the text after the last tab on a line; it reports the position after the box's width, so the box width must be subtracted (the first version of the spike omitted that and oscillated).

**Verdict.** The inline-box route works. Left tabs on one line align exactly; right, centre and decimal work when the tab's text stays on its line; wrapping tabs need an explicit "past the margin means a line break" rule and a one-pass forward solver for left tabs.

**What the implementation changed (2026-11-10, after this spike).** `libertum-layout` now keeps the tab character in parley's text and styles its own glyph at 0.01 pt with a transparent brush, so it contributes no visible advance while byte offsets, cursor movement, hit testing, selection and hyphenation stay unchanged. The advance comes from the in-flow box at the tab's offset. That removes the offset map this spike expected to need. The width solver still iterates (2 to 4 passes in practice, cap 6); a stop past the right margin is ignored (width 0) rather than moving the text to the next line, which keeps the pass stable. See `docs/decisions.md`, 2026-11-10, "Tabs".


## Machine and method

| Item | Value |
|---|---|
| CPU | AMD Ryzen 5 5500U (6 cores / 12 threads, mobile Zen 2) |
| RAM | 15.3 GB |
| GPU | AMD Radeon integrated graphics (not used: this spike is CPU-only) |
| OS | Windows 11 Home 10.0.26200 |
| Toolchain | Rust 1.99.1 (MSVC), release profile (`lto = "thin"`, `codegen-units = 1`) |
| Crates | parley 0.12.0, swash 0.2.10, tiny-skia 0.12.0, memory-stats 1.2.0 |
| Font | Cambria (found on this machine; 235 families discovered; fallback chain Georgia, Times New Roman, Liberation Serif, DejaVu Serif, Noto Serif, then generic serif/sans-serif) |
| Test document | 506 A4 pages, 5,010 paragraphs, 1.95 M characters, 288 k words. 11 pt body, 16/22 pt bold headings every 14 paragraphs, random bold, italic, 14 pt and red spans, accents and curly quotes, a few empty paragraphs. Deterministic (`libertum_layout::spike::generate_document(500, 42)`) |
| Command | `cargo run --release -p libertum-render --example spike_a` |

**Caveat: this is not the min-spec machine.** PLAN §2 budgets are defined for a roughly 2015 dual-core laptop with 4 GB RAM. This machine has 6 fast cores, 15 GB RAM and a modern-ish mobile CPU, and the spike is single-threaded, so single-core speed is what matters. My estimate (not measured) is that a 2015 dual-core runs this code 3 to 5 times slower. The "min-spec estimate" column applies that factor and must be confirmed on real hardware or a throttled VM before Phase 0 is signed off. Numbers below come from several runs; run-to-run spread was about 10%, with occasional single-sample spikes to 15 ms from the OS.

## Spike A: results against PLAN §2 budgets

| Metric | Budget | Measured here | Min-spec estimate (3 to 5x) | Verdict |
|---|---|---|---|---|
| Keystroke to pixels, typing in the middle of a 500-page doc (p95) | < 16 ms | **2.4 to 3.3 ms** at 1.0x (re-layout 0.3 + repaginate + render page 2.2); **6.7 to 8.6 ms** at 2.0x | 8 to 16 ms at 1.0x; 20 to 40 ms at 2.0x | Pass here. At risk on min-spec at 2.0x. See notes 1 and 2 |
| Open: first page visible | < 1.5 s | **14 to 18 ms** (font init 4, shape 11 paragraphs 7, render 5) | 50 to 90 ms | Pass. Excludes DOCX parsing |
| Open: full pagination (background) | < 8 s | **1.2 to 1.5 s** (506 pages, 5,010 paragraphs shaped, single thread) | 4 to 7 s | Pass here, thin margin on min-spec; runs off the UI thread |
| Scrolling at 60 fps, no frame > 33 ms | 60 fps | Raster cost per new page: **2.1 ms** at 1.0x, **6.2 to 6.9 ms** at 2.0x (warm glyph cache); **5 ms** at 1.0x when the paragraph layouts also have to be rebuilt | 10 to 35 ms for a 2.0x page on a cold layout | Partial. Raster cost fits the frame budget here, but scrolling inside Iced was not measured (app work) |
| Resident memory, 500-page text-only doc | < 300 MB | **82 MB** working set after laying out 506 pages; **152 MB** with the 64 MB page cache full (8 pages at 2.0x) | similar (memory does not scale with CPU) | Pass. Excludes Iced/winit/GPU overhead |
| Idle CPU | ~0% | Not measured: no event loop in this spike | n/a | Not applicable here |
| Save of 500-page DOCX | < 2 s | Not applicable (no file I/O in this spike) | n/a | Not applicable here |

Detailed numbers from the final run:

- (a) full layout and pagination: 1,298 ms (font discovery 7 ms, pagination 1,291 ms), 506 pages, 5,010 parley layouts built (about 0.26 ms per paragraph).
- (b) first page: 16 ms in total.
- (c) render a page, average over pages 100 to 119: 1.0x gives 4.99 ms on the first pass (includes rebuilding evicted paragraph layouts and warming glyphs) and 2.12 ms warm. 2.0x gives 6.92 ms first pass and 6.19 ms warm. Glyph cache: 1,377 entries, 0.2 MB, 99.5% hit rate.
- (d) simulated keystroke at a paragraph near page 250, 200 iterations, each inserting one character (letters with a space every sixth key) then re-laying out that paragraph, re-paginating incrementally, and re-rendering its page at 1.0x: mean 2.10 ms, p95 2.44 ms, max 2.97 ms. Layout plus repagination is 0.21 ms of that (p95 0.31 ms); rasterising the page is 1.89 ms. At 2.0x: mean 5.94 ms, p95 6.72 ms.
- (e) working set: 5 MB at start, 82 MB after laying out 506 pages (3,235 full parley layouts cached plus line-height tables for all 5,010 paragraphs), 152 MB after filling the page cache.

PNGs for inspection: `D:\Libertum\target\spike_a\page001_1x.png`, `page001_2x.png`, `page250_1x.png`, `page250_2x.png`. Text renders legibly with bold, italic, size changes, colour spans and headings; accented characters and curly quotes are correct.

### Notes on the numbers

1. **Not yet counted in the keystroke figure:** Iced's texture upload of the page bitmap, window compositing, and caret/selection overlay drawing. These are app-side and need measuring in the Iced canvas widget. A 2.0x page is a 1191 x 1684 bitmap (8 MB); uploading it on every keystroke will cost more than rasterising it on a weak iGPU.
2. **The render step dominates the keystroke path (1.9 of 2.1 ms), and it re-rasterises the whole page.** PLAN §4.4 already says to re-rasterise only dirty line rectangles while typing. That is not implemented in this spike. It is the main lever if min-spec misses the 16 ms budget, because the layout and pagination share is only about 0.2 ms.
3. **Pagination ripple.** Inserting a character that does not change the paragraph's line count converges in 1 to 2 pages (199 of 200 keystrokes in the benchmark, mean 2.1 ms). A keystroke that adds or removes a line shifts every later page break by one line; breaks only realign when some page has slack to absorb it. In the benchmark one such keystroke re-broke 187 pages (the walk uses cached line heights only, so it cost 2.6 ms in total, but every one of those pages gets a new content hash and will need re-rasterising if it is visible). Correctness is covered by tests that compare the incremental result with a from-scratch pagination. This is inherent to flowing text, not a bug, but PLAN §4.2's "typing re-paginates one or two pages" is true only when line counts stay the same.
4. **Memory design.** Full parley layouts hold shaped glyph data and are the heavy part. They sit in an LRU capped at 4,000 entries (`LayoutOptions::max_cached_layouts`). A compact per-paragraph line-height table (a `Vec<f32>`) is kept for all paragraphs so pagination never needs the full layout. Evicted layouts are rebuilt on demand when a page is rendered (about 0.26 ms each).
5. The pagination spike is single-threaded and synchronous. `LayoutState` is `Send + Sync` (checked by a test), so it can move behind a worker, but the background-pagination protocol (PLAN §4.2, Arc snapshots and chunked results) is not built.

### What was built

`libertum-layout` provides `PageGeometry`, spike paragraph types, parley paragraph layout, the layout cache, line-level pagination with incremental re-pagination and convergence, hit-testing and caret geometry. `libertum-render` provides the page rasteriser, glyph bitmap cache, page bitmap cache (64 MB default) and the `spike_a` example. Both crates pass `cargo fmt`, `cargo clippy -p libertum-layout -p libertum-render --all-targets -- -D warnings` and `cargo test -p libertum-layout -p libertum-render` (17 tests).

### Not done in Spike A

- Pagination does no widow/orphan control, keep-with-next, headers, footnotes, columns or tables (the brief asked for simple line-level breaking).
- Only outline glyphs are drawn. Colour emoji (bitmap or colour-outline glyphs), underline and strikethrough are not drawn. Inline boxes are laid out by parley but not drawn.
- Dirty-line-rectangle re-rasterisation, and Spike A's Iced display (scroll and zoom inside a canvas widget), are not part of this work.
- Paragraph insertion and deletion are not supported by the incremental path (only edits inside one paragraph); the convergence check relies on paragraph indices staying stable.
- No justified alignment, tabs or RTL testing.

## Spike C: parley 0.12 features

Verified by reading the parley 0.12.0 source and by tests in `libertum-layout`. Verdict: **all three required capabilities exist; no blocker.**

**Inline boxes (images, footnote markers): yes.** `RangedBuilder::push_inline_box(InlineBox { id: u64, kind: InlineBoxKind, index: usize, width: f32, height: f32, baseline: Option<f32>, vertical_align: VerticalAlign })`. `kind` is `InlineBoxKind::InFlow` (takes space, flows with text), `OutOfFlow`, or `CustomOutOfFlow` (control yields to the caller). After line breaking, `Line::items()` yields `PositionedLayoutItem::InlineBox(PositionedInlineBox { x, y, width, height, baseline, id, kind })` interleaved with `PositionedLayoutItem::GlyphRun(..)`. `Layout::inline_boxes()` lists them in text order. A test (`parley_inline_boxes_are_positioned`) confirms a 30 x 20 box is placed mid-line with its id and size. Notes: `index` is a byte offset and the box occupies no text, so the cursor model has no position "on" the box by itself. My recommendation (opinion, untested) is to put a U+FFFC placeholder character in the paragraph text at the box's offset so cursor movement, selection and hit-testing treat it as one character. `TreeBuilder` and `StyleRunBuilder` take boxes in the same way.

**Per-span styles: yes.** `LayoutContext::ranged_builder(&mut FontContext, text, scale, quantize)` returns a `RangedBuilder`; `push_default(StyleProperty)` sets paragraph-wide defaults and `push(StyleProperty, Range<usize>)` sets a span. Used here for `StyleProperty::FontSize`, `FontWeight(FontWeight::BOLD)`, `FontStyle(FontStyle::Italic)`, `Brush(B)`, `FontFamily(FontFamily)`, `LineHeight(LineHeight::FontSizeRelative(1.2))` and `OverflowWrap(OverflowWrap::Anywhere)`. Also available: `Underline`, `Strikethrough` (with offset, size and brush), `LetterSpacing`, `WordSpacing`, `FontVariations`, `FontFeatures`, `Locale`, `VerticalAlign` (with `VerticalAlign::SUPER` and `SUB` constants), `WordBreak`, `LineBreak`, `TextWrapMode`, `WhiteSpaceCollapse`. The brush is a generic `B: Brush` (here `libertum_layout::Brush([u8; 4])`) and comes back per glyph run via `GlyphRun::style().brush`. For documents with many runs, `StyleRunBuilder` (shared style table plus indexed runs) skips parley's range-splitting and may be faster; not benchmarked.

**Hit-testing and cursor geometry: yes**, in `parley::editing`:
- Point to cursor: `Cursor::from_point(&Layout, x, y) -> Cursor`, and `Selection::from_point`. Lower level: `Cluster::from_point(&Layout, x, y) -> Option<(Cluster, ClusterSide)>`.
- Offset to cursor: `Cursor::from_byte_index(&Layout, index, Affinity) -> Cursor`; read back with `Cursor::index()` and `Cursor::affinity()`.
- Cursor to rectangle: `Cursor::geometry(&Layout, width: f32) -> BoundingBox { x0, y0, x1, y1: f64 }`.
- Selection rectangles: `Selection::geometry(&Layout) -> Vec<(BoundingBox, usize /* line index */)>`, or `geometry_with` with a callback (no allocation).
- Movement: `Cursor::{next_visual, previous_visual, next_visual_word, previous_visual_word, next_logical_word, previous_logical_word}`; `Selection::{next_visual, previous_visual, next_line, previous_line, move_lines, line_start, line_end, hard_line_start, hard_line_end, extend_to_point, shift_click_extension, word_from_point, line_from_point}`.
- Both crates' coordinates are layout-space f32/f64 in the units given to the builder (points here, scale 1.0).
- A test (`hit_test_and_caret_round_trip`) maps offset to caret rectangle to page point and back to the same offset. `LayoutState::hit_test` and `LayoutState::caret_rect` wrap these with page and margin offsets.

**Bonus for Phase 1 and Spike B:** parley ships an editor, `parley::editing::PlainEditor<T>` with `PlainEditorDriver`, including IME composition support (`set_compose`, `set_compose_byte_range`, `clear_compose`, `finish_compose`, `raw_compose`). It is plain-text only and owns one layout, so it does not fit Libertum's per-paragraph rich model directly, but its composition handling is a useful reference for how to render preedit text.

**Other Spike C observations**
- parley 0.12 splits into `parley`, `parley_engine`, `parlance` (style value types) and `linebender_resource_handle` (`FontData`, `Blob`). The re-export `parley::fontique` means no separate fontique dependency is needed.
- `FontData { data: Blob<u8>, index: u32 }`; `Blob::id()` is a stable u64, which makes a good glyph cache key.
- Line breaking is incremental: `Layout::break_lines()` returns a `BreakLines` with `break_next()` and `revert_to(BreakerState)`, which could allow breaking a paragraph one line at a time (for example to flow around a page boundary). Not used in this spike.
- Parley does not emergency-break long unbreakable words by default (`OverflowWrap::Normal`). The spike sets `OverflowWrap::Anywhere` as a default, otherwise a 600-character word overflows the margin.
- System font discovery is fast: `FontContext::new()` took 4 to 8 ms for 235 families (fonts are loaded lazily).

## Spike D: GPU rendering path (vello 0.7)

**Verdict: works.** vello renders correct, layout-identical pages from the same parley glyph runs on this machine's iGPU. One vello bug needed a workaround (below). Iced integration is feasible through the `shader` widget on the shared wgpu device, but was investigated by reading source only; it is not built or run.

**Versions resolved (Cargo.lock, whole workspace including libertum-app and iced):** one `wgpu` 27.0.1 (wgpu-core 27.0.3, wgpu-hal 27.0.4), shared by `iced_wgpu` 0.14.0, `cryoglyph` and `vello` 0.7.0; one `peniko` 0.6.1 (vello, vello_encoding). `cargo tree -i wgpu` and `-i peniko` show no duplicates, so wgpu `Device`, `Queue` and `Texture` types are interchangeable between Iced and vello, and parley's `FontData` goes straight into `Scene::draw_glyphs`. Two `skrifa` versions (0.40 in vello, 0.44 in parley) and two `kurbo` versions coexist; they never meet in a public type (normalised coordinates are `i16` on both sides).

**Adapter:** AMD Radeon(TM) Graphics (Ryzen 5 5500U iGPU, vendor 0x1002, device 0x164c), backend Vulkan, integrated, driver AMD proprietary 22.20.16. Adapter init 1.2 to 2.6 s; `vello::Renderer::new` (shader compile, area AA only) 0.17 to 1.4 s.

**What was built (`libertum-render`, feature `gpu`, off by default):** the `PageBackend` trait and `PageOutput` enum (`Bitmap(Arc<Pixmap>)` or `Texture(Arc<wgpu::Texture>)`), implemented for the existing `PageRenderer` (CPU API unchanged) and for `gpu::VelloPageRenderer`. `VelloPageRenderer` owns cloned `wgpu::Device` and `Queue`, builds a point-unit `vello::Scene` per page (white rectangle, then `draw_glyphs` per parley run with font, size, normalised coords, brush colour and glyph positions; synthetic skew as `glyph_transform`), caches scenes by `(page, content hash)` (64 pages), and renders at any scale by appending the cached scene under `Affine::scale` into a root scene and calling `Renderer::render_to_texture` into an `Rgba8Unorm` texture with `STORAGE_BINDING | TEXTURE_BINDING | COPY_SRC`. `read_back` and `render_to_rgba` give tight RGBA for tests. `HeadlessGpu` creates a headless device (`iced_like()` uses exactly Iced's compositor limits). Example: `cargo run --release -p libertum-render --features gpu --example spike_d`. Tests: `tests/gpu_smoke.rs` (skip when there is no adapter).

**vello 0.7 bug and workaround.** Cambria (the default serif here) has embedded monochrome bitmap strikes (EBLC/EBDT). vello's `draw_glyphs` uses a strike when one exists for the size and draws it as a white image that ignores the brush, so most body text was invisible on the first run (only glyphs without a bitmap, such as q, f, h and accents, showed). Workaround in `gpu.rs`: pass vello a cached copy of the font with the `EBLC` table tag renamed to `EBLB` (the table directory stays sorted), so it uses the outlines. Cost: one font-file copy per font, once. Colour bitmap fonts (CBLC, sbix) and COLR are untouched. Worth reporting upstream.

**CPU vs GPU diff** (pages 1 and 250 of `generate_document(500, 42)`, A4). Layout matches exactly (same lines, breaks and positions; checked by viewing the PNGs).

| Page | Scale | Mean abs diff (RGB, 0 to 255) | Pixels differing by more than 32 | Ink pixels CPU / GPU |
|---|---|---|---|---|
| 1 | 1.0x | 5.85 | 7.6% | 53,929 / 56,131 |
| 1 | 2.0x | 3.79 | 4.9% | 186,460 / 185,479 |
| 250 | 1.0x | 5.55 | 7.3% | 51,105 / 53,655 |
| 250 | 2.0x | 3.82 | 5.0% | 177,106 / 176,351 |

The differences are anti-aliasing. The CPU path snaps each baseline to a whole pixel and uses 4 horizontal subpixel positions with swash coverage; vello uses exact positions and its own area coverage. Ink coverage agrees within 5% at 1.0x and under 1% at 2.0x. The test asserts mean diff under 10 and ink within 15%. PNGs: `D:\Libertum\target\spike_d\page001_1x_cpu.png` and `_gpu.png`, likewise 2x and `page250_*`.

**Timings** (release build, average over pages 101 to 120, adapter above). The machine was shared with another build, so figures vary by about 30% between runs; three runs are shown as a range.

| Step | 1.0x | 2.0x |
|---|---|---|
| Scene build (parley runs to vello encoding), includes rebuilding evicted paragraph layouts on the first pass | 3.0 to 5.1 ms | 0.4 to 0.5 ms (layouts warm) |
| GPU render (append scene, submit, wait for completion; includes allocating the target texture) | 6.9 to 9.8 ms | 9.4 to 12.1 ms |
| Readback to CPU (copy, map, unpad); tests only | 5.5 to 9.7 ms | 8.6 to 10.9 ms |
| CPU raster (warm glyph cache), for comparison | 2.4 to 3.7 ms | 5.8 to 7.9 ms |

Reading: on this iGPU, vello is not faster than the CPU path per page at 1.0x and is about equal at 2.0x. The GPU figures include a blocking wait per page and a fresh texture per render, so they overstate steady-state cost, but there is no raster speed-up on min-spec hardware. The benefit is that a cached scene re-renders at any zoom with no CPU raster and no bitmap upload (the CPU path uploads 8 MB per 2.0x page), and the output is already a texture. Readback is a test-only cost.

**Integration with Iced 0.14** (from reading `iced_wgpu` 0.14.0 and `iced_widget` 0.14.2 source; not built):

1. Use `iced::widget::shader` (`Shader`, `Program`; re-exports `iced_wgpu::primitive::{Primitive, Pipeline, Storage}`). It exists only with the wgpu renderer, which matches PLAN §4.5 (GPU mode means Iced's wgpu renderer).
2. `Pipeline::new(device, queue, format)` runs once per pipeline type on first use and receives Iced's own `wgpu::Device` and `Queue`. Create the `VelloPageRenderer` there with `device.clone()` and `queue.clone()` (wgpu 27 handles are ref-counted, and the versions match). It cannot be created earlier because the app does not own Iced's device.
3. `Primitive::prepare(&self, pipeline: &mut Pipeline, device, queue, bounds, viewport)` runs every frame the widget is drawn. Render the page texture there only when `(page, content hash, scale)` changed, otherwise reuse the pipeline's texture. Primitives are `Send + Sync + 'static`, so carry an `Arc` snapshot (or a prebuilt scene) plus page index, hash and scale, not a borrow of `LayoutState`.
4. `Primitive::draw(&self, pipeline, render_pass) -> bool` draws into Iced's pass with viewport and scissor already set to the widget bounds and clip. Implement a small textured-quad pipeline (own WGSL, about 40 lines) that samples the vello texture. vello cannot render straight into the surface (compute pipeline, storage texture), so an intermediate texture plus blit is required, as vello's own docs say. `Primitive::render(encoder, target, clip_bounds)` is the alternative when a separate pass is needed.
5. Colour: Iced picks an sRGB surface format (`iced_graphics::color::GAMMA_CORRECTION` is true unless the `web-colors` feature is on). vello writes sRGB-encoded values into an `Rgba8Unorm` texture. Sampling it raw and writing to an sRGB target would encode twice (washed-out pages). `create_target` therefore lists `Rgba8UnormSrgb` in `view_formats`; create the sampled view with that format so the sampler decodes and the surface re-encodes, which is an identity round trip. Not verified on screen; check this first. The blit pipeline must target the `format` passed to `Pipeline::new`.
6. Device limits: Iced requests no features and default limits with `max_bind_groups: 2` and `max_non_sampler_bindings: 2048`. The test `vello_runs_on_iced_style_device` confirms `vello::Renderer::new` and a render succeed on a device with exactly those limits on this GPU. vello's optional features (`CLEAR_TEXTURE`, `PIPELINE_CACHE`) are absent on Iced's device, which vello tolerates.
7. Many pages in a scroll view: one widget covering the viewport that draws all visible pages in one primitive is cheaper than one widget per page, and fits PLAN §4.4 (visible plus one above and below). Keep a small texture pool in the pipeline (8 pages at 2.0x is about 64 MB, the same budget as the CPU cache).
8. Cursor, selection and IME overlays stay as ordinary Iced widgets or canvas layers above the shader widget (§4.4).
9. Fallback matches PLAN §4.5: if `Renderer::new` or the first render errors, switch to CPU mode. Write the "GPU start in progress" flag before the first `Pipeline::new`.

**Alternative:** render offscreen and read back into an `iced::widget::image::Handle`. It works with any Iced renderer but costs a readback (5 to 11 ms measured) plus an upload per page, which removes the point of GPU mode. Use it only as a diagnostic.

**Risks**
- vello 0.7 is several releases behind. Moving to vello 0.11 needs wgpu 30, while Iced 0.14 uses 27, so upgrading waits for Iced.
- No synthetic embolden in vello (a font lacking a bold face renders at regular weight; the CPU path emboldens). Underline, strikethrough, inline boxes and colour emoji are not drawn by either renderer yet; COLR and colour-bitmap glyphs go through vello's own path and are untested here.
- First `Renderer::new` costs 0.2 to 1.4 s (shader compile; `PIPELINE_CACHE` is unavailable on Iced's device). Do it off the first frame or accept a one-off hitch.
- Tested only on one AMD Vulkan driver (22.20.16, 2022 era). vello needs compute shaders and storage textures, so GL-only or downlevel adapters will fail `Renderer::new`; the automatic CPU fallback covers that.
- GPU baselines are not snapped to whole pixels, so GPU text can look slightly softer than the CPU path at 1.0x.

### Not done in Spike D
- The Iced `shader` widget and blit pipeline are not implemented (the app lives in its own crate), and the sRGB round trip is unverified on screen.
- No dirty-region or incremental GPU update (the whole page is re-rendered).
- Scene memory was not measured; scenes are capped by count (64 pages), not bytes.
- No test for scale factors other than 1.0 and 2.0, or for pages with images or colour emoji.

## Spike A in-app: scroll frame times

Setup: release build, 1200 x 680 window at 125% scaling, `spike::generate_document(500, 42)` (506 pages), zoom 100%, `LIBERTUM_PERF=1 LIBERTUM_AUTOSCROLL=1` scrolling page 1 to the last page at 4,000 logical px/s. Frame interval measured between consecutive `draw` calls while scrolling. Ryzen 5 5500U, one run per backend.

| Backend | Frames | Mean | p95 | p99 | Max | > 33 ms |
|---|---|---|---|---|---|---|
| wgpu | 8,717 | 16.65 ms | 17.86 ms | 19.01 ms | 60.85 ms | 7 |
| tiny-skia | 2,252 | 64.49 ms | 112.06 ms | 120.54 ms | 135.20 ms | 2,004 |

wgpu holds 60 fps (vsync-bound) with 7 hitches over 33 ms out of 8,717 frames (0.08%); the 6 frames with a blank page and the 60 ms maximum are most likely the first-screenful raster and texture uploads, not confirmed. Not a clean pass of "no frame over 33 ms", but close.

tiny-skia fails the budget: about 15 fps, 89% of frames over 33 ms. The app's own cost is negligible (draw 0.4 ms mean, update 0.01 ms). With page bitmaps skipped the same backend still took about 25 ms per frame, so the floor is Iced's CPU compositor (full-window damage, background fill, blit, present), not Libertum code. Worker page raster: mean 8.4 ms, p95 about 10 ms, 506 pages, never a stale-scale page.

Implication: CPU mode needs either a cheaper compositor path (smaller damage, scroll by blitting a retained surface) or to remain a fallback with lower expectations; the vello GPU path (Spike D) targets this. Estimate, not measured: a window smaller than 1200 x 680 would scale the tiny-skia cost down roughly with area.


## GPU platform support (vello 0.7 on wgpu 27.0.1)

Sources: `~/.cargo/registry/src/*/` crates `wgpu-27.0.1`, `wgpu-core-27.0.3`, `wgpu-hal-27.0.4`, `wgpu-types-27.0.1`, `vello-0.7.0`, `vello_shaders-0.7.0`, `iced_wgpu-0.14.0`. "Fact" means read in that source (file and line given); "measured" means run on this machine; "estimate" and "opinion" are labelled. Code: `crates/libertum-render/src/gpu.rs` and `src/gpu/caps.rs`.

### Gating rules (implemented)

`VelloPageRenderer::new`, `new_with` and `init_in_background` never panic. They return `Result<_, GpuUnavailable { reason }>`. Steps, in order:

1. **Capability gate** (`check_caps`, a pure function over `GpuCaps`, unit-tested without a GPU; every problem found is listed in `reason`).
   - Backend must be Vulkan, DX12 or Metal. OpenGL is refused by default (see below; opt in with `GatePolicy::allow_experimental_backends`). Noop and BrowserWebGpu are always refused.
   - Device type `Cpu` (WARP, llvmpipe, SwiftShader) is refused, because a software rasteriser is slower than the CPU page renderer (opinion; `allow_software` overrides).
   - Downlevel flags must include `COMPUTE_SHADERS` and `INDIRECT_EXECUTION`, and the shader model must be at least SM5. Fact: vello 0.7 is all compute shaders and uses `Command::DispatchIndirect` (`vello-0.7.0/src/recording.rs:84,184`). Full `is_webgpu_compliant()` is not required; it is reported in `GpuReport` only (GL lacks `VIEW_FORMATS`, which vello does not need).
   - `Rgba8Unorm` must be a write-only storage format.
   - Limits (fact, from the bind layouts in `vello-0.7.0/src/shaders.rs:102-215` and the WGSL): storage buffers per stage at least 8 (`coarse` binds 5 read-only and 3 read-write), storage textures 1, sampled textures 2, uniform buffers 1 (`fine_area`), invocations per workgroup and workgroup size x 256 (16 of 20 shaders use `@workgroup_size(256)`), workgroup size y 16 (`fine` is 4 x 16), workgroup storage 16,384 bytes (`coarse.wgsl` declares exactly that), workgroups per dimension 65,535, 2D texture 4,096. Estimate, not read from vello: storage binding size 128 MiB.
   - Iced's second device attempt uses `Limits::downlevel_defaults()` (`iced_wgpu` `compositor.rs:153`), which allows 4 storage buffers per stage, so vello is correctly refused on such a device.
   - Inside Iced's `Pipeline::new` only a `wgpu::Device` is available. `GpuCaps::from_device` reads limits and features from it and recovers the backend through `Device::as_hal` (a test checks it equals the adapter's backend). Downlevel flags and device type are unknown there and are not held against the GPU; pass `GpuCaps::from_adapter` when the adapter is available.
2. **sRGB view probe**: creates a 1 x 1 page texture that lists `Rgba8UnormSrgb` as a view format, inside an error scope. If that fails (downlevel `VIEW_FORMATS` missing, as on GL), page textures are created without it and `supports_srgb_view()` is false.
3. **`vello::Renderer::new`** with area anti-aliasing only, inside Validation, OutOfMemory and Internal error scopes plus `catch_unwind`.
4. **Smoke render**: a 64 x 64 scene (left half red, right half white base) is rendered and read back, and two pixels are checked, so a driver that accepts the shaders but draws garbage is also refused.
5. Every later `render_into` also runs inside the three scopes and returns `GpuError::Renderer` instead of aborting.

Uncaptured errors. Fact: wgpu 27's default handler is `panic!` (`wgpu-27.0.1/src/backend/wgpu_core.rs:689-693`; this is what killed spike D on GL). `Device::on_uncaptured_error(Arc<dyn Fn(Error)>)` replaces it for the whole device (`api/device.rs:410`). Iced owns the device in the app and a handler would change behaviour for Iced too, so the renderer never installs one itself. `HeadlessGpu` installs a logging one, and `install_nonfatal_error_handler(&device, &UncapturedErrors)` is there for the app to opt in. Fact: error scopes belong to the device, not the thread (one `ErrorSink` per device, `wgpu_core.rs:626-640, 1788-1800`). In the app, background init therefore also catches errors Iced raises on the same device during the init window; that surfaces as an init failure (CPU fallback) rather than an abort.

### Windows backend matrix (AMD Radeon Vega iGPU, Ryzen 5 5500U, Vulkan driver 22.20.16, D3D driver 31.0.12016.9)

Release build, `spike_d`. "Init" is the whole `init_in_background` (gate, `Renderer::new`, smoke render) with area AA only. The machine was running other builds during some runs. That inflates the FXC figures about 2.5 times (the compile is CPU bound), so both a quiet and a loaded range are given.

| Backend | Chosen by default? | vello init, quiet | vello init, loaded | CPU vs GPU output | GPU page render 1x / 2x (incl. wait) | Verdict |
|---|---|---|---|---|---|---|
| Vulkan | yes (wgpu prefers it) | 0.10 to 0.24 s (1.1 to 1.7 s once, cold driver cache) | n/a | identical to DX12 | 6.7 / 9.0 ms | supported |
| DX12, FXC (wgpu default) | only if `WGPU_BACKEND=dx12` or no Vulkan | 6.2 to 7.1 s | 9 to 19 s | identical | 10.7 / 11.8 ms | supported; slow first start, init in background |
| DX12, DXC (`WGPU_DX12_COMPILER=dxc`, `dxcompiler.dll` and `dxil.dll` beside the exe) | no | 0.6 to 0.8 s | about 2 s | identical | not re-measured | not reachable from Iced 0.14, see below |
| OpenGL | no | refused by the gate | n/a | n/a | n/a | refused, no panic |
| OpenGL, `LIBERTUM_GPU_ALLOW_GL=1` (diagnostic) | no | 2.2 s | 2.3 s | identical | 17.6 / 20.6 ms | works on this one driver, slowest |

**GL result.** Before this work `WGPU_BACKEND=gl spike_d` panicked. Root cause (measured and read): not vello. `Renderer::new` succeeded; the panic came from our own `create_target`, which listed an sRGB view format, and that needs downlevel `VIEW_FORMATS`, which GL lacks (`Downlevel flags DownlevelFlags(VIEW_FORMATS) are required but not supported`). Now the gate reports `unavailable: the OpenGL backend is not enabled by default ...`, with `uncaptured wgpu errors: []` and a normal exit. With the override vello renders correctly on GL here (texture created without the sRGB view) but 1.5 to 2.5 times slower than Vulkan, so GL stays opt-in. One driver proves little, so the default refusal is policy (opinion).

**DX12 root cause (fact).** wgpu 27's default DX12 shader compiler is FXC (`Dx12Compiler::Fxc`, marked `#[default]`, `wgpu-types-27.0.1/src/instance.rs:513-519`; its doc calls it "old, slow and unmaintained"). Each vello compute shader (about 20 with area AA only) goes naga WGSL to HLSL, then D3DCompile via FXC when the pipeline is created (`wgpu-hal-27.0.4/src/dx12/shader_compilation.rs:217-247`; optimised unless `InstanceFlags::DEBUG` is set). Measured: 1 thread 14.7 s, 2 threads 9.6 s, 4 threads 6.2 s, 8 and 12 threads 6.3 to 6.4 s, so a few big shaders form the critical path and more than 4 threads gains nothing. Other levers measured and rejected: `WGPU_DEBUG=1` (FXC skips optimisation; also enables the D3D debug layer) 2.9 s, because it yields slower shaders; vello `use_cpu` 3.2 s, because vello documents it as poor performance. Fewer AA modes is already applied (`AaSupport::area_only()`, so only one `fine_*` shader is built). DXC takes 0.6 s.

**What the app can influence (fact).** `iced_wgpu` 0.14 builds its instance as `InstanceDescriptor { backends: settings.backends, flags: debugging only with its `strict-assertions` feature, ..Default::default() }` (`compositor.rs:55-64`). `Default` does not read the environment (only `with_env` and `from_env_or_default` do, and `wgpu::Instance::new` does not call them). So `WGPU_DX12_COMPILER` and the other instance variables are ignored in the app. Iced reads only `WGPU_BACKEND` (`compositor.rs:275`, replacing `Backends::all()` from `settings.rs:36`) and `WGPU_POWER_PREF` (`compositor.rs:85-86`). The `static-dxc` feature only makes `Dx12Compiler::StaticDxc` available (it adds the `mach-dxcompiler-rs` dependency); it does not change the default. Using DXC needs a patched `iced_wgpu` (set `backend_options.dx12.shader_compiler`) plus shipping `dxcompiler.dll` (14 MB) and `dxil.dll` (1.5 MB) from the Windows SDK. Caution (fact, `wgpu-hal-27.0.4/src/dx12/instance.rs:73-90`): if DXC is requested and its DLL cannot be loaded, DX12 instance creation fails outright, so such a patch must test for the DLLs and fall back to FXC. Decision: no Iced patch now. Accept 6 to 7 s of background init on DX12, hidden by `init_in_background` (CPU pages show meanwhile), which defaults to `min(4, cores - 1)` compile threads (1 on macOS).

**Which backend Iced picks on Windows (fact).** Backends default to all. Adapters are enumerated in the order Vulkan, Metal, DX12, GL (`wgpu-core-27.0.3/src/instance.rs:104-114`) and then stably sorted by device type for the power preference (`instance.rs:498-526`), so for the same GPU Vulkan beats DX12, and GL (reported as "Other") comes last. On this machine the app is therefore on Vulkan unless `WGPU_BACKEND` says otherwise.

**Compositor backend recommendation for Windows (opinion).** Keep wgpu's automatic order (Vulkan, then DX12, then GL) and do not hard-code either. On this iGPU Vulkan starts vello 30 to 60 times faster than DX12 and renders pages about 40% faster. DX12 should stay as the second choice because it is often the more reliable path on old Intel iGPUs. The real risk is a Vulkan driver that starts but misbehaves (old Intel drivers are the usual suspect; no such hardware was available, so this is unverified). Mitigate with the PLAN §4.5 crash flag: if the previous launch died during GPU start, relaunch with `WGPU_BACKEND=dx12`, then with the tiny-skia fallback (`ICED_BACKEND=tiny-skia`). Offer "Graphics backend: Automatic, Vulkan, DirectX 12" in Settings (set the env var before `iced::run`; takes effect after restart), populated from `gpu::probe_all()`. Do not prefer DX12 by default: its only advantage is hypothetical driver maturity, and its measured costs are the 6 to 7 s FXC build and slower page renders.

### macOS (Metal): analysis only, nothing run on a Mac

`cargo check -p libertum-render --features gpu` passes for `aarch64-apple-darwin` and `x86_64-apple-darwin`.

- **Minimum OS and GPU.** Fact: wgpu's Metal backend has a minimum target of macOS 10.13 (`wgpu-hal-27.0.4/src/metal/mod.rs:15-17`); `maxBufferLength` needs 10.14 (`adapter.rs:767`); `MTLGPUFamily` queries need 10.15 (`adapter.rs:563`); the MSL version is chosen from the OS (`adapter.rs:612-632`). Estimate: 10.15 Catalina or newer is a safe floor for vello; Apple Silicon needs macOS 11 anyway. Iced 0.14 or winit may impose a higher floor of their own (not checked).
- **What vello needs from Metal.** Fact: `INDIRECT_EXECUTION` comes from `macOS_GPUFamily1_v1`, which every Mac has (`adapter.rs:474-477`), and wgpu-types notes that Metal on Apple1/Apple2 (old iOS) GPUs lacks it, so the gate refuses those. Limits on a Mac: 31 storage buffers per stage (`adapter.rs:733`; vello needs 8), 128 textures per stage on macOS (`adapter.rs:735`; needs 3), 1024 threads per group (`adapter.rs:824`; needs 256), threadgroup memory 32 KiB on macOS GPU family 1 v2 and up, 16 KiB otherwise (`adapter.rs:835`; vello needs exactly 16,384, so even the smaller case just fits), 65,535 workgroups per dimension (`adapter.rs:1076`). `Rgba8Unorm` gets write-only storage (`adapter.rs:145-154`). No optional wgpu feature is required.
- **Shader build.** Fact: vello recommends and defaults to `num_init_threads = Some(1)` on macOS (`vello-0.7.0/src/lib.rs:391-415`, citing a Bevy issue). The old code passed `None` explicitly, which overrode that; `new_with` and `init_in_background` now default to 1 on macOS (`MACOS_INIT_THREADS`). Estimate: Metal shader build is much cheaper than FXC, but it is unmeasured; the background API hides it either way.
- **Surface format.** Fact: the Metal surface offers only `Bgra8Unorm`, `Bgra8UnormSrgb`, `Rgba16Float` and, where supported, `Rgb10a2Unorm` (`adapter.rs:362-369`). Iced picks the first sRGB format when `GAMMA_CORRECTION` is true (always, unless its `web-colors` feature is on; `iced_graphics` `color.rs:30`, `compositor.rs:111`), so on macOS it picks `Bgra8UnormSrgb`; on Windows it picks whichever sRGB format the surface lists first (`Bgra8UnormSrgb` or `Rgba8UnormSrgb`). If no sRGB format exists it falls back to the first format (`compositor.rs:114-119`), which could even be `Rgba16Float`.
- **Format handling in our code.**
  - vello always renders into `Rgba8Unorm` + `STORAGE_BINDING` (`TARGET_FORMAT`), independent of the surface. Surface mismatch is the blit's job. vello's own windowed helper does the same (`vello-0.7.0/src/util.rs:80-84` picks an `Rgba8Unorm` or `Bgra8Unorm` surface and blits).
  - vello writes sRGB-encoded values. The blit must sample through `Rgba8UnormSrgb` when the surface format is sRGB (the hardware re-encodes on write), and through plain `Rgba8Unorm` otherwise. `gpu::page_view_format(surface_format)` implements that rule and is tested for `Bgra8UnormSrgb`, `Rgba8UnormSrgb`, `Bgra8Unorm`, `Rgba8Unorm`, `Rgba16Float` and `Rgb10a2Unorm`. Channel order of the surface does not matter, because the blit shader reads and writes RGBA and the hardware swizzles. `VelloPageRenderer::sampled_view(texture, surface_format)` creates the right view, or returns `None` when the device cannot make an sRGB view (then decode sRGB in the blit shader).
  - Platform-specific paths in `gpu.rs`: (1) the `Rgba8UnormSrgb` view format on the page texture is now decided by a runtime probe. It works on Vulkan and DX12 here; on Metal the default downlevel flags are not cleared (`adapter.rs` `capabilities()`), so it is expected to work (estimate). (2) `read_back` pads rows to `COPY_BYTES_PER_ROW_ALIGNMENT` (256), identical on every backend. (3) `device.poll(wait_indefinitely)` is used only by tests and timing helpers. (4) `Device::as_hal` backend detection is `cfg`-gated per target (Metal on Apple; Vulkan, DX12 and GL elsewhere), because `wgpu-27.0.1/build.rs` compiles only Metal on Apple targets unless `angle` or `vulkan-portability` is enabled.
  - Unverified on any Mac: the sRGB view of a storage texture on Metal, hi-DPI behaviour, and Metal shader compile time on an Intel Mac iGPU.

### Verification

- Verified here: 14 pure gate and format unit tests; error-scope tests (a validation error and a panic become values; scopes stay balanced); backend recovered from a bare device; `cargo test -p libertum-render --features gpu` with `WGPU_BACKEND` = vulkan, dx12 and gl (22 + 3 + 3 pass; on GL the vello tests skip through the gate); `spike_d` on all three; clippy `-D warnings` with and without `--all-features`; `cargo check` for both Apple targets.
- Not verified: any non-AMD GPU (Intel, NVIDIA, Apple); a Windows machine without Vulkan; Iced's `shader` widget (not built), and so the in-app interaction of device-wide error scopes with Iced's own frames; the blit's sRGB round trip on screen; anything on macOS at runtime.
