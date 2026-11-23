# Contributing to Libertum

Working notes for anyone changing this repository. Keep them current.

## What this project is

Libertum is a cross-platform (Windows, macOS, Linux) desktop word processor in Rust. It is built for long documents (books, hundreds of pages) on modest hardware (4 GB RAM, older integrated GPU). It edits ODT and DOCX natively, and Markdown in a WYSIWYG mode. The interface follows Apple Pages: the document canvas sits on the left, with a collapsible inspector on the right, switched by the Format and Document toolbar buttons.

## Read before working

1. **`docs/STATUS.md`:** where the project is now, what is done, and what to do next. Start here.
2. **`PLAN.md`:** the source of truth for architecture, performance budgets (§2), crate boundaries (§3), formats (§6) and phases (§9). Read the section relevant to your task, not only this file.
3. **`docs/decisions.md`:** every deviation from PLAN.md, with reasons. Do not undo a logged decision without the maintainer's approval.
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
cargo run --release -p libertum-editor --example find_bench           # Find on a 1,000-page document
powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1   # dist\Libertum-Setup-<version>.exe
packaging/linux/build-packages.sh                     # dist/libertum_<version>_amd64.deb and dist/libertum-<version>-1.x86_64.rpm
packaging/linux/build-appimage.sh                     # dist/Libertum-<version>-x86_64.AppImage (downloads linuxdeploy/appimagetool)
packaging/macos/build-app.sh                          # dist/Libertum.app and dist/Libertum-<version>-macos-<arch>.dmg (on macOS)
cargo run -p libertum-render --example export_pdf     # target/pdf-sample{,-a}.pdf, for a viewer and a PDF/A check
cargo run --release -p libertum-app --example page_count -- tests/fixtures/odt/rich.odt   # our page count, for the LibreOffice comparison
tools/check-page-counts.sh <documents...>             # compares with LibreOffice's count (needs soffice and pdfinfo)
cargo bench -p libertum-layout --bench pagination -- --save-baseline base   # then again with `head`, then:
tools/check-benches.py target/criterion --threshold 10 --floor-us 50        # the performance job's comparison
```

The macOS compile check covers the whole workspace; to lint the platform-specific code for macOS with warnings as errors (as CI does), leave out the benches, whose C dependencies need a macOS compiler: `cargo clippy -p libertum-app -p libertum-installer -p libertum-render --bins --lib --tests --examples --all-features --target aarch64-apple-darwin -- -D warnings`.

`cargo bench ... -- --quick` only works with `--bench <name>` (the library test harness rejects the flag).

**Definition of done for any task:** fmt is clean, clippy passes with `-D warnings` (with and without `--all-features`), all tests pass, the macOS compile check passes if you touched platform or GPU code, and the performance budgets still hold if you touched the canvas, layout or rendering. Report anything you could not verify. Do not claim it works.

CI is in `.github/workflows/ci.yml`: fmt, clippy, tests and a release build on Windows, macOS (Apple Silicon and Intel) and Linux. Each platform's build is uploaded as a run artifact (`libertum-<target>`), and the Windows job also packs `Libertum-Setup.exe`; the Linux job also builds the `.deb` and `.rpm` (`packaging/linux/build-packages.sh`) and the AppImage (`packaging/linux/build-appimage.sh`) and installs the packages on the runner as a check; the macOS jobs build `Libertum.app` and the `.dmg` (`packaging/macos/build-app.sh`). A separate `libreoffice-page-counts` job compares our page counts with LibreOffice's (`tools/check-page-counts.sh`, ±2%), and a `performance` job on pull requests benches the base and head commits in one runner and fails a regression past 10% and 50 µs (`tools/check-benches.py`). `RUSTFLAGS=-D warnings` is set for every job, so code used only on one platform must not leave dead code on the others. The Linux job runs on `ubuntu-24.04` on purpose: the `.deb` records the glibc the binary needs (2.39), and a newer runner image would silently raise it.

## Releases

A release starts from a green CI run on `main`. Write `docs/RELEASE-<version>.md` with the notes, bump the workspace version in `Cargo.toml`, commit, and push a `v<version>` tag. `.github/workflows/release.yml` builds the four platform bundles, writes `SHA256SUMS`, creates or edits the GitHub release and attaches the files.

To assemble a release by hand, download the CI run's artifacts (`gh run download <run id>`), collect the `.dmg`, `.AppImage`, `.deb`, `.rpm` and `Libertum-Setup.exe` files, make the versioned installer copy and `SHA256SUMS`, then use `gh release create v<version> <files> --target main`. Nothing is code-signed (PLAN §9): say so in the release notes, with the SmartScreen and Gatekeeper workarounds.

**The in-app updater needs two more things in every release** (`crates/libertum-app/src/install.rs`; without them it falls back to opening the release page):

1. **`Libertum-Setup-<version>.exe`**, attached under that exact name (the updater looks for it; `Libertum-Setup.exe` stays as the fixed-name copy).
2. **A `SHA256SUMS` file** listing every attached file, made after all the files are collected: `sha256sum <files> > SHA256SUMS` (Git Bash on Windows has `sha256sum`), then attach it. It must include each file name exactly as attached. The macOS artifact also holds `Libertum-<version>-macos-<arch>.zip` (the app, for the updater); attach it next to the `.dmg`.

The updater checks each download against `SHA256SUMS` from the same release, so it catches a damaged download, not a tampered release (nothing is code-signed). On Linux a `.deb` or `.rpm` is installed through `pkexec`, so the release files must keep the names the updater looks for (`libertum_<version>_amd64.deb`, `libertum-<version>-1.x86_64.rpm`); without `pkexec`, the updater downloads the package and shows the command instead.

## Workspace layout

| Crate | Role | Status |
|---|---|---|
| `libertum-model` | Document tree, operations with exact inverses, undo/redo. `format.rs` (character and paragraph formatting, style sheet), `paragraph.rs` (paragraphs, runs, `ParaProps`, blocks), `table.rs` (tables, rows, cells), `lists.rs`, `ops.rs`, `document.rs`, `history.rs`. No I/O, no UI. | Working |
| `libertum-layout` | parley shaping and line breaking (`engine.rs`: indents, line spacing, super/subscript, ligatures, hyphenation), incremental pagination with keep and widow rules, decorations (page numbers, TOC leaders, list markers, hyphens) (`paginate.rs`). `spike.rs` holds `ParagraphSnapshot` (model to layout) and the demo-document generator. | Working |
| `libertum-render` | Page rendering. CPU: tiny-skia + swash (`renderer.rs`, `glyphs.rs`, `page_cache.rs`). GPU: vello behind the `gpu` feature (`gpu.rs`, `gpu/caps.rs`), an experiment the app does not link. `backend.rs` holds the `PageBackend` trait. | Working (CPU pages; GPU pages not offered) |
| `libertum-editor` | Cursor, selection, movement, typing, clipboard, character and paragraph formatting, styles, lists, TOC sync, sections. UI-agnostic. | Working |
| `libertum-docx`, `libertum-odt`, `libertum-rtf` | DOCX, ODT and RTF import/export: paragraphs, runs and styles, lists, sections and page setup, headers and footers, tables (spans, merges, styles), footnotes and endnotes, bookmarks and cross-references, inline images; unknown parts or groups are preserved | Working (round-trip fixtures; Word opens the exports with no repair prompt) |
| `libertum-markdown` | GFM Markdown with byte-identical re-save of unedited blocks, the file's own conventions for edited ones, local images, and `loss_report` (what a Markdown save would drop) | Working |
| `libertum-native` | The `.lbtd` format: the model serialised with serde in a deterministic ZIP (see `docs/format-libertum-native.md`); also the crash-recovery snapshot | Working |
| `libertum-export` | Chapter splitting, the AO3 HTML fragment, EPUB 3 and plain text (model only) | Working |
| `libertum-formats-common` | Shared XML helpers, units, colours, and image bytes (`media`: PNG/JPEG kept, GIF/BMP converted) | Working |
| `libertum-installer` | The single-file Windows installer: carries `libertum.exe`, `install.ps1` and the README appended to its own `.exe`; `pack` builds it (`packaging/windows/build-installer.ps1`) | Working (unsigned) |
| `libertum-app` | Iced 0.14 app: `app.rs` (messages, update, formatting commands), `toolbar.rs`, `inspector/`, `home.rs`, `templates.rs`, `widgets/` (stepper, segmented, controls), `canvas.rs` (custom page widget, caret, IME), `docview.rs` (model + editor + layout + strip render cache), `shortcuts.rs` (the single shortcut map), `theme.rs` (all colour tokens), `strings.rs` (all UI text), `state.rs`, `settings.rs`, `perf.rs` | Working; some sidebar controls not bound yet (see STATUS) |

**Boundaries (enforced):** the format crates and `libertum-model` never depend on UI crates. `libertum-layout` and `libertum-render` never depend on `libertum-app`.

## Key architecture facts (details in PLAN.md and docs/)

- **Performance is the product.** Nothing may be O(document size) on the keystroke path. Layout is cached per paragraph, pagination is incremental, and only visible pages (±1) are rendered.
- **Rendering has two layers** (see PLAN §4.5):
  - The Iced window compositor is always wgpu, with tiny-skia only as an automatic fallback (it measured 15 fps vs 60 fps).
  - Pages are always rendered on the CPU (tiny-skia + swash); there is no Graphics acceleration setting (removed 2026-11-13: vello measured slower per page than the CPU renderer, and the setting never did anything). The vello code stays behind the `gpu` feature as an experiment.
- **Pinned versions with reasons:** iced 0.14, parley 0.12, vello **0.7** (iced_wgpu 0.14 needs wgpu 27; vello 0.11 needs wgpu 30), wgpu 27.0.1, peniko 0.6. Do not bump vello or Iced independently. They must share one wgpu version, so check with `cargo tree -i wgpu`. winit 0.30.13, iced_core 0.14.0 and iced_winit 0.14.1 are vendored under `vendor/` for Wayland touchpad pinch; re-apply the patches before bumping them (`docs/decisions.md`, 2026-11-14).
- **GPU safety:** vello is only started after `gpu::caps::check_caps` passes (Vulkan, DX12 or Metal; OpenGL refused by default). Initialisation runs in the background (`init_in_background`), and failure falls back to CPU pages. Never introduce a code path that can panic on an unsupported GPU.
- **Known vello 0.7 issue:** embedded bitmap strikes (for example in Cambria) render invisibly. `gpu.rs` works around this by renaming the `EBLC` table.
- **Default format** for new documents is ODT, and the maintainer can switch it to DOCX in Settings.
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
7. Stop and ask the maintainer when: a performance budget can't be met, a spike or approach fails, a decision in PLAN.md or decisions.md would need reversing, or a licence is non-permissive.
8. Do not commit, push or create branches unless the maintainer asks.
9. Every format feature needs a round-trip fixture under `tests/fixtures/`. Every new behaviour needs tests.

## Commits and pull requests

- Commits and pull requests are attributed to the maintainer (wyn-cmd) only. Do not add co-authors, bots or tooling to commits.
- Commit messages: one-line subject, then body paragraphs written as unwrapped text, one paragraph per line. Do not insert newlines to wrap at a fixed width. GitHub wraps the text itself when it renders the page.
- Do not rewrite published history (force-push, retag) without the maintainer's explicit approval.
- A release is made by pushing a `v<version>` tag. `.github/workflows/release.yml` builds the platform bundles, writes `SHA256SUMS` and publishes the release with `docs/RELEASE-<version>.md` as its notes.

## Working notes (gotchas found so far)

- **A running app locks `target\release\libertum.exe` on Windows**, so a release build silently leaves the old exe in place (cargo reports "Access is denied"). The fresh binary is `target\release\deps\libertum.exe`. When the maintainer reports that something "doesn't work", compare exe timestamps first: twice so far the report came from a stale build.
- **parley 0.12 limits:** no tab stops, no hyphenation (Libertum adds break opportunities through `RangedBuilder::set_line_break_override`, whose callback is called once per character pair in order; a test guards this), and no hyphen glyph at a break (drawn as a decoration). Line `metrics().advance` is measured before justification; use the positioned glyphs for where a justified line really ends.
- **iced_wgpu uploads images of 2 MiB or more asynchronously** and draws nothing until done; that is why page bitmaps are split into strips (`docview.rs`).
- **Wayland sends zero-delta wheel events** for `axis_source` and `axis_stop`. The canvas ignores them; treating one as movement zeroes the fling velocity as the fingers lift, which made Linux scrolling stop dead.
- **Inspector bindings:** a control is "working" only when its message reaches the editor (`App::apply_format`, `App::apply_field`, `App::apply_document`) and `App::refresh_format` reads the value back from `Editor::char_summary` / `para_summary`. Add an app test for each new control.
- UI changes need screenshots in both themes. Driving the app with `SendKeys` and `mouse_event` (screenshot recipe below) works; inspector coordinates depend on DPI and zoom, so capture once to find them.

## Debug and test hooks (environment variables)

| Variable | Effect |
|---|---|
| `LIBERTUM_OPEN=<path>` | Start with that `.docx` open instead of the home screen (for screenshots and save/close checks) |
| `LIBERTUM_DEMO=1` | Open the generated 500-page demo document instead of the home screen (implied by `LIBERTUM_AUTOSCROLL`) |
| `LIBERTUM_DEMO_PAGES=<n>` | Size of the generated demo document (default 500; 1,000 for the Phase 1 acceptance run) |
| `LIBERTUM_HEADERS=1` | With `LIBERTUM_DEMO=1`, give the demo a running header (with first-page and even-page variants) and a `{page} of {pages}` footer, for header/footer screenshots |
| `LIBERTUM_BENCHMARK=1\|show` | Run the benchmark at start-up. `1` prints the report to the log and exits; `show` leaves the results window open (for screenshots) |
| `LIBERTUM_LOG=debug\|trace` | More logging (for example, input method events) |
| `LIBERTUM_PERF=1` | Log frame and update timings, and print a summary on exit |
| `LIBERTUM_AUTOSCROLL=1` | Scroll page 1 to the last page, then exit (combine with `LIBERTUM_PERF`). `LIBERTUM_AUTOSCROLL_SPEED` sets px/s (default 4000) |
| `LIBERTUM_THEME=light\|dark` | Force the theme |
| `LIBERTUM_SCREENSHOT=<file.png>` | Save a picture of the window after `LIBERTUM_SCREENSHOT_DELAY` seconds (default 6) and exit. Works where the desktop blocks other programs from capturing the screen (GNOME on Wayland); combine with `LIBERTUM_THEME`, `LIBERTUM_MENU`, `LIBERTUM_AO3` and the other screenshot hooks (`src/shot.rs`) |
| `LIBERTUM_LAYOUT_THREADS=<n>` | Threads that shape paragraphs ahead of pagination (0 or unset: one per core, up to 4; 1: none; more than 4 only for measuring), for measuring |
| `LIBERTUM_INSPECTOR=format-style\|format-layout\|format-more\|document\|section` | Open a given inspector pane at start |
| `LIBERTUM_BOOKLET=1` | Start in the booklet (two-page spread) view |
| `LIBERTUM_THUMBNAILS=1` | Start with the page thumbnails column shown |

A document path given as the first command-line argument (what Windows passes when a file is double-clicked or opened with "Open with") opens straight into the editor; `LIBERTUM_OPEN` does the same for scripts.
| `LIBERTUM_MENU=view\|zoom\|insert\|export` | Open a toolbar menu at start (for screenshots) |
| `LIBERTUM_AO3=1` | Open the Export menu's AO3 chapter list at start (for screenshots) |
| `LIBERTUM_AO3_PICK=<n>` | With `LIBERTUM_AO3=1`, open that chapter's HTML window at start |
| `LIBERTUM_AO3_TICK=1` | With `LIBERTUM_AO3_PICK`, also tick every suggested tag (for screenshots of the per-category Copy buttons) |
| `LIBERTUM_TAGS=1\|rating\|warning` | Open the AO3 tag vocabulary window at start (for screenshots); `rating` or `warning` opens it on that category |
| `LIBERTUM_SETTINGS=1` | Open the Settings window at start (for screenshots) |
| `LIBERTUM_PREEDIT=<text>` | Show that text as an input-method composition at the caret (for screenshots; independent of the platform input method) |
| `LIBERTUM_MCP_RUNNER=1` | Open the MCP tool runner at start (for screenshots; needs `dev_mode` and `mcp_enabled` in the settings file) |
| `LIBERTUM_MACROS=<folder>` | Read recipe macros from that folder instead of `macros/` in the data folder (for screenshots and tests) |
| `LIBERTUM_UPDATE=<version>` | Show the update prompt for that version at start (for screenshots; no network call) |
| `LIBERTUM_TAG_SEED=1` | With `LIBERTUM_TAGS`, fill an empty tag vocabulary with example tags in memory (for screenshots; nothing is written) |
| `LIBERTUM_TYPE=<text>` | Type that text into the open document through the real key path at about eight characters a second, then exit (for spellcheck and typing measurements); `LIBERTUM_TYPE_AFTER=<seconds>` delays the first key |
| `LIBERTUM_COMPARE=1\|next` | Start in Compare with Posted; `next` also steps to the first change (for screenshots) |
| `LIBERTUM_LIBRARY=<folder>` | Open the Library window on that folder at start (for screenshots) |
| `LIBERTUM_HISTORY=<file>` | Open that document's version history window at start (for screenshots) |
| `LIBERTUM_TEMPLATES=<folder>` | Use that folder for added templates instead of the platform data folder (for screenshots and tests) |
| `LIBERTUM_ZOOM=<percent>\|fit` | Initial zoom |
| `LIBERTUM_UNITS=cm\|in` | Measurement units |
| `LIBERTUM_FILTER=linear` | Linear instead of nearest filtering for page images |
| `LIBERTUM_GPU_ALLOW_GL=1` | Allow vello on the OpenGL backend (refused by default) |
| `ICED_BACKEND=wgpu\|tiny-skia` | Force Iced's compositor (the app sets `wgpu,tiny-skia` if unset) |
| `WGPU_BACKEND=vulkan\|dx12\|metal\|gl` | Force the wgpu backend |

**Scrolling performance check:** run `LIBERTUM_PERF=1 LIBERTUM_AUTOSCROLL=1 target/release/libertum` and compare against the baseline of 16.7 ms mean and about 18 ms p95 frame interval on the development machine. For the deep-scroll case (the one that used to grow quadratic in the page number), use a long document and a faster pass: `LIBERTUM_DEMO_PAGES=2000 LIBERTUM_PERF=1 LIBERTUM_AUTOSCROLL=1 LIBERTUM_AUTOSCROLL_SPEED=40000 target/release/libertum`. The per-second `scroll frames, last second` lines and the exit summary must stay flat from the first page to the last (2026-11-21, release build: mean 15.58 ms, p95 19.31 ms, `draw()` 0.05 ms); a rise with depth means a page lookup became linear again.

**Screenshot recipe (Windows, PowerShell):**
1. Call `user32!SetProcessDPIAware`, then start the release exe and wait about 6 s.
2. Call `SetForegroundWindow` on its main window.
3. Read the window bounds with `dwmapi!DwmGetWindowAttribute(hwnd, 9 /*EXTENDED_FRAME_BOUNDS*/, out RECT, 16)`.
4. Capture with `System.Drawing.Graphics.CopyFromScreen`, save as PNG, then inspect it.

Always look at screenshots after UI changes, in both themes.
