# Third-party licences

Dependencies are MIT, Apache-2.0, ISC, BSD or Zlib (AGENTS.md rule 4). This file records the cases that need more than that: embedded data with its own licence, and notices that must ship with the binary.

## Hyphenation patterns (`hypher` 0.1.8)

The `hypher` crate (MIT OR Apache-2.0, by the Typst authors) embeds TeX hyphenation patterns, compiled into the binary. **Each language's patterns have their own licence**, and some (LPPL, MPL) are not on the approved list, so the workspace enables languages one by one (`Cargo.toml`, `hypher` features). Check a language's pattern file in <https://github.com/typst/hypher/tree/main/patterns> before enabling it, add it below, and log it in `docs/decisions.md`.

| Language | Feature | Pattern file | Licence | Checked |
|---|---|---|---|---|
| English (US) | `english` | `hyph-en-us.tex` | FSF all-permissive (notice below) | 2026-11-09 |

Notice that must be preserved (from `hyph-en-us.tex`, part of the hyph-utf8 package):

> Copyright (C) 1990, 2004, 2005 Gerard D.C. Kuiken
>
> Copying and distribution of this file, with or without modification, are permitted in any medium without royalty provided the copyright notice and this notice are preserved.

When packaging (Phase 9), ship this notice with the application (for example in an "Acknowledgements" file or the About box).

## Icons

Lucide icons (ISC), with the licence in `crates/libertum-app/assets/icons/`.

## Crates added for the native format (2026-11-11)

`serde` (with the `rc` feature) and `serde_json` are MIT OR Apache-2.0, so the permissive-only rule in
AGENTS.md is satisfied. They were added to the workspace for `libertum-native` and the serde derives on
`libertum-model`. The ZIP reader and writer was already a dependency of the DOCX and ODT crates (`zip`, MIT).
No embedded data files were added, so the notices this application must ship are unchanged.

## PDF export (2026-11-13)

`printpdf` 0.12.9 and `lopdf` 0.44 are MIT. printpdf brings `allsorts`/`allsorts-azul`, `time` and
`image` transitively; those are MIT or Apache-2.0. No font or ICC data is shipped: fonts come from
the user's system and are embedded (subset) in the exported PDF only, and the sRGB profile used
for PDF/A's output intent is generated in `crates/libertum-render/src/pdf.rs`, so there is no
third-party profile licence to carry.

## Vendored crates (2026-11-14)

For Wayland touchpad pinch zoom, three crates are vendored under `vendor/` with the small patches
described in `docs/decisions.md`: `winit` 0.30.13 (Apache-2.0 OR MIT, licence in
`vendor/winit/LICENSE`), `iced_core` 0.14.0 and `iced_winit` 0.14.1 (MIT, licence in
`vendor/iced_core/LICENSE` and `vendor/iced_winit/LICENSE`). The patches do not change the licence
terms.
