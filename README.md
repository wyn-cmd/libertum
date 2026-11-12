# Libertum

[![CI](https://github.com/wyn-cmd/libertum/actions/workflows/ci.yml/badge.svg)](https://github.com/wyn-cmd/libertum/actions/workflows/ci.yml)

A cross-platform desktop word processor for long documents — novels, mostly — built in Rust.
It edits ODT and DOCX natively, reads and writes RTF and Markdown, and has its own lossless
format for documents that need everything the interchange formats cannot hold.

The interface follows Pages: the document sits on the left, a collapsible inspector on the
right, and the Format and Document buttons switch between them.

## Status

Working today:

- **Editing**: caret, selection (mouse, keyboard, double and triple click), undo and redo,
  plain-text clipboard, IME input.
- **Formatting**: character and paragraph formatting, paragraph styles, lists, borders, tabs,
  columns, hyphenation and ligatures, all bound to the selection with mixed states.
- **Pages**: page size, orientation, margins, per-section geometry, headers and footers (first
  and even variants, `{page}` and `{pages}`), page numbers, text watermarks, booklet view,
  page thumbnails, full screen, zoom, and a **pageless** reading view that shows the document
  as one endless A4-wide column.
- **Long documents**: an automatically updated table of contents, footnotes and endnotes,
  bookmarks and cross-references, inline images, and tables that split across pages with
  repeating header rows and editable merged cells.
- **Files**: ODT, DOCX, RTF and Markdown, plus the native `.lbtd` format. Find and replace,
  autosave with crash recovery, recent files, drag and drop, right-click menus and a Settings
  window.

`docs/STATUS.md` has the current detail and the open items. `PLAN.md` has the architecture,
the performance budgets and the phases: document stitching is done, and the writing tools still
to come are AO3 chapter export and proofreading.

## Install on Windows

Download `Libertum-Setup.exe` from the repository's **Releases** page (or a CI run's
`libertum-x86_64-pc-windows-msvc` artifact, or build it as below) and double-click it. It installs for the current user, with no
administrator rights: the program goes in `%LOCALAPPDATA%\Libertum`, with Start menu and
desktop shortcuts, and Libertum is offered in "Open with" (and Settings > Apps > Default apps)
for `.lbtd`, `.odt`, `.docx`, `.rtf` and `.md` files. Running it again updates an installed
copy; close Libertum first. An uninstaller, `Libertum-Uninstall.ps1`, is written beside the
program.

The installer and the program are not code-signed, so Windows SmartScreen may warn the first
time: choose "More info", then "Run anyway". Double-clicking a document that Libertum is set to
open starts it straight in that document.

## Install on Linux

The Linux build is shipped three ways, all unsigned: a `.deb`, an `.rpm`, and the plain
`libertum` program inside `Libertum-<version>-linux-x64.zip`. A release has the packages
attached; any green CI run has them in its `libertum-x86_64-unknown-linux-gnu` artifact.

```sh
sudo apt install ./libertum_0.1.1_amd64.deb                  # Debian, Ubuntu, ...
sudo dnf install ./libertum-0.1.1-1.x86_64.rpm               # Fedora, openSUSE, ...
```

The package puts `libertum` in `/usr/bin` and installs the desktop entry, the icon and a
shared-mime-info entry for `.lbtd` files, so Libertum appears in the application menu and in
"Open with". `sudo apt remove libertum` (or `sudo dnf remove libertum`) takes it away again.

The packages are built on Ubuntu 24.04, so they need glibc 2.39 or newer: Debian 13, Ubuntu
24.04, Fedora 41 and later. Older distributions can run a build of their own (see below). The
runtime libraries are declared as dependencies — fontconfig, X11, Wayland, `libxkbcommon`,
Vulkan and EGL/GLES — because the fonts, the window and the compositor are chosen at run time.

On macOS the release has the `libertum` program itself, unsigned and without an installer: run
it from a terminal (on macOS, the first time, right-click it and choose Open).

To build the installer: `powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1`
(it writes `dist\Libertum-Setup-<version>.exe`). The same install can be done by hand by running
`packaging\windows\install.ps1` from a folder that holds `libertum.exe`.

## Build and run

Rust stable (developed on 1.99, edition 2024). On Windows the toolchain is MSVC.

```sh
cargo run --release -p libertum-app          # the binary is named `libertum`
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
packaging/linux/build-packages.sh             # dist/libertum_<version>_amd64.deb and .rpm
```

The release profile matters for anything you judge by feel: a debug build pages far slower, and
the performance budgets in `PLAN.md` §2 are all release numbers.

### Frame-rate counter

A build-time counter that draws the current frame rate and frame time in the top-right corner:

```sh
cargo run --release -p libertum-app --features fps
```

It is off by default and is not part of any official build, so release and CI binaries carry
neither the counter nor the continuous redraw it needs. While it is on, the window redraws
every frame; compare runs at the same zoom, window size and document.

### Debug hooks

A few environment variables help with screenshots and measurements:

| Variable | Effect |
|---|---|
| `LIBERTUM_OPEN=<path>` | Start with that document instead of the home screen |
| `LIBERTUM_DEMO=1` | Open the generated 500-page demo document |
| `LIBERTUM_THEME=light\|dark` | Force a theme |
| `LIBERTUM_PERF=1` | Log frame and update timings |
| `LIBERTUM_AUTOSCROLL=1` | Scroll to the last page and exit (a scrolling benchmark) |

The full list is in `AGENTS.md`.

## Repository layout

| Path | What it is |
|---|---|
| `crates/libertum-model` | Document tree, operations with exact inverses, undo/redo, styles, tables |
| `crates/libertum-layout` | Text shaping (parley), line breaking, pagination |
| `crates/libertum-render` | Page rendering: tiny-skia on the CPU, optionally vello on the GPU |
| `crates/libertum-editor` | Cursor, selection, typing, formatting commands, find and replace |
| `crates/libertum-docx`, `-odt`, `-rtf`, `-markdown`, `-native` | Format import and export |
| `crates/libertum-formats-common` | Code the format crates share |
| `crates/libertum-app` | The Iced application: window, canvas, inspector, toolbar |
| `docs/` | Current status, decisions, measurements, the native format specification, accessibility notes |
| `tests/fixtures/` | Round-trip fixtures for every format |

## Notes on the code

- Every document change is an `Operation` with an exact inverse, and the model tests check each
  one with a property test. Undo is the same machinery running backwards.
- The keystroke path is kept independent of document size: layout is cached per paragraph,
  pagination is incremental, and only visible pages are rendered.
- Third-party versions are pinned together on purpose (iced 0.14, wgpu 27, vello 0.7); see
  `PLAN.md` §4.5 before bumping any of them.

## Licence

Private project; no licence has been chosen yet. Dependencies and embedded data are listed with
their licences in `docs/third-party-licences.md`.
