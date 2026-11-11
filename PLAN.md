# Libertum — Implementation Plan

Libertum is a cross-platform (Windows, macOS, Linux) desktop word processor written in Rust. It is built for long-form writing, such as books and documents of several hundred pages, and must stay responsive on modest hardware. It edits DOCX and ODT natively and has a WYSIWYG Markdown mode. The interface follows Apple Pages: the document canvas sits on the left and a collapsible formatting sidebar on the right.

This is the working plan for the application, phase by phase. Read the whole document before starting. Each phase lists acceptance criteria, and a phase is not finished until they pass.

---

## 1. Decisions already made

| Topic | Decision |
|---|---|
| Language | Rust (stable toolchain, edition 2021 or later) |
| GUI toolkit | **Iced** (latest stable release at the time of implementation) |
| Text shaping and layout | **parley** (Linebender), with **swash** for glyph rasterisation |
| Rendering of page content | CPU rasterisation per page (tiny-skia + swash) into cached bitmaps shown inside Iced, as the default. A user setting, **Graphics acceleration**, switches to GPU rendering (§4.5) |
| Default format for new documents | **ODT**. The user can change the default to DOCX in Settings |
| DOCX/ODT support (v1) | Core text, page layout, and book features (§6). Comments and tracked changes are out of scope for v1 but must be **preserved** on save |
| Markdown mode | WYSIWYG, using the same editor as DOCX. Unedited blocks keep their original source text on save (§7) |
| Minimum hardware | 4 GB RAM, older integrated GPU (around 2015 dual-core laptop) |
| Packaging | GitHub Actions matrix build, done last (Phase 9) |

### Assumptions (flag to the user if any turn out wrong)

- There is no proprietary native format. DOCX, ODT and Markdown are first-class formats. A new document defaults to ODT, and Settings can change that to DOCX.
- PDF export, spellcheck and printing are post-v1, except where a phase lists them as optional.
- The licence is not yet chosen. Do not add GPL-licensed dependencies without asking, and prefer MIT/Apache-2.0 crates.
- English UI only for v1, but the code must stay localisable: no user-visible strings scattered through logic.

---

## 2. Performance budgets (non-negotiable)

These budgets are measured on the minimum-spec machine or an equivalent throttled VM. A test corpus of generated 500-page and 1,000-page documents lives in `bench/corpus/` (generator script in `tools/`).

| Metric | Budget |
|---|---|
| Keystroke to pixels on screen (typing in the middle of a 500-page doc) | < 16 ms at p95 |
| Opening a 500-page DOCX: first page visible | < 1.5 s |
| Opening a 500-page DOCX: full pagination complete (in the background) | < 8 s |
| Scrolling through the document | 60 fps sustained, no frames > 33 ms |
| Resident memory, 500-page text-only document | < 300 MB |
| Idle CPU with the document open | ~0% (no busy redraw loop) |
| Save of a 500-page DOCX | < 2 s, off the UI thread |

These budgets drive the architecture: **nothing may be O(document size) on the keystroke path.** Edits, layout, pagination and painting must all be incremental or limited to the viewport.

Add `criterion` benchmarks for editing, layout and pagination from Phase 1, and run them in CI with a regression threshold from Phase 9.

---

## 3. Workspace layout

Use a Cargo workspace. Keep the crate boundaries strict, because format crates must never depend on UI crates.

```
libertum/
├─ Cargo.toml                 (workspace)
├─ crates/
│  ├─ libertum-model/         Document tree, styles, operations, undo/redo. No I/O, no UI.
│  ├─ libertum-layout/        parley shaping, line breaking, pagination, layout caches.
│  ├─ libertum-render/        Page rasterisation (tiny-skia + swash), bitmap cache.
│  ├─ libertum-docx/          DOCX import/export (zip + quick-xml).
│  ├─ libertum-odt/           ODT import/export (zip + quick-xml).
│  ├─ libertum-rtf/           RTF import/export (plain-text token stream).
│  ├─ libertum-markdown/      Markdown import/export with source-span preservation.
│  ├─ libertum-editor/        Cursor, selection, commands, input handling (UI-agnostic).
│  └─ libertum-app/           Iced application: window, toolbar, canvas widget, sidebar.
├─ bench/                     criterion benches and generated corpus
├─ tests/fixtures/            real-world .docx/.odt/.md files for round-trip tests
├─ tools/                     corpus generator, fixture utilities
└─ .github/workflows/         CI and release (Phase 9)
```

### Candidate dependencies

Check each crate's current version and API on docs.rs before use. **Do not rely on remembered APIs.** Iced and parley in particular change between releases.

- UI: `iced`, `rfd` (native file dialogs), `arboard` (clipboard)
- Text: `parley`, `swash`, `fontique` (font discovery, parley's companion), `unicode-segmentation`
- Rendering: `tiny-skia` (CPU path), `vello` (GPU path, Linebender's GPU 2D renderer that pairs with parley; must be wgpu-version-compatible with Iced, see §4.5)
- Formats: `zip`, `quick-xml`, `comrak` (CommonMark + GFM, with source positions), `image`; for RTF code pages, `encoding_rs` (Apache-2.0 OR MIT) rather than hand-written tables
- Data structures: `slotmap` (stable node IDs), `smallvec`. Consider `im` or a hand-rolled chunked B-tree for the block list (§4.1)
- Infrastructure: `thiserror`, `anyhow` (app crate only), `tracing`, `serde` + `toml` (settings), `directories` (config paths)
- Testing: `criterion`, `proptest`, `insta`

Evaluate the existing `docx-rs` crate in a spike, but default to a hand-written reader/writer. Lossless round-tripping of unknown XML is easier to guarantee with our own code.

---

## 4. Core architecture

### 4.1 Document model (`libertum-model`)

```
Document
 ├─ styles: StyleSheet            (paragraph, character, list/numbering, table styles; inheritance chain)
 ├─ sections: Vec<Section>        (page size, margins, orientation, columns, header/footer refs, page-number format)
 ├─ body: BlockList               (top-level blocks, with section boundaries marked)
 ├─ headers_footers: SlotMap<HfId, BlockList>
 ├─ notes: SlotMap<NoteId, Note>  (footnotes/endnotes, each owning a BlockList)
 ├─ media: SlotMap<MediaId, Media>(image bytes + metadata, decoded lazily)
 ├─ bookmarks, fields metadata
 └─ preserved: PreservedParts     (unknown package parts and opaque XML, see §6.4)

Block = Paragraph | Table | SectionBreak | PageBreak | TocField | Opaque
Paragraph { id: BlockId, style: StyleRef, props: ParaProps (direct formatting), text: String, runs: RunList, inlines: Vec<InlineObject>, version: u64 }
RunList = sorted, non-overlapping spans of (byte_range, CharStyleRef, CharProps)
InlineObject = FootnoteRef | Image | Bookmark | Field (page number, cross-ref) | Tab | LineBreak | Opaque, anchored at a byte offset
```

Key rules:

- **Paragraph-local text.** Each paragraph owns its own `String`. Edits touch one paragraph (or a few when splitting or merging), never a whole-document buffer.
- **Block list must be O(log n) for index, insert and delete.** A `Vec<BlockId>` is fine up to roughly 20k blocks. Put it behind a `BlockList` trait so it can be swapped for a chunked B-tree or `im::Vector` if benchmarks demand it.
- **Stable IDs.** `BlockId` comes from a `SlotMap` and never changes for the life of a block. Layout caches are keyed by `(BlockId, version)`.
- **Positions.** `DocPosition { block: BlockId, offset: usize /* byte offset, always on a grapheme boundary */ }`. Never store absolute document offsets.
- **Operations.** All mutations go through `Operation` (InsertText, DeleteRange, SplitBlock, MergeBlocks, SetCharProps, SetParaProps, InsertBlock, RemoveBlock, …). Each operation returns its inverse. `Transaction` groups operations. Undo/redo is a stack of transactions. Coalesce consecutive typing into a single undo step (break on whitespace, cursor jump, or a 1 s pause).
- **Change notification.** Applying a transaction yields a `ChangeSet` (dirty block IDs plus structural changes) that layout consumes.
- **Style resolution.** Resolved properties are computed as document defaults → paragraph style chain → character style chain → direct formatting. Cache resolved styles by `(style ref, direct props hash)`.

### 4.2 Layout (`libertum-layout`)

Two levels:

1. **Paragraph layout.** A paragraph is shaped with parley at a given content width, producing lines with heights and baselines. Cache it in an LRU keyed by `(BlockId, version, width, style_epoch)`. Only dirty paragraphs are re-shaped. Inline images and footnote references use parley's inline box support. Verify this exists in the current parley version during the Phase 0 spike.
2. **Pagination.** Flow paragraph lines into pages according to the section's page geometry. The pass handles:
   - widow/orphan control, keep-with-next, keep-lines-together, and page-break-before
   - footnotes, which reserve space at the bottom of the page where their reference lands (iterate until stable, with a hard iteration cap)
   - headers and footers, laid out per page with field substitution (page number, total pages)
   - columns, flowed column-major within a page
   - tables: rows are the pagination unit, and a row may split across pages when allowed; repeat header rows

**Incremental pagination.** Store `PageMap: Vec<PageInfo { first: (BlockId, line), last: (BlockId, line), y_offsets… }>`. After an edit to block *B*, re-paginate starting at the page containing *B*. Stop as soon as a page break lands on the same `(BlockId, line)` as before and the block versions after it are unchanged (convergence). When an edit keeps a paragraph's line count the same, typing re-paginates one or two pages. When it adds or removes a line, the page breaks after it can shift for hundreds of pages until one page has slack. Spike A measured 187 pages re-broken in under 3 ms, so the pagination walk is cheap. The real cost is that every shifted page gets a new content hash. Only the visible shifted pages may be re-rasterised; off-screen pages are re-rendered lazily when scrolled into view.

**Background pagination on open.** Paginate the first screenful synchronously, then the rest on a worker thread, sending `PageMap` chunks to the UI. Until it finishes, the scrollbar uses an estimated page count (from the average characters per page), and the status bar shows "Paginating…".

**Threading model.** The model is owned by the UI thread. The worker receives immutable snapshots: cheap clones via `Arc` paragraphs, so `Paragraph` storage should be `Arc<Paragraph>` with copy-on-write. Layout results are cached in a shared structure keyed by version, so stale results are discarded.

### 4.3 Editor (`libertum-editor`)

This crate is UI-agnostic and testable without a window.

- Cursor and selection: anchor plus head `DocPosition`, plus a "preferred x" for up/down movement.
- Movement: by grapheme, word, line (via layout), paragraph, and page; Home/End; document start and end.
- Commands: type, delete, backspace, word delete, cut/copy/paste (rich internal format plus plain text; paste DOCX/HTML as plain text in v1 and record this as a known limitation), select all, undo/redo, toggle bold/italic/underline/strike, apply styles, and the list operations.
- Hit-testing: map a point in page coordinates to a `DocPosition` using cached line layouts.
- Find and replace: search per paragraph with an incremental results list, and never block the UI. Use a worker for whole-document searches over about 200 pages.
- IME: composition text must render inline with an underline. **Verify Iced's current IME support in Phase 0.** If it is missing or incomplete, record it as a blocking risk and report to the user before continuing.

### 4.4 Rendering (`libertum-render` + canvas widget in `libertum-app`)

- The document canvas is a custom Iced widget that shows a vertical strip of pages on a neutral grey background (as in Pages), with zoom (50–400%, plus fit-width).
- **Only visible pages, plus one page above and below, are rasterised.** Each page is rasterised with tiny-skia, and glyphs are drawn with swash from parley glyph runs. The bitmap is cached in an LRU keyed by `(page index, page content hash, zoom, scale factor)`. Memory cap: about 64 MB of page bitmaps, with the oldest evicted first.
- While typing, re-rasterise only the dirty line rectangles of the affected page, not the whole page.
- The cursor, selection highlight and IME composition are drawn as overlay primitives by Iced, not baked into the bitmap, so blinking the cursor never re-rasterises.
- Hi-DPI: rasterise at the window scale factor.
- Why CPU rasterisation: it behaves the same under Iced's wgpu and tiny-skia backends, sidesteps weak or buggy old iGPU drivers, and decouples us from Iced's internal text pipeline (which uses cosmic-text, not parley). **Phase 0 must validate this approach with a spike.** The fallback, if the spike fails the budgets, is to render with Iced's own text primitives via cosmic-text and to record the trade-offs.

### 4.5 Graphics acceleration (user setting)

Rendering has two layers (decided after the Phase 0 measurements in docs/spikes.md):

- **Window compositor: always the GPU when one exists.** Iced's wgpu compositor held 60 fps while scrolling 506 pages. Its tiny-skia CPU compositor measured about 15 fps, because of Iced's own CPU cost of roughly 25 ms per frame, before any pages are drawn. tiny-skia is used only as an automatic last-resort fallback when wgpu cannot start, and it is documented as degraded.
- **Page renderer: the Graphics acceleration toggle** in Settings → General. It is off by default, because the minimum-spec machines have weak or unreliable older GPUs.
  - **Off (CPU):** pages are rasterised with tiny-skia + swash on a worker thread (§4.4) and uploaded as image textures.
  - **On (GPU):** pages are drawn with vello from the same parley glyph runs, inside an Iced `shader` widget that shares Iced's wgpu device (see docs/spikes.md, Spike D). Pages are redrawn from cached vello scenes per page, and only visible pages are encoded.
- Put both page renderers behind one `PageRenderer` trait in `libertum-render`, so layout and editor code never know which one is active.
- Iced picks its renderer at startup, so changing the toggle says "takes effect after restart". If the page renderer can be switched live, do it, but the shell may still need the restart. Check Iced's current mechanism for choosing a renderer (feature flags, environment variable or settings) on docs.rs rather than assuming one.
- **Safe fallback:** if GPU initialisation fails, or the first frame errors, fall back to CPU automatically. Save the setting as off and show a one-time notice. Write a "GPU start in progress" flag before initialising the GPU and clear it after the first successful frame. If the flag is still set on the next launch (a crash), start in CPU mode and say why.
- vello and Iced must use compatible `wgpu` versions. If they can't be aligned, vello can render to an offscreen texture that is then copied into an Iced image, at some cost. Record which approach was used in `docs/decisions.md`.
- Both renderers must produce visually matching pages. Add a test that renders the same fixture pages through both and compares them within a small pixel tolerance.
- The performance budgets in §2 apply to both modes. GPU mode should make zoom and scroll smoother on capable machines, but must never be required.

---

## 5. User interface (`libertum-app`)

Visual reference: Apple Pages. It should be clean, roomy, and use neutral greys. Follow the OS light/dark preference, with a manual override.

```
┌──────────────────────────────────────────────────────────────────────────┐
│ [View▾] [Page Break] [Zoom 125%▾] │ [Insert▾] [Table] [Image] [Footnote] │ [Format ▣] │  ← toolbar
├───────────────────────────────────────────────────┬──────────────────────┤
│                                                   │ Format │ Document    │  ← sidebar tabs
│        ┌─────────────────────────┐                │──────────────────────│
│        │                         │                │ Paragraph style ▾    │
│        │     page (white)        │                │ ─ Font ─────────────  │
│        │                         │                │ [Family ▾] [Weight▾] │
│        │                         │                │ [B][I][U][S]  [12 ▾] │
│        └─────────────────────────┘                │ Colour ■  Highlight ■ │
│        ┌─────────────────────────┐                │ ─ Alignment ────────  │
│        │                         │                │ [≡][≡][≡][≡]          │
│                  (scrolls)                        │ ─ Character Colour ── │
│                                                   │ ─ Bullets & Lists ──  │
├───────────────────────────────────────────────────┴──────────────────────┤
│ Page 12 of 486 · 104,233 words · DOCX · Saved                            │  ← status bar
└──────────────────────────────────────────────────────────────────────────┘
```

**Pageless view.** The canvas can show the document either as pages or as one endless column (View > Pageless). In the pageless view there is no page gap or shadow and no page numbers, headers or footers, and the column is as wide as the document's own paper (A4 for an A4 document) with its side margins, so the body reads as one long sheet. Markdown and RTF open that way; DOCX and ODT open as pages and can be switched deliberately. The choice belongs to the view, not the file: no interchange format can store it (the proposed native format would, in `view.json`).

**Toolbar:** View (show/hide sidebar, invisibles, page thumbnails later), Page Break, Zoom (with its exact-percentage stepper), Insert (page break, section break, table, image, footnote, endnote, table of contents, page number), and a Format toggle on the right that collapses or expands the sidebar.

**Right sidebar (about 280 px, collapsible):**
- The collapse toggle is the toolbar button plus `Ctrl/Cmd+Alt+I`. Collapse and expand animate in under 150 ms. The width and collapsed state persist in settings. When collapsed, the canvas takes the full width.
- **Format tab**, with collapsible groups:
  - Paragraph style picker (shows style names rendered in their own style, as Pages does)
  - Font: family, typeface/weight, size, bold, italic, underline, strikethrough, colour, highlight, superscript/subscript, character spacing
  - Alignment: left, centre, right, justify; indents
  - Character Colour: text colour and highlight swatches
  - Bullets & Lists: list type, level, numbering format
  - Spacing: line spacing, before, after
  - Advanced: keep-with-next, widow/orphan, page-break-before
- **Document tab:** page size, orientation, margins, header/footer on or off (first page different, odd/even different), columns, page-number format, section settings.
- Controls always reflect the selection. Mixed values (for example, a selection that spans two fonts) show an indeterminate state.
- In **Markdown mode**, controls with no Markdown equivalent (font family, colour, size, alignment, page layout) are hidden or disabled with a tooltip that explains why (§7).

**Status bar:** current page and total, word count (updated incrementally per paragraph, never by rescanning the whole document), file format, and save state.

**Keyboard shortcuts:** platform-standard. Use `Cmd` on macOS and `Ctrl` elsewhere, through a single shortcut map.

**Settings:** stored as TOML in the platform config directory (`directories` crate). Settings cover theme, default font, **default format for new documents (ODT, or DOCX)**, **graphics acceleration (off by default, §4.5)**, autosave interval, sidebar state, and recent files. A new document's first "Save" dialog preselects the default format, and the user can still pick another.

**Autosave and recovery:** every 60 s, and on focus loss, write a recovery snapshot to the platform data directory, never over the user's file. On launch, offer to restore after a crash.

---

## 6. File formats

### 6.1 Round-trip principle

**Anything Libertum does not understand must survive open → edit → save unchanged, as far as possible.** This is what makes the DOCX/ODT support "native" rather than "import".

A **native Libertum format** (`.lbtd`) that stores the model and the view state one-to-one is specified in `docs/format-libertum-native.md` and **implemented** in `crates/libertum-native` (2026-11-11): the app opens and saves it, and crash recovery uses it. ODT remains the built-in default format until the user chooses otherwise in Settings.

- Keep every unknown package part (comments, custom XML, themes, glossary, etc.) as raw bytes and write it back with its relationships intact.
- Unknown elements inside a paragraph or run become `InlineObject::Opaque` or carry opaque attributes, and are re-emitted in place.
- Unknown block-level elements become `Block::Opaque`, shown in the editor as a non-editable placeholder ("Unsupported content: …").
- Comments and tracked changes (v1, not editable): preserve their markup. If a user edits text inside a tracked-change range, show a warning once per document that the tracked change may be flattened.
- RTF has no package parts, so its preservation works differently: keep the raw token stream of every destination Libertum does not understand, including `\*` ignorable groups, and re-emit it in place (§6.5).

### 6.2 DOCX (`libertum-docx`)

Parse with `zip` + `quick-xml` (streaming reader, not a DOM, for memory). Parts to support:

- `[Content_Types].xml`, `_rels/.rels`, `word/_rels/document.xml.rels`
- `word/document.xml`: paragraphs, runs, `w:sectPr`, tables, drawings (inline images only in v1; anchored/floating images are preserved and shown as inline placeholders), field codes (`PAGE`, `NUMPAGES`, `TOC`, `REF`), bookmarks, breaks
- `word/styles.xml`: style inheritance (`w:basedOn`), defaults (`w:docDefaults`)
- `word/numbering.xml`: abstract numbering and instance overrides, mapped to list styles
- `word/footnotes.xml`, `word/endnotes.xml`
- `word/header*.xml`, `word/footer*.xml`
- `word/settings.xml` (read what affects layout; preserve the rest)
- `word/media/*`
- `docProps/*`: preserve, and update the modified time and app name on save

Writing must produce files that open cleanly in Microsoft Word, LibreOffice and Pages. A Word "repair" prompt counts as a bug.

### 6.3 ODT (`libertum-odt`)

- The `mimetype` entry must be the first entry in the zip and stored uncompressed (this is an ODF packaging requirement).
- Parts: `META-INF/manifest.xml`, `content.xml`, `styles.xml`, `meta.xml`, `settings.xml`, `Pictures/*`.
- Map `style:style` families (paragraph, text, table), automatic styles versus common styles, `text:list` / `text:list-style`, `text:note` (footnotes and endnotes), `style:master-page` + `style:page-layout` (page geometry, headers and footers), `text:table-of-content`, `text:bookmark`, `text:page-number`, and `draw:frame` / `draw:image`.
- On export, generate automatic styles for direct formatting and deduplicate them.

### 6.4 Shared format code

Put XML helpers, unit conversion (twips, EMU, half-points, cm/in/pt), and the preserved-parts store in a small module that both crates share. Either add a `libertum-formats-common` crate or place it in `libertum-model::interop`, whichever keeps the dependencies clean.

### 6.5 RTF (`libertum-rtf`)

RTF 1.9.1 is a plain-text format: a document is one stream of groups (`{ … }`), control words (`\word` with an optional numeric parameter), control symbols and text. It is worth supporting because every word processor writes it and it needs no zip or XML stack. It is also the natural "Word opens it with no repair prompt" fallback when a DOCX feature is not implemented yet.

- **Reader:** a byte-level tokeniser (groups, control words, `\*` ignorable destinations, text runs) that never builds a DOM. Map the control words Libertum models:
  - document: `\paperw`, `\paperh`, `\margl`, `\margr`, `\margt`, `\margb`, `\landscape`, `\sect`, `\cols` (once columns are modelled);
  - text: `\par`, `\line`, `\tab`, `\page`, `\sect`, `\b`, `\i`, `\ul`, `\strike`, `\super`, `\sub`, `\fs`, `\f`, `\cf`, `\highlight`, `\expnd`;
  - paragraphs: `\ql`/`\qc`/`\qr`/`\qj`, `\li`/`\ri`/`\fi`, `\sb`/`\sa`, `\sl`/`\slmult`, then the style sheet from `\stylesheet` and `\s`/`\cs`;
  - lists: `\listtable`, `\listoverridetable`, `\ls`, `\ilvl`, `\pntext`;
  - tables: `\trowd`, `\cellx`, `\cell`, `\row`;
  - fields: `\field` with `PAGE`, `NUMPAGES`, `TOC` and `REF`; `\fldrslt` supplies the cached value and is regenerated on save;
  - furniture: `\header`, `\footer`, `\headerf`, `\headerl`, `\headerr`, `\footerf`, `\footerl`, `\footerr`;
  - notes and anchors: `\footnote`, `\*\bkmkstart`, `\bkmkend`;
  - pictures: `\pict` with `\pngblip` / `\jpegblip` hex payloads.
- **Encoding:** an RTF stream is 7-bit. Characters arrive as `\uN` (with `\uc` fallback bytes to skip) or as raw bytes under `\ansicpg`. Parse `\uN` directly; map raw bytes through the declared code page. CP1252 and `\ansicpg65001` cover most files; other code pages use `encoding_rs` rather than hand-written tables (log the dependency, as with every other).
- **Preservation:** no kept-parts store. Keep the raw token stream of each unknown destination (whole group, including its `\*` prefix) and any unknown control word on a known run or paragraph, and write it back at the same place. This gives the same open → edit → save guarantee as §6.1 without dropping unknown-but-valid constructs.
- **Writer:** `\rtf1\ansi\deff0` with a font table, colour table, style sheet and `\uc1`; escape `\`, `{`, `}`; write non-ASCII as `\uN` with a `?` fallback; group property changes per paragraph rather than per character where possible. Output must open in Word, LibreOffice and Pages without a repair prompt (the same rule as DOCX/ODT).
- **Not editable in v1:** drawing objects and shapes (`\shp`, `\object`), annotations, and any field whose instruction is not one of the four above are preserved as opaque content. RTF has no comments or tracked-change model worth exposing, so their groups are preserved unchanged.

### 6.6 Fidelity scope for v1

DOCX, ODT and RTF all target this table; RTF additionally writes a cached `\fldrslt` value for every field.

| Feature | Edit | Preserve |
|---|---|---|
| Paragraph/character styles, headings, B/I/U/S, fonts, sizes, colours, highlight, sub/superscript | ✅ | ✅ |
| Alignment, indents, spacing, line spacing, page breaks | ✅ | ✅ |
| Bulleted and numbered lists (multi-level) | ✅ | ✅ |
| Page size, margins, orientation, sections | ✅ | ✅ |
| Headers and footers (incl. first/odd/even), page numbers | ✅ | ✅ |
| Columns | ✅ | ✅ |
| Footnotes and endnotes | ✅ | ✅ |
| Table of contents (generated from headings, updates on demand and before save) | ✅ | ✅ |
| Bookmarks and cross-references | ✅ | ✅ |
| Inline images (insert, resize, delete) | ✅ | ✅ |
| Tables (insert, edit cells, add/remove rows and columns, merged cells: read and preserve, basic merge/split) | ✅ | ✅ |
| Floating/anchored images, text boxes, shapes | ❌ | ✅ (placeholder) |
| Comments, tracked changes | ❌ | ✅ |
| Equations, charts, SmartArt, embedded objects | ❌ | ✅ (placeholder) |

---

## 7. Markdown mode (`libertum-markdown`)

The user chose WYSIWYG Markdown editing in the same editor. The known risk is that regenerating Markdown on save reformats the user's file (changing `*` to `_`, re-wrapping lines, renumbering lists). Mitigation:

- Parse with `comrak` (CommonMark + GFM: tables, strikethrough, task lists, footnotes, autolinks) **with source positions enabled**.
- Each imported block stores its original source slice and a `source_dirty: bool` flag. On save, clean blocks emit their original bytes exactly, and only dirty blocks are re-serialised.
- When re-serialising, follow the conventions detected in the file (emphasis marker, bullet marker, heading style ATX/Setext, code fence style). Detect them on import and store them in the document metadata.
- Front matter (YAML/TOML) and raw HTML blocks are preserved as `Block::Opaque`.
- Unsupported formatting in Markdown mode: the sidebar exposes only what Markdown can express (headings, bold, italic, strike, inline code, links, lists, task lists, block quotes, code blocks, tables, images, footnotes, horizontal rules).
- Layout in Markdown mode: the view is continuous (no page boxes) at a comfortable reading width, as Pages' word-processing view without page chrome. Pagination is skipped entirely, which keeps Markdown mode fast.
- Converting between modes ("Save As DOCX" from Markdown, or the reverse) is explicit. When saving to Markdown would lose formatting, show a list of what will be lost.

**Acceptance test:** open, then save without editing, must be byte-identical for every Markdown fixture.

---

## 8. Testing strategy

- **Unit tests** in every crate. `libertum-model` operations get `proptest` coverage: applying a random sequence of operations and then all their inverses must restore the original document.
- **Round-trip tests:** for every fixture in `tests/fixtures/{docx,odt,md}/`, run import → export → import and compare the models; for Markdown, compare bytes. Use `insta` snapshots of a normalised model dump.
- **Fixture corpus:** collect real-world files from Word, LibreOffice, Google Docs exports and Pages exports, plus edge cases such as huge tables, deep list nesting, many sections, CJK/RTL text and emoji. Note: RTL is a "must not crash or corrupt" requirement for v1, not a "must edit perfectly" one.
- **External validation:** a CI job converts exported DOCX/ODT files with headless LibreOffice (`soffice --headless --convert-to pdf`) to catch files that fail to open.
- **Performance:** `criterion` benches for typing at document middle, pagination of the 500-page corpus, opening, and saving. Track the results over time.
- **Editor behaviour tests** run headless against `libertum-editor`, with no window.

---

## 9. Phases

Each phase should end with a working, runnable app. Keep commits small. Do not start a phase until the previous phase's acceptance criteria pass.

### Phase 0 — Foundations and risk spikes

- Create the workspace skeleton, CI (fmt, clippy `-D warnings`, test on all three OSes), and a basic Iced window with a toolbar, empty canvas and collapsible sidebar.
- **Spike A, rendering:** shape 500 pages of generated text with parley, rasterise visible pages with tiny-skia + swash, and display them in an Iced canvas widget with scrolling and zoom. Measure the budgets in §2.
- **Spike B, IME:** confirm whether Iced exposes IME composition events on all three OSes.
- **Spike C, parley features:** confirm inline boxes (for images and footnote markers), per-span styles, and hit-testing/cursor APIs.
- **Spike D, GPU path:** render the same 500 pages with vello inside Iced's wgpu renderer. Confirm the wgpu versions are compatible (or that offscreen rendering works), and measure the same budgets as Spike A.
- Write `docs/spikes.md` with the findings. **Stop and report to the user if any spike fails.** The fallbacks (cosmic-text rendering, or a different toolkit) are the user's decision.

✅ Acceptance: scrolling 500 generated pages at 60 fps on the min-spec target in CPU mode, GPU mode working on a capable machine, and the spike report is written.

### Phase 1 — Model and plain-text editing

- Build `libertum-model` (blocks, paragraphs, operations, transactions, undo/redo) and `libertum-editor` (cursor, selection, movement, typing, deletion, clipboard as plain text).
- Add paragraph layout caching and the basic pagination engine (fixed A4/Letter page, no headers or footers yet), with incremental re-pagination.
- Wire up the canvas widget: cursor and selection overlay, mouse hit-testing, keyboard input.

✅ Acceptance: typing in the middle of the 1,000-page corpus meets the keystroke budget, undo/redo works across 1,000 steps, and the proptest suite passes.

### Phase 2 — Rich formatting and the sidebar

- Add the style sheet and style resolution, character and paragraph properties, and lists.
- Build the full Format tab of the sidebar, bound to the selection with mixed-state display, plus keyboard shortcuts for bold, italic and the rest.
- Add the status bar with incremental word count.

✅ Acceptance: every Format tab control works on a selection spanning multiple paragraphs, and undo restores the formatting exactly.

### Phase 3 — DOCX core

- Import and export of core text: styles, runs, paragraphs, lists, page breaks, and the preserved-parts mechanism.
- Open and save from the file menu (`rfd`), track dirty state, and prompt to save on close.

✅ Acceptance: the fixture round-trip tests pass, exported files open in Word and LibreOffice without repair prompts, and a 500-page DOCX meets the open/save budgets.

### Phase 4 — ODT core

- Same scope as Phase 3, for ODT.

✅ Acceptance: same as Phase 3, using LibreOffice as the reference.

### Phase 5 — Page layout

- Sections, page size, margins and orientation; headers and footers (first, odd, even); page-number fields; columns.
- Build the Document tab in the sidebar.
- Add DOCX and ODT import/export for all of the above.

> **Done 2026-11-11:** per-section page geometry is laid out, rendered and drawn on the canvas; each section uses its own paper size, orientation and margins. The Document pane still applies page setup to every section at once, and the LibreOffice page-count comparison is still open (see `docs/STATUS.md`).

✅ Acceptance: page layout fixtures render with the same page count as LibreOffice ±2%. This is an estimate-level target, because font metrics differ between engines.

### Phase 6 — Book features

> Pulled forward (2026-11-10, at the user's request): a **tables** slice is implemented — `Block::Table`, row-based pagination with repeated header rows, cell editing and a Table menu, plus DOCX round-trip. See `docs/decisions.md`. Footnotes, endnotes, bookmarks, images and table styles remain.
>
> Audited 2026-11-11 (third pass): **done.** The navigation slice, tables/TOC, footnotes/endnotes, bookmarks with cross-references, inline images and named table styles are in, with DOCX, ODT and RTF import/export for each. Table rows split across pages (unless kept together) with repeated header rows, and merged cells can be merged and split. The acceptance run is 343 pages with 200 footnotes, a 10-entry TOC, 20 images, 10 bookmarks, 2 cross-references and a banded table spanning four pages with one split row; see `docs/STATUS.md`.

- Footnotes and endnotes (insert, edit, renumber automatically, lay out at the page bottom).
- Table of contents (generate from headings, update command, auto-update before save).
- Bookmarks and cross-references.
- Inline images (insert, resize, lazy decoding, downsampled display cache).
- Tables (pagination across pages, repeated header rows, editing operations).
- Add import/export for all of the above.

✅ Acceptance (met 2026-11-11, `novel_notes_acceptance`): a 300-page novel fixture with 200 footnotes, a table of contents and images opens, edits and saves within budget, and round-trips cleanly.

### Phase 7 — Markdown mode

- Build `libertum-markdown` as described in §7: continuous view, mode-aware sidebar, and source-span preservation.

✅ Acceptance: the byte-identical no-edit round-trip passes on every fixture, and edited files keep their detected conventions.

### Phase 7b — RTF

- Build `libertum-rtf` as described in §6.5: tokeniser, model mapping, writer and the raw-token preservation of unknown destinations.
- Enable Open…/Save As for `.rtf` and add `.rtf` to the home screen's Open filter. Independent of the Markdown phase; schedule it with the user once the DOCX and ODT paths exist.

✅ Acceptance: every `.rtf` fixture round-trips with no edit, exported files open in Word, LibreOffice and Pages without a repair prompt, and a 500-page RTF opens within the §2 budgets.

### Phase 8 — Polish

> Done 2026-11-11: find and replace, autosave with crash recovery, recent files, the Settings window, drag and drop, right-click context menus and the accessibility audit. The native `.lbtd` format specified in `docs/format-libertum-native.md` was built alongside it (worth its own line in the next release notes).

- Find and replace (with match case and whole word), autosave and crash recovery, recent files, light/dark themes, the settings window, drag and drop to open files.
- **Context menus (right-click):** text (cut/copy/paste, formatting), table cells (insert/delete row and column, merge/split, delete table, cell borders/fill), headers/images (resize, delete, wrap) and the Headers pane (go to, copy link). Every action must also be reachable from the menu bar or a keyboard shortcut; a context menu never becomes the only route. The canvas already receives pointer button events, so this is app/widget work, not a layout change.
- Optional, if time permits: spellcheck (evaluate the `spellbook` crate with Hunspell dictionaries, and check its licence first) and PDF export.
- An accessibility pass: keyboard reachability of every sidebar control. Screen-reader support depends on Iced's accessibility status at the time, so record what is and isn't possible.

### Phase 9 — CI, packaging, release

- Use a GitHub Actions matrix (windows-latest, macos-latest for both arm64 and x86_64 or a universal binary, ubuntu-latest).
- Packaging: evaluate `cargo-dist` and `cargo-packager`. Targets are an MSI or NSIS installer on Windows, a `.dmg` with a `.app` bundle on macOS, and an AppImage plus `.deb` on Linux.
- Set up file associations for `.docx`, `.odt`, `.rtf` and `.md` (opt-in on Linux).
- Code signing and notarisation (Apple Developer ID, Windows Authenticode) need the user's certificates. Leave the steps stubbed, document them, and ask the user before wiring in any secrets.
- Add a performance regression job that runs the criterion benches against the previous release.

### Phase 10 — Document stitching (post-v1, priority 1)

- **Stitch documents:** pick several files in one dialog, reorder them in the app (Up/Down/Remove), choose the output format, and combine them into one book. Sources may be DOCX, ODT, RTF or Markdown; each is imported into a temporary `Document` and appended.
- **Merge rules:** the first document supplies document defaults and style-sheet order; later documents' styles are matched by name, then by formatting, else imported as new styles. List groups are renumbered so numbering does not continue across sources unless the user asks for it. Each source after the first starts a new section (with the first document's header/footer links), and its first heading can mark a new chapter.
- **Contents and numbering:** regenerate the table of contents, renumber chapters/footnotes/endnotes if the user asks, and offer "Keep source page numbering" versus "Number continuously".
- **Safety:** stitch into a new file (or an explicit "Replace current document" choice), keep each source untouched, and show a summary of what will change. Fixtures for same/different styles, section starts, lists and TOC.
- **Acceptance:** stitch three documents in a chosen order and the combined book opens with the expected block order, styles, chapter order and TOC; undo is not needed because stitching is a new document.

### Phase 11 — Chapter HTML export and local tag suggestions (post-v1, priority 2)

- **Chapter selection:** the user sets the chapter heading level (for example Heading 1) and chooses *latest chapter* or a range from a start heading to the heading before the next heading of that level. Settings persist in the document/settings.
- **HTML for AO3:** export a fragment (not a full page) with only the markup AO3 accepts — paragraphs, `em`, `strong`, `blockquote`, `ul`/`ol`/`li`, headings, horizontal rules and line breaks — with correct escaping and no page furniture. Copy-to-clipboard is the primary action; Save as `.html` is secondary.
- **Tags:** a local tag vocabulary stored in settings; BM25 (or another local ranking method) scores the chapter text against tag names, and the user reviews/edits the suggested tags. No network call and no automatic tagging: the writer stays in control. Tag rules (rating, warning, fandom, relationship, character, freeform) are kept separate.
- **Acceptance:** a fixture book exports the expected chapter only; the fragment pastes into AO3's HTML editor with correct emphasis, lists, headings and quotation marks; tag suggestions are deterministic and explainable.

### Phase 12 — Proofreading mode and AO3 sync (post-v1, priority 3)

- **Local first:** a proofreading mode that opens a local document, tracks pending edits and can compare a revised chapter with the version last synced.
- **AO3 connection:** AO3 has no official public API. Any sync must be a user-initiated, documented workflow (for example importing an author's own downloaded draft/HTML or a browser-assisted session), must never store passwords or session cookies in plain text, and must respect AO3's terms, rate limits and robots policy. The app must work fully offline; a sync failure never loses a local edit.
- **Draft versus published:** show both states, diff them, and require an explicit "update draft" or "update published chapter" action. Never silently publish. Keep a local copy of the last synced text.
- **Spike first:** before implementation, confirm what AO3 exposes (export/download, editor HTML) and whether a browser extension or a user-supplied URL is the safest route. This phase needs a user decision on scope and authentication.

---

## 10. Known risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Iced IME support incomplete on some OS | Medium (estimate) | Phase 0 Spike B. If it fails, report to the user before proceeding |
| parley API churn between versions | Medium | Pin versions in `Cargo.lock` and isolate parley behind `libertum-layout`'s own types |
| Page count differs from Word/LibreOffice (font metrics, hyphenation, kerning) | High | Accept it, and document it. Never claim pixel-identical layout. Use the same fonts where available |
| DOCX edge cases corrupt files | Medium | Preserved-parts design, a large fixture corpus, the headless LibreOffice CI check |
| Footnote/pagination feedback loops never converge | Low–Medium | Iteration cap plus a deterministic fallback (push the footnote continuation to the next page) |
| vello and Iced need incompatible `wgpu` versions | Medium (estimate) | Offscreen rendering to a texture (§4.5), or pin to versions that line up |
| GPU driver crashes on old hardware in GPU mode | Medium | Off by default, automatic fallback, and crash-flag detection (§4.5) |
| Fonts referenced by a document are missing on the system | High | Fallback via fontique, and show a "missing fonts" notice in the Document tab |
| Scope creep in Phase 6 (tables in particular) | High | Keep the table editing operations minimal in v1, and preserve the rest |
| Stitching style and numbering collisions | Medium | Match styles by name/formatting, renumber list groups, and make "continuous numbering" an explicit choice |
| AO3 HTML/tag rules change without notice | Medium | Keep the HTML whitelist and tag vocabulary data, add a fixture per rule, and never auto-publish |
| AO3 has no official API and its terms may forbid automated access | High | Phase 12 starts with a spike; prefer user-initiated export/import, store no credentials, and make the offline path primary |

---

## 11. Working rules

1. Read this plan fully before Phase 0. Re-read the relevant section at the start of each phase.
2. Look up current crate APIs on docs.rs. Do not write code from memory against Iced or parley.
3. Keep the crate boundaries in §3. Format crates must not depend on UI crates.
4. No `unwrap()`/`expect()` on I/O, parsing or user-input paths. Return typed errors (`thiserror`) and show friendly messages in the UI.
5. Every new feature comes with tests. Every format feature comes with a round-trip fixture.
6. Measure before optimising, but never introduce O(document) work on the keystroke path.
7. Stop and ask the user when a spike fails, a budget cannot be met, a dependency needs a non-permissive licence, or this plan conflicts with reality.
8. Keep `docs/decisions.md` as a short log of any deviation from this plan, with the reason.
9. Use British/Singapore English in user-facing strings (colour, centre, organise).
