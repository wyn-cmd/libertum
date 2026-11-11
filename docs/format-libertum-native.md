# A native Libertum document format (proposal)

Status: **implemented** (2026-11-11) in `crates/libertum-native`. The app opens, saves and offers
`.lbtd`, and the crash-recovery snapshot uses it. Differences from the sketch below: `view.json`
carries the zoom as well as pageless; `document.json` is compact rather than pretty-printed (size,
with determinism coming from ordering); and the manifest's `features` list is advisory — written
but not enforced on save, though unknown `extensions/` entries are kept. Two model changes were
needed: serde derives (with `#[serde(default)]` where the type has a `Default`) and a
`Document::from_parts` that rebuilds the indexes without reallocating ids. The container layout,
the media-by-hash split, deterministic bytes, the version gate and the refusals are as designed.

## Why consider one

DOCX, ODT and RTF are interchange formats. Every one of them is a lossy projection of
Libertum's model:

| Libertum concept | DOCX | ODT | RTF |
|---|---|---|---|
| Pageless view | no concept | no concept | no concept |
| A paragraph style's own direct formatting vs inherited | `w:pPr` plus style chain | `style:parent-style-name` plus direct | style sheet plus direct |
| Notes with byte offsets inside a paragraph, several per paragraph | `footnoteReference` run, nearest position | `text:note` inline | `\footnote` group |
| Cross-references with a cached result | `REF`/`PAGEREF` fields | `text:bookmark-ref` | field group |
| Images with a stable model hash | part per image, no hash | `Pictures/…` | `\pict` |
| Table style with header and band fills | regenerated `Tbl*` styles | `table:template-name` | resolved fills |
| Unknown-but-preserved material | package parts | package parts | raw groups |

On top of that, each writer regenerates parts it does not model (`styles.xml` wholesale,
RTF font/colour tables), so a save can quietly change bytes the user never edited. The
importers are careful, but "no loss in the mapped features" is still the honest promise —
the model is larger than the intersection of the three formats.

A native format can store the model as it is, store **view state** the interchange formats
have nowhere to put (pageless, zoom, which inspector was open), and still be a plain ZIP of
human-readable JSON that survives `git diff` reasonably.

## Goals

1. **Faithful.** Every field of `Document` round-trips, including ids, paragraph versions,
   style chains, notes, bookmarks, references, images (with their hash), tables (widths,
   spans, merges, styles), sections and header/footer variants.
2. **Forward compatible.** A newer file opens in an older build without losing the parts it
   does not understand (the model already has "preserve unknown package parts" for DOCX/ODT;
   do the same here as opaque `extensions/` entries).
3. **Cheap.** No XML schema, no namespace juggling, no zip tricks. Save must stay well inside
   the 2 s budget for a 500-page document; the existing 800-page ODT save is 82 ms, so JSON
   of roughly the same size is comfortable.
4. **Auditable.** A text container: `unzip`, `jq . document.json`, diff. Acceptable overhead
   over binary formats is a few times the size; a 500-page novel is a few hundred KB.
5. **Not a wall.** DOCX/ODT/RTF/Markdown stay first-class; this format is what LibreOffice
   and Word never needed, not a replacement for them.

## Non-goals

- Not an ODF/DOCX profile: no namespace, no `content.xml`, no interop with Word.
- Not a database: no incremental patching, no journal. A save writes the whole file.
- Not compressed beyond the ZIP entries (a later `zstd` option could be added behind a flag).

## Container

A ZIP archive, extension **`.lbtd`** ("Libertum document"), media type
`application/vnd.libertum.document+zip` (provisional; registering a vendor type is a Phase 9
task).

```
mimetype                     "application/vnd.libertum.document" (stored, first entry)
manifest.json                format version, generator, feature flags, part list
document.json                the model: metadata, styles, sections, body blocks
view.json                    view state: pageless, zoom, inspector, scroll anchor
media/<hash>.<ext>           one entry per distinct image, named by the model's media hash
extensions/<name>            opaque entries a newer build wrote; kept byte for byte
```

The `mimetype` entry is stored uncompressed and first, exactly as ODT does, so a file's type
can be read without opening the archive. `media/` entries are shared by hash, so two
paragraphs showing the same image store it once (the model already holds images behind an
`Arc<Media>` with a hash).

### `manifest.json`

```json
{
  "format": "libertum-document",
  "version": 1,
  "generator": "libertum 0.1.0",
  "features": ["pageless", "images", "notes", "cross-references", "table-styles"],
  "parts": ["document.json", "view.json", "media/9f2c….png"]
}
```

`features` is advisory: a build that does not know a feature must still refuse to *save*
over the file rather than silently drop it, unless the user says so (the same rule the
Markdown loss prompt follows).

### `document.json` (sketch)

One JSON object mirroring the model, snake_case, units in points. The model's own names are
kept so the mapping in code is mechanical (`serde` derives on `libertum-model` types, with
`#[serde(default)]` on every optional field for forward compatibility).

```json
{
  "id_seed": 4211,
  "styles": {
    "defaults": { "family": "Georgia", "size_pt": 12.0, "ligatures": "common" },
    "para_defaults": { "space_after_pt": 6.0, "line_spacing": { "multiple": 1.2 } },
    "paragraph_styles": [
      { "id": "Heading1", "name": "Heading 1", "base": "Body",
        "props": { "outline_level": 1, "space_before_pt": 18.0 } }
    ],
    "character_styles": []
  },
  "sections": [
    {
      "page": { "paper": "A4", "orientation": "portrait",
                "margins_pt": { "top": 72.0, "right": 72.0, "bottom": 72.0, "left": 72.0 } },
      "columns": 1, "column_gutter_pt": 18.0,
      "page_numbers": { "vertical": "bottom", "horizontal": "centre",
                        "start_at": 1, "mirror": false, "hide_first_page": false },
      "headers": { "default": { "text": "Halfway — {page}", "align": "start" } },
      "footers": { "default": { "text": "{page} of {pages}", "align": "centre" } }
    }
  ],
  "body": [
    { "type": "paragraph",
      "id": 3, "version": 7,
      "props": { "style": "Heading1", "outline_level": 1, "toc_target": 12 },
      "runs": [ { "range": [0, 9], "format": { "bold": true } } ],
      "notes":    [ { "kind": "footnote", "offset": 9, "text": "A note." } ],
      "bookmarks":[ { "name": "chapter_1", "offset": 0 } ],
      "images":   [ { "offset": 9, "media": "9f2c…", "width_pt": 240.0,
                      "height_pt": 180.0, "alt": "A checker", "name": "checker.png" } ],
      "cross_refs":[ { "range": [10, 19], "target": "chapter_1", "kind": "text" } ] },
    { "type": "table",
      "id": 40, "widths_pt": [200.0, 200.0], "header_rows": 1,
      "borders": { "outer": true, "inner": true, "width_pt": 0.5, "colour": [0,0,0,255] },
      "style": { "name": "Banded Rows", "header_fill": [217,217,217,255],
                 "band_fill": [242,242,242,255] },
      "rows": [
        { "cells": [ { "col_span": 1, "paragraphs": [ … ] } ],
          "min_height_pt": 0.0, "keep_together": false }
      ] }
  ],
  "extensions": { "com.example.plugin": "extensions/com.example.plugin" }
}
```

Text ranges are byte ranges into the paragraph text, exactly as the model holds them, so no
re-indexing is needed on load and the DOM-style "inline runs" problem disappears.

### `view.json`

```json
{ "pageless": true, "zoom": { "percent": 110 }, "inspector": "format-style",
  "scroll": { "block": 12, "offset": 0 } }
```

This is the part no interchange format can hold. It is **optional** and never affects the
document's content; a build that does not understand a key ignores it.

## Compatibility rules

1. **Never silently lose.** An older build that sees `features` it does not know, or an
   `extensions/` part, keeps the part and warns on save (reuse `loss_report`'s shape).
2. **Unknown keys are kept on read** where they can be (an `extras` map per object) or, if
   not worth modelling, the file is opened read-only with a clear message.
3. **Version gate.** `manifest.version` is a single integer; a newer major version refuses to
   open with "written by a newer Libertum" rather than guessing.
4. **Determinism.** Entries in a fixed order, timestamps zeroed, JSON keys sorted, so the
   same document saves to the same bytes — CI can hash a fixture (the Markdown crate already
   relies on byte-identical round trips).

## Sizing and performance

- A 500-page novel: about 3.5 MB of JSON before ZIP, roughly 500 KB after (a comparable ODT
  is 63 KiB for 800 pages of plain text; JSON is more verbose but deflate is kind to it).
- Save opens a `ZipWriter`, writes `document.json` in one pass from a borrowed `Document`.
  No extra byte buffers beyond the archive.
- Open parses once into the model. serde_json on a few MB is single-digit milliseconds on
  this machine (the ODT open of 800 pages is 97 ms including XML).
- Images stay out of `document.json`; only their hash is stored, so an edit does not rewrite
  megabytes of base64.

## Work needed (estimate)

| Step | Work | Size |
|---|---|---|
| 1 | `serde` derives on `libertum-model` (all public types, with `default` attributes) | 2–3 days |
| 2 | `libertum-native` crate: container read/write, manifest, media, extensions, determinism | 3–4 days |
| 3 | Round-trip fixtures and a proptest against `Document::content_eq` | 2 days |
| 4 | App wiring: open/save/Save As, home screen filter, default format setting | 1 day |
| 5 | `view.json` (pageless and zoom survive a reopen) | 1 day |
| 6 | Docs, decisions log, third-party licence entry (none new expected) | half a day |

`serde` and `serde_json` are MIT/Apache-2.0, so rule 4 is satisfied. No new binary formats
and no data files, so `docs/third-party-licences.md` needs only the dependency note.

## Risks

- **A fourth format.** Four formats already exist; a fifth doubles the round-trip matrix.
  Mitigated by making the native format the *canonical* one: DOCX/ODT/RTF/Markdown become
  import/export, and only the native path must be lossless.
- **Model churn.** Every model change becomes a schema change. Mitigated by `#[serde(default)]`
  and the `extensions` map; a version bump is needed only for incompatible reshaping.
- **File size versus ODT.** Acceptable for novels; worth measuring before claiming.
- **Ecosystem pull.** A proprietary extension invites "can Word open it?" — the answer must
  be a clear no, with DOCX/ODT export one command away.

## Alternatives considered

- **A stricter ODT profile** (our XML subset): keeps interop but still cannot express
  pageless, model ids, image hashes, or unknown-key preservation, and its writers regenerate
  parts today.
- **CBOR or MessagePack instead of JSON:** smaller and faster, but not human-readable, which
  removes the main advantage over a binary custom format for this project's debugging style.
- **SQLite with the model in tables:** good for huge documents and incremental saves, but a
  large dependency and complex save semantics for a word processor whose documents are small
  files the user copies around.

## Recommendation

Adopt it **as the canonical format** once Phase 8's polish lands, in this order:

1. Land serde on the model and the `libertum-native` crate behind a setting, so round-trip
   tests can compare it against DOCX/ODT from day one.
2. Make `.lbtd` the default for new documents; keep the current default (ODT) for users who
   turn it off, since the Settings window already offers a default format.
3. Use `view.json` for pageless, zoom and inspector state.
4. Leave DOCX, ODT, RTF and Markdown exactly as they are — import/export, with their loss
   prompts.

Do not start it before Phase 8: polish (context menus, find and replace, autosave) improves
every document, while a native format mostly improves fidelity that the three formats
already carry for the features most users touch.
