# Third-party licences

Dependencies are MIT, Apache-2.0, ISC, BSD or Zlib (CONTRIBUTING.md rule 4). This file records the cases that need more than that: embedded data with its own licence, and notices that must ship with the binary.

## Hyphenation patterns (`hypher` 0.1.8)

The `hypher` crate (MIT OR Apache-2.0, by the Typst authors) embeds TeX hyphenation patterns, compiled into the binary. **Each language's patterns have their own licence**, and some (LPPL, MPL) are not on the approved list, so the workspace enables languages one by one (`Cargo.toml`, `hypher` features). Check a language's pattern file in <https://github.com/typst/hypher/tree/main/patterns> before enabling it, add it below, and log it in `docs/decisions.md`.

| Language | Feature | Pattern file | Licence | Checked |
|---|---|---|---|---|
| English (US) | `english` | `hyph-en-us.tex` | FSF all-permissive (notice below) | 2026-11-09 |
| Ukrainian | `ukrainian` | `hyph-uk.tex` | MIT, chosen from "MIT or LPPL" (notice below) | 2026-11-23 |

Notice that must be preserved (from `hyph-en-us.tex`, part of the hyph-utf8 package):

> Copyright (C) 1990, 2004, 2005 Gerard D.C. Kuiken
>
> Copying and distribution of this file, with or without modification, are permitted in any medium without royalty provided the copyright notice and this notice are preserved.

Ukrainian (`hyph-uk.tex`, hyph-utf8 package) is offered under either the MIT licence or the LPPL; Libertum uses it under the MIT licence:

> Copyright (C) 1998-2001 Maksym Polyakov
>
> Licensed under the MIT licence, <https://opensource.org/licenses/MIT>: the standard text, with the copyright line above, applies to these patterns.

Russian hyphenation patterns (`hyph-ru.tex`) are LPPL, which Libertum's permissive-only rule does not take, so Russian text is not hyphenated (its spelling dictionary is separate, below).

When packaging (Phase 9), ship this notice with the application (for example in an "Acknowledgements" file or the About box).

## Icons

Lucide icons (ISC), with the licence in `crates/libertum-app/assets/icons/`.

## Crates added for the native format (2026-11-11)

`serde` (with the `rc` feature) and `serde_json` are MIT OR Apache-2.0, so the permissive-only rule in
CONTRIBUTING.md is satisfied. They were added to the workspace for `libertum-native` and the serde derives on
`libertum-model`. The ZIP reader and writer was already a dependency of the DOCX and ODT crates (`zip`, MIT).
No embedded data files were added, so the notices this application must ship are unchanged.

## PDF export (2026-11-13)

`printpdf` 0.12.9 and `lopdf` 0.44 are MIT. printpdf brings `allsorts`/`allsorts-azul`, `time` and
`image` transitively; those are MIT or Apache-2.0. No font or ICC data is shipped: fonts come from
the host system and are embedded (subset) in the exported PDF only, and the sRGB profile used
for PDF/A's output intent is generated in `crates/libertum-render/src/pdf.rs`, so there is no
third-party profile licence to carry.

## Vendored crates (2026-11-14)

For Wayland touchpad pinch zoom, three crates are vendored under `vendor/` with the small patches
described in `docs/decisions.md`: `winit` 0.30.13 (Apache-2.0 OR MIT, licence in
`vendor/winit/LICENSE`), `iced_core` 0.14.0 and `iced_winit` 0.14.1 (MIT, licence in
`vendor/iced_core/LICENSE` and `vendor/iced_winit/LICENSE`). The patches do not change the licence
terms.

## Spellchecking (2026-11-15, engine and languages changed 2026-11-23)

**`spellbook` 0.4.2 is MPL-2.0**, the one dependency here that is not MIT, Apache-2.0, ISC, BSD or
Zlib. The maintainer approved it on 2026-11-23 (docs/decisions.md). MPL-2.0 is a file-level copyleft:
the crate is used unmodified, so Libertum's own code stays MIT; if a `spellbook` file were ever
modified, that file's changes would have to be published under the MPL, and the MPL text and a
pointer to the crate's source must ship with the application. Its text is the `LICENSE` file in
the crate (<https://github.com/helix-editor/spellbook>). Its dependencies are `hashbrown`
(MIT OR Apache-2.0) and `foldhash` (Zlib). It replaced `zspell` (Apache-2.0), which is no longer
used.

The bundled dictionaries:

| Dictionary | Source | Licence | Files |
|---|---|---|---|
| English (British) | `wooorm/dictionaries` `en-GB` | MIT AND BSD | `crates/libertum-app/assets/dictionaries/en_GB.{aff,dic}`, `LICENSE_en_GB.txt` |
| English (American) | SCOWL `en_US` via LibreOffice dictionaries | SCOWL permission (with BSD, WordNet and UKACD notices) | `crates/libertum-app/assets/dictionaries/en_US.{aff,dic}`, `LICENSE_en_US.txt` |
| Ukrainian | `uk_UA` in LibreOffice dictionaries, from the `dict_uk` project (Andriy Rysin) | **MPL 1.1** | `crates/libertum-app/assets/dictionaries/uk_UA.{aff,dic}`, `LICENSE_uk_UA.txt` (README and full MPL 1.1 text) |
| Russian | `ru_RU` in LibreOffice dictionaries (Alexander I. Lebedev) | BSD-style: keep the notice, modified versions must be marked, no endorsement | `crates/libertum-app/assets/dictionaries/ru_RU.{aff,dic}`, `LICENSE_ru_RU.txt` |

The licence files are kept beside the dictionaries and must be included in packaged notices. The
dictionaries are embedded unmodified (the MPL 1.1 covers the Ukrainian files themselves, and
they are shipped in source form, as the dictionary source is the data). They add about 12 MB to
the binary.

## Word dictionary for Chinese and Japanese (2026-11-23)

Double-click, Ctrl+Arrow and word delete find Chinese and Japanese words with ICU4X's `cjdict`
dictionary, through `icu_segmenter` 2.4 with its compiled data. The crate was already in the tree
(parley depends on it); it is now also a direct dependency of `libertum-editor`. Licence:
**Unicode-3.0** (the ICU4X data and code), which is permissive: keep the copyright and permission
notice with the application. It is not on the list above by name; `unicode-ident` already carries
the same licence in this tree.

## HTML import and comparison (release 0.5.0, 2026-11-22)

- **`html5ever` 0.40** (MIT OR Apache-2.0) parses HTML in `libertum-html`. Its 35-crate tree
  was checked with `cargo metadata`: all MIT, Apache-2.0 or Unlicense-OR-MIT, plus
  Unicode-3.0 on `unicode-ident` (already in the tree through `syn`). `markup5ever_rcdom` is
  deliberately not used. Base64 is written in the crate, so no new dependency is added for it.
- **`similar` 3.2.1** (Apache-2.0, no extra crates with default features) compares paragraphs
  and words in `libertum-diff`; words are split with the existing `unicode-segmentation`.
- **The AO3 starter tag list** (`crates/libertum-export/data/ao3-starter-tags.txt`) is
  hand-written from general knowledge for this release, not copied from AO3 or a tag dataset.
  It carries no third-party data and no notice; it still needs the writer's review.

## AO3 rating and warning names

`libertum-export` (`AO3_RATINGS`, `AO3_WARNINGS`) carries the names of the ratings and archive warnings that AO3's posting form offers, so the writer picks them instead of typing them. They are short factual labels, not AO3 code or text, and no AO3 content is bundled beyond them. They were taken from AO3's own pages (the Terms of Service FAQ, the Tags FAQ and the Posting a Work tutorial) through a web search on 2026-11-21; the live form was not read because AO3 rate-limited the request. "Underage Sex" is the name a 2024 AO3 guide gives; the older Tags FAQ says "Underage". Re-check both lists against the live form when the rate limit allows.
