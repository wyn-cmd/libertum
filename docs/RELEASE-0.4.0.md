# Libertum 0.4.0

## Features

- **Built-in benchmark.** Settings > Benchmark measures scrolling, typing and saving on generated documents from 20 to 12,000 pages, finds the largest size that holds 60, 90 and 120 fps, saves the ten newest reports, and can show the last one later.
- **Live tables of contents in DOCX and ODT.** A Word or LibreOffice contents list opens, is rebuilt from the headings and re-saves as a real Word content-control TOC or ODT index, with links, page-number fields and `TOC1`-`TOC9` styles.
- **Word headings from styles.** Heading levels stored in a paragraph style are now detected, so inserted contents lists and the Headers pane see Word's headings.
- **AO3 tag suggestions from the chapter.** The chapter window suggests characters, romantic or platonic pairings and recurring keywords from the text, with the evidence for each guess, and leaves ratings and warnings to AO3's own pickers.
- **AO3 tag and title copying.** One **Copy tags** button writes the ticked tags, ratings and warnings show **Choose on AO3**, and a **Copy title** button copies the chapter heading.
- **New zoom controls.** The toolbar uses small minus and plus buttons around an editable percentage, with the Zoom menu under the middle button.
- **Linux window identity.** The window sets the `libertum` application id, so the dock and taskbar can match it to the desktop entry and icon.

## Changes

- Memory at rest for the 500-page demo is about 278-287 MB on Linux, down from about 830 MB.
- The spellcheck dictionary is built once and only the active language is kept, instead of one copy per worker thread.
- Layout shaping uses at most four threads, and fewer on small machines; an explicit count is still available for measurement.
- The benchmark and screenshot hooks are documented in `CONTRIBUTING.md`.
- Releases are built and published by `.github/workflows/release.yml` from a `v<version>` tag, with notes from this file and a generated `SHA256SUMS`.

## Fixes

- Word and LibreOffice tables of contents no longer stop a document from opening.
- Inserting a contents list into a Word-style document no longer produces an empty list.
- Spellcheck startup no longer spikes to about ten busy cores and a gigabyte of memory.
- Linux packages, the AppImage and the macOS bundle show the Libertum icon in the dock and taskbar.
- `libc` is only linked on Linux with glibc.

## Packaging

- Windows: `Libertum-Setup.exe` and `Libertum-Setup-0.4.0.exe`.
- Linux: `.deb`, `.rpm` and `.AppImage`.
- macOS: `.dmg` and `.zip` for Apple Silicon and Intel.
- `SHA256SUMS` covers every attached file.

## Notes

- Builds are unsigned. Windows SmartScreen may warn on first run; on macOS, right-click the app and choose **Open**.
- Updating a table of contents inside Word or LibreOffice has not been checked on this machine; the files follow the structure both programs write.
- Pasting the AO3 export into AO3's editor has not been checked with an account.
- The benchmark result depends on the display, window size and what else the computer is doing.
