# Libertum

[![CI](https://github.com/wyn-cmd/libertum/actions/workflows/ci.yml/badge.svg)](https://github.com/wyn-cmd/libertum/actions/workflows/ci.yml)

Libertum is a desktop word processor for long manuscripts. It runs on Windows, macOS and
Linux, and it is written in Rust.

It opens and saves ODT and DOCX directly, reads and writes RTF, Markdown and HTML, imports EPUB
and AO3 downloads, and keeps its own format (`.lbtd`) for documents that need more than the
interchange formats can hold. The window
follows Pages: the page sits on the left and the inspector on the right, with the Format and
Document buttons switching between the formatting and document panes.

The largest test document is a 343-page novel with footnotes, images, a table of contents and
cross-references.

## What works

- Editing: caret and selection by mouse or keyboard, undo and redo, plain-text clipboard, IME
  input.
- Formatting: character and paragraph formatting, paragraph styles, lists, borders, tab stops,
  columns, hyphenation and ligatures. Changes apply to the selection, and a mixed selection
  shows as mixed.
- Pages: page size, orientation, margins, per-section layout, headers and footers with first
  page and even page variants, page numbers, text watermarks, booklet view, page thumbnails,
  full screen and zoom, including a trackpad pinch on Wayland and macOS. There is also a
  pageless mode that shows the document as one long column.
- Page colour: White, nine soft tints or any hex colour, shown in both themes and kept in
  Libertum, Word, OpenDocument, HTML and PDF files.
- Long documents: a table of contents that updates itself, footnotes and endnotes, bookmarks
  and cross-references, inline images, and tables that split across pages with repeating header
  rows and editable merged cells.
- Files: ODT, DOCX, RTF, Markdown, HTML and `.lbtd`, plus EPUB and AO3 downloads imported as
  new documents, with find and replace, autosave and crash recovery, recent files, drag and
  drop, right-click menus, a settings window, version history and a library view.
- Chapters: **Insert > Chapter from File…** and **Chapter from Copied HTML** add a chapter
  after the caret as one undo step.
- Templates: Blank, Manuscript and Book are built in. Add your own from an ODT, DOCX or
  Libertum (`.lbtd`) document on the home screen; the template is a copy in Libertum's data
  folder, and removing it leaves the original alone.
- Export: PDF with searchable text, PDF/A-2b, EPUB 3, RTF, plain text, Markdown and plain
  HTML. A chapter can also be exported as HTML for AO3, with full names, repeated phrases and
  tags learned in a local library.
- Benchmark: **Settings > Benchmark** measures scrolling, typing and saving on generated
  documents of growing size, finds the largest size the computer holds at 60, 90 or 120 fps,
  and keeps the last ten results.
- Tables of contents: opens a Word or LibreOffice contents list, rebuilds it from the headings,
  and saves its own as a live Word content-control TOC or ODT index.
- AO3 export: suggests characters, pairings and keywords from the chapter's own text, and
  copies a chapter title or the ticked tags. **Mark as Posted** keeps a local copy of a
  chapter, and **View > Compare with Posted** shows what changed since then, with Keep and
  Revert to Posted for each change.
- Updates: asks GitHub at most once a day whether a newer release exists. **Update now**
  downloads the right build, checks it against `SHA256SUMS` and installs it by itself
  (Windows installer, AppImage, macOS app, or a `.deb`/`.rpm` through the desktop's own
  administrator prompt); only a failed check falls back to the release page. The check can
  be turned off in Settings.

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

A release includes a `.deb`, an `.rpm`, an AppImage and a zip with the plain binary. They are
all unsigned.

```sh
sudo apt install ./libertum_0.5.1_amd64.deb                  # Debian, Ubuntu, ...
sudo dnf install ./libertum-0.5.1-1.x86_64.rpm               # Fedora, openSUSE, ...
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
chmod +x Libertum-0.5.1-x86_64.AppImage
./Libertum-0.5.1-x86_64.AppImage
```

Pinch-to-zoom needs a Wayland session. On X11, Ctrl + two-finger scroll zooms.
Update checks use the system `curl` (the packages recommend it); without it the rest of
the app is unaffected and updates stay manual.

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
Pushing a `v<version>` tag runs `.github/workflows/release.yml`, which builds the platform
bundles, writes `SHA256SUMS` and publishes the release notes from `docs/RELEASE-<version>.md`.
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

The full list is in `CONTRIBUTING.md`.

## Repository layout

| Path | What it is |
|---|---|
| `crates/libertum-model` | Document tree, operations with exact inverses, undo/redo, styles, tables |
| `crates/libertum-layout` | Text shaping (parley), line breaking, pagination |
| `crates/libertum-render` | Page rendering: tiny-skia on the CPU, optionally vello on the GPU |
| `crates/libertum-editor` | Cursor, selection, typing, formatting commands, find and replace |
| `crates/libertum-docx`, `-odt`, `-rtf`, `-markdown`, `-html`, `-native` | Format import and export |
| `crates/libertum-diff` | Paragraph and word comparison for Compare with Posted |
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

## Acknowledgements

Libertum embeds data and libraries that carry their own licences and notices. The full list,
with the licence texts, is in `docs/third-party-licences.md`. The main ones:

- **Hyphenation patterns**, from the hyph-utf8 package through the `hypher` crate (MIT or
  Apache-2.0, by the Typst authors): English (US) by Gerard D.C. Kuiken (all-permissive
  notice) and Ukrainian by Maksym Polyakov (MIT, 1998-2001). The patterns are used unmodified.
- **Spelling**: the `spellbook` crate (MPL-2.0) with the bundled Hunspell dictionaries for
  British and American English, Ukrainian (MPL 1.1) and Russian (BSD-style). Each
  dictionary's licence is in `crates/libertum-app/assets/dictionaries/`.
- **East Asian word boundaries**: ICU4X `icu_segmenter` and its dictionary data
  (Unicode-3.0).
- **Icons**: Lucide (ISC).

## Licence

No licence has been chosen yet. The source is public, but that is not a grant to reuse it.
Dependencies and embedded data are listed with their licences in
`docs/third-party-licences.md`.
