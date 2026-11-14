# Libertum

[![CI](https://github.com/wyn-cmd/libertum/actions/workflows/ci.yml/badge.svg)](https://github.com/wyn-cmd/libertum/actions/workflows/ci.yml)

Libertum is a desktop word processor for long manuscripts. It runs on Windows, macOS and
Linux, and it is written in Rust.

It opens and saves ODT and DOCX directly, reads and writes RTF and Markdown, and keeps its own
format (`.lbtd`) for documents that need more than the interchange formats can hold. The window
follows Pages: the page sits on the left and the inspector on the right, with the Format and
Document buttons switching between the formatting and document panes.

The main acceptance document is a 343-page novel with footnotes, images, a table of contents
and cross-references, so long documents are the point rather than an afterthought.

## What works

- Editing: caret and selection by mouse or keyboard, undo and redo, plain-text clipboard, IME
  input.
- Formatting: character and paragraph formatting, paragraph styles, lists, borders, tab stops,
  columns, hyphenation and ligatures. Changes apply to the selection, and mixed states show as
  mixed.
- Pages: page size, orientation, margins, per-section layout, headers and footers with first
  page and even page variants, page numbers, text watermarks, booklet view, page thumbnails,
  full screen and zoom. There is also a pageless mode that shows the document as one long
  column.
- Long documents: a table of contents that updates itself, footnotes and endnotes, bookmarks
  and cross-references, inline images, and tables that split across pages with repeating header
  rows and editable merged cells.
- Files: ODT, DOCX, RTF, Markdown and `.lbtd`, with find and replace, autosave and crash
  recovery, recent files, drag and drop, right-click menus, a settings window, version history
  and a library view.
- Export: PDF with searchable text, PDF/A-2b, EPUB 3, RTF, plain text and Markdown. A chapter
  can also be exported as HTML for AO3, with tag suggestions ranked locally from your own tag
  list.

`docs/STATUS.md` has the current state and the open items. `PLAN.md` covers the architecture,
the performance budgets and the phase list. Of the planned work, the main pieces still missing
are proofreading and AO3 sync.

## Install on Windows

Download `Libertum-Setup.exe` from the [Releases](https://github.com/wyn-cmd/libertum/releases)
page, or take it from a CI artifact, or build it yourself. Double-click it and it installs for
the current user, with no administrator rights. The program goes in `%LOCALAPPDATA%\Libertum`,
with Start menu and desktop shortcuts, and Libertum is registered in "Open with" for `.lbtd`,
`.odt`, `.docx`, `.rtf` and `.md`. Running the installer again updates an installed copy, so
close Libertum first. It also writes `Libertum-Uninstall.ps1` beside the program.

Nothing is code-signed, so SmartScreen may warn you the first time. Choose "More info", then
"Run anyway". Once Libertum is the default handler for a document type, double-clicking one of
those files opens it straight in the editor.

## Install on Linux

A release includes a `.deb`, an `.rpm`, an AppImage and a zip with the plain binary. None of
them are signed.

```sh
sudo apt install ./libertum_0.1.2_amd64.deb                  # Debian, Ubuntu, ...
sudo dnf install ./libertum-0.1.2-1.x86_64.rpm               # Fedora, openSUSE, ...
```

The packages put `libertum` in `/usr/bin` and install a desktop entry, the icon and a MIME
entry for `.lbtd` files, so the app appears in the application menu and in "Open with". Remove
it with `sudo apt remove libertum` or `sudo dnf remove libertum`.

They are built on Ubuntu 24.04 and need glibc 2.39 or newer: Debian 13, Ubuntu 24.04, Fedora 41
and later. The packages depend on fontconfig, X11, Wayland, `libxkbcommon`, Vulkan and EGL/GLES
because the fonts, the window system and the compositor are chosen at run time. If your
distribution is older, build Libertum from source.

The AppImage carries its own font, input, Wayland and graphics libraries, and only needs the
host's X11/XCB and glibc:

```sh
chmod +x Libertum-0.1.2-x86_64.AppImage
./Libertum-0.1.2-x86_64.AppImage
```

## Install on macOS

Download `Libertum-<version>-macos-arm64.dmg` for Apple Silicon or
`Libertum-<version>-macos-x64.dmg` for Intel, open the disk image and drag **Libertum** to
Applications. The app is signed ad hoc rather than notarised, so Gatekeeper will complain the
first time: right-click Libertum in Finder, choose **Open**, then **Open** again. It handles
`.lbtd`, `.odt`, `.docx`, `.rtf` and `.md` from Finder.

## Building from source

Rust stable. The workspace declares 1.92, CI builds with stable, and development happens on
1.100. On Windows the toolchain is MSVC.

```sh
cargo run --release -p libertum-app          # the binary is named `libertum`
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
packaging/linux/build-packages.sh             # dist/libertum_<version>_amd64.deb and .rpm
```

Use a release build for anything you judge by feel. A debug build pages much more slowly, and
the performance budgets in `PLAN.md` §2 are all release numbers.

The Windows installer is packed with `powershell -ExecutionPolicy Bypass -File
packaging\windows\build-installer.ps1`, which writes `dist\Libertum-Setup-<version>.exe`.
`packaging\windows\install.ps1` performs the same install by hand from a folder that holds
`libertum.exe`. The Linux packages and the AppImage have scripts under `packaging/linux/`; the
macOS bundle and disk image are built by `packaging/macos/build-app.sh` on a Mac.

### Frame times

Settings has a **Show performance overlay** switch that draws frame rate and frame time in the
top-right corner while it is on. It is off by default. The window redraws every frame while the
overlay is shown, so turn it off after measuring.

### Environment variables

A few variables are handy for screenshots and measurements:

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
| `crates/libertum-export` | Chapter splitting, AO3 HTML, EPUB and plain text |
| `crates/libertum-library` | Atomic saves, change detection and version history on disk |
| `crates/libertum-formats-common` | Code the format crates share |
| `crates/libertum-installer` | The single-file Windows installer |
| `crates/libertum-app` | The Iced application: window, canvas, inspector, toolbar |
| `docs/` | Current status, decisions, measurements, the native format specification, accessibility notes |
| `tests/fixtures/` | Round-trip fixtures for every format |

## Notes on the code

- Every document change is an `Operation` with an exact inverse, and the model tests check each
  one with a property test. Undo is the same machinery running backwards.
- Nothing on the keystroke path may grow with document size. Layout is cached per paragraph,
  pagination is incremental, and only visible pages are rendered.
- Third-party versions are pinned together on purpose (iced 0.14, wgpu 27, vello 0.7). Read
  `PLAN.md` §4.5 before bumping any of them.

## Licence

No licence has been chosen yet. The source is public, but that is not a grant to reuse it.
Dependencies and embedded data are listed with their licences in
`docs/third-party-licences.md`.
