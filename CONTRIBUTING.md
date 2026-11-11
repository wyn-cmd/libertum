# Notes for contributors — Libertum

Working notes for anyone changing this repository. Everything here applies whatever editor or tooling you use; keep it that way.

## What this project is

Libertum is a cross-platform (Windows, macOS, Linux) desktop word processor in Rust. It is built for long documents (books, hundreds of pages) on modest hardware (4 GB RAM, older integrated GPU). It edits ODT and DOCX natively, and Markdown in a WYSIWYG mode. The interface follows Apple Pages: the document canvas sits on the left, with a collapsible inspector on the right, switched by the Format and Document toolbar buttons.

## Read before working

1. **`docs/STATUS.md`:** where the project is now, what is done, and what to do next. Start here.
2. **`PLAN.md`:** the source of truth for architecture, performance budgets (§2), crate boundaries (§3), formats (§6) and phases (§9). Read the section relevant to your task, not only this file.
3. **`docs/decisions.md`:** every deviation from PLAN.md, with reasons. Do not undo a logged decision without the user's approval.
4. **`docs/spikes.md`:** measured results (performance, GPU backends, API findings for parley, vello and Iced).
5. **`docs/third-party-licences.md`:** licences of embedded data (hyphenation patterns) and notices that must ship.

## Toolchain and commands

- Rust stable (developed on 1.99.1, MSVC on Windows), edition 2024, workspace resolver 3.
- On the development machine, `cargo` may not be on `PATH` in a fresh shell. Use `~/.cargo/bin/cargo` (Bash) or `& "$env:USERPROFILE\.cargo\bin\cargo.exe"` (PowerShell).
- The macOS targets `aarch64-apple-darwin` and `x86_64-apple-darwin` are installed for compile-checking. Nothing can be run on macOS locally.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo test -p libertum-render --features gpu          # GPU tests skip themselves if no adapter
cargo check --workspace --all-features --target aarch64-apple-darwin   # macOS compile check
cargo run --release -p libertum-app                   # binary is named `libertum`
cargo run --release -p libertum-render --example spike_a              # CPU layout/render benchmark
cargo run --release -p libertum-render --features gpu --example spike_d   # GPU vs CPU comparison
cargo bench -p libertum-editor --bench editing        # keystroke and undo benches (criterion)
cargo bench -p libertum-layout --bench pagination     # full and incremental pagination benches
```

`cargo bench ... -- --quick` only works with `--bench <name>` (the library test harness rejects the flag).

**Definition of done for any task:** fmt is clean, clippy passes with `-D warnings` (with and without `--all-features`), all tests pass, the macOS compile check passes if you touched platform or GPU code, and the performance budgets still hold if you touched the canvas, layout or rendering. Report anything you could not verify. Do not claim it works.

CI is in `.github/workflows/ci.yml`: fmt, clippy, tests and a release build on Windows, macOS (Apple Silicon and Intel) and Linux.

## Workspace layout

| Crate | Role | Status |
|---|---|---|
| `libertum-model` | Document tree, operations with exact inverses, undo/redo. `format.rs` (character and paragraph formatting, style sheet), `paragraph.rs` (paragraphs, runs, `ParaProps`, blocks), `table.rs` (tables, rows, cells), `lists.rs`, `ops.rs`, `document.rs`, `history.rs`. No I/O, no UI. | Working |
| `libertum-layout` | parley shaping and line breaking (`engine.rs`: indents, line spacing, super/subscript, ligatures, hyphenation), incremental pagination with keep and widow rules, decorations (page numbers, TOC leaders, list markers, hyphens) (`paginate.rs`). `spike.rs` holds `ParagraphSnapshot` (model to layout) and the demo-document generator. | Working |
| `libertum-render` | Page rendering. CPU: tiny-skia + swash (`renderer.rs`, `glyphs.rs`, `page_cache.rs`). GPU: vello behind the `gpu` feature (`gpu.rs`, `gpu/caps.rs`). `backend.rs` holds the `PageBackend` trait. | Working (vello not wired into the canvas) |
| `libertum-editor` | Cursor, selection, movement, typing, clipboard, character and paragraph formatting, styles, lists, TOC sync, sections. UI-agnostic. | Working |
| `libertum-docx`, `libertum-odt`, `libertum-markdown` | Format import/export with lossless round-trip | DOCX works end to end (import/export of paragraphs, lists, sections and tables; preserved package parts; app Open/Save; round-trip fixtures); ODT and Markdown still to come |
| `libertum-app` | Iced 0.14 app: `app.rs` (messages, update, formatting commands), `toolbar.rs`, `inspector/`, `home.rs`, `templates.rs`, `widgets/` (stepper, segmented, controls), `canvas.rs` (custom page widget, caret, IME), `docview.rs` (model + editor + layout + strip render cache), `shortcuts.rs` (the single shortcut map), `theme.rs` (all colour tokens), `strings.rs` (all UI text), `state.rs`, `settings.rs`, `perf.rs` | Working; some sidebar controls not bound yet (see STATUS) |

**Boundaries (enforced):** the format crates and `libertum-model` never depend on UI crates. `libertum-layout` and `libertum-render` never depend on `libertum-app`.

## Key architecture facts (details in PLAN.md and docs/)

- **Performance is the product.** Nothing may be O(document size) on the keystroke path. Layout is cached per paragraph, pagination is incremental, and only visible pages (±1) are rendered.
- **Rendering has two layers** (see PLAN §4.5):
  - The Iced window compositor is always wgpu, with tiny-skia only as an automatic fallback (it measured 15 fps vs 60 fps).
  - The **Graphics acceleration** setting selects only the page renderer: CPU (default) or vello on the GPU.
- **Pinned versions with reasons:** iced 0.14, parley 0.12, vello **0.7** (iced_wgpu 0.14 needs wgpu 27; vello 0.11 needs wgpu 30), wgpu 27.0.1, peniko 0.6. Do not bump vello or Iced independently. They must share one wgpu version, so check with `cargo tree -i wgpu`.
- **GPU safety:** vello is only started after `gpu::caps::check_caps` passes (Vulkan, DX12 or Metal; OpenGL refused by default). Initialisation runs in the background (`init_in_background`), and failure falls back to CPU pages. Never introduce a code path that can panic on an unsupported GPU.
- **Known vello 0.7 issue:** embedded bitmap strikes (for example in Cambria) render invisibly. `gpu.rs` works around this by renaming the `EBLC` table.
- **Default format** for new documents is ODT, and the user can switch it to DOCX in Settings.
- **Formatting model** (details in `docs/decisions.md`, Phase 2 entries):
  - Formatting resolves as document defaults, then the paragraph style's `base` chain, then direct formatting. Character formatting is a partial `CharFormat` on each `Run`; paragraph formatting is a partial `ParaFormat` in `ParaProps::format`. `None` always means "inherit". Keep "set here" apart from "inherited": DOCX and ODT round-trip depends on it.
  - Every edit is an `Operation` with an exact inverse; the proptest in `libertum-model/src/tests.rs` checks each one. Add new operation kinds to it.
  - Runs are kept normalised, so equal-looking paragraphs compare equal (`Document::content_eq`).
  - Lists: `ParaProps::list` (style, level, group). Numbers are computed (`Document::list_labels`), never stored; `DocView::sync` renumbers only after a structural or list change.
  - A style change doesn't bump paragraph versions, so `DocView` folds a style epoch into snapshot versions. Anything that changes how a paragraph looks without editing it must do the same.
- **The keystroke path must stay O(1) in document size.** O(paragraphs) passes are allowed only on explicit actions (style changes, structural edits, renumbering).

## Rules

1. **Never write Iced, parley, vello or wgpu code from memory.** Their APIs change between releases. Read the exact pinned version's source in `~/.cargo/registry/src/*/<crate>-<version>/` or on docs.rs before using an API.
2. No `unwrap()` or `expect()` on I/O, parsing, GPU or user-input paths. Use typed errors (`thiserror`).
3. User-facing strings go in `libertum-app/src/strings.rs`, written in British/Singapore English (colour, centre, organise). Colours go in `theme.rs` as tokens for both light and dark themes.
4. New dependencies must be MIT, Apache-2.0, ISC, BSD or Zlib. Ask before adding anything GPL/LGPL, and log every new dependency in `docs/decisions.md`. Embedded data counts too: `hypher` languages are enabled one at a time, after checking each pattern file's licence (`docs/third-party-licences.md`).
5. Do not use Apple SF Symbols. The icons are Lucide (ISC), in `crates/libertum-app/assets/icons/`, with the licence included.
6. Any deviation from PLAN.md goes in `docs/decisions.md` (dated, one bullet: what changed and why). When you finish a unit of work, update `docs/STATUS.md`.
7. Stop and ask the user when: a performance budget can't be met, a spike or approach fails, a decision in PLAN.md or decisions.md would need reversing, or a licence is non-permissive.
8. Do not commit, push or create branches unless the user asks.
9. Every format feature needs a round-trip fixture under `tests/fixtures/`. Every new behaviour needs tests.

## Working notes (gotchas found so far)

- **A running app locks `target\release\libertum.exe` on Windows**, so a release build silently leaves the old exe in place (cargo reports "Access is denied"). The fresh binary is `target\release\deps\libertum.exe`. When the user reports that something "doesn't work", compare exe timestamps first: twice so far the report came from a stale build.
- **parley 0.12 limits:** no tab stops, no hyphenation (Libertum adds break opportunities through `RangedBuilder::set_line_break_override`, whose callback is called once per character pair in order; a test guards this), and no hyphen glyph at a break (drawn as a decoration). Line `metrics().advance` is measured before justification; use the positioned glyphs for where a justified line really ends.
- **iced_wgpu uploads images of 2 MiB or more asynchronously** and draws nothing until done; that is why page bitmaps are split into strips (`docview.rs`).
- **Inspector bindings:** a control is "working" only when its message reaches the editor (`App::apply_format`, `App::apply_field`, `App::apply_document`) and `App::refresh_format` reads the value back from `Editor::char_summary` / `para_summary`. Add an app test for each new control.
- UI changes need screenshots in both themes. Driving the app with `SendKeys` and `mouse_event` (screenshot recipe below) works; inspector coordinates depend on DPI and zoom, so capture once to find them.

## Debug and test hooks (environment variables)

| Variable | Effect |
|---|---|
| `LIBERTUM_OPEN=<path>` | Start with that `.docx` open instead of the home screen (for screenshots and save/close checks) |
| `LIBERTUM_DEMO=1` | Open the generated 500-page demo document instead of the home screen (implied by `LIBERTUM_AUTOSCROLL`) |
| `LIBERTUM_DEMO_PAGES=<n>` | Size of the generated demo document (default 500; 1,000 for the Phase 1 acceptance run) |
| `LIBERTUM_HEADERS=1` | With `LIBERTUM_DEMO=1`, give the demo a running header (with first-page and even-page variants) and a `{page} of {pages}` footer, for header/footer screenshots |
| `LIBERTUM_LOG=debug\|trace` | More logging (for example, input method events) |
| `LIBERTUM_PERF=1` | Log frame and update timings, and print a summary on exit |
| `LIBERTUM_AUTOSCROLL=1` | Scroll page 1 to the last page, then exit (combine with `LIBERTUM_PERF`). `LIBERTUM_AUTOSCROLL_SPEED` sets px/s (default 4000) |
| `LIBERTUM_THEME=light\|dark` | Force the theme |
| `LIBERTUM_INSPECTOR=format-style\|format-layout\|format-more\|document\|section` | Open a given inspector pane at start |
| `LIBERTUM_BOOKLET=1` | Start in the booklet (two-page spread) view |
| `LIBERTUM_MENU=view\|zoom` | Open a toolbar menu at start (for screenshots) |
| `LIBERTUM_ZOOM=<percent>\|fit` | Initial zoom |
| `LIBERTUM_UNITS=cm\|in` | Measurement units |
| `LIBERTUM_FILTER=linear` | Linear instead of nearest filtering for page images |
| `LIBERTUM_GPU_ALLOW_GL=1` | Allow vello on the OpenGL backend (refused by default) |
| `ICED_BACKEND=wgpu\|tiny-skia` | Force Iced's compositor (the app sets `wgpu,tiny-skia` if unset) |
| `WGPU_BACKEND=vulkan\|dx12\|metal\|gl` | Force the wgpu backend |

**Scrolling performance check:** run `LIBERTUM_PERF=1 LIBERTUM_AUTOSCROLL=1 target/release/libertum` and compare against the baseline of 16.7 ms mean and about 18 ms p95 frame interval on the development machine.

**Screenshot recipe (Windows, PowerShell):**
1. Call `user32!SetProcessDPIAware`, then start the release exe and wait about 6 s.
2. Call `SetForegroundWindow` on its main window.
3. Read the window bounds with `dwmapi!DwmGetWindowAttribute(hwnd, 9 /*EXTENDED_FRAME_BOUNDS*/, out RECT, 16)`.
4. Capture with `System.Drawing.Graphics.CopyFromScreen`, save as PNG, then inspect it.

Always look at screenshots after UI changes, in both themes.
