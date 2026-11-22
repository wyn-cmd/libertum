# Libertum 0.5.0

## Features

- **HTML and EPUB.** Libertum opens and saves web pages (`.html`, `.htm`) and imports EPUBs and AO3 downloads. HTML opens in the pageless reading view; Save writes a standalone page that keeps the text's look, and **Export > Plain HTML** writes semantic markup with no fonts, colours or layout. Both show the HTML loss report before a save that would drop formatting. An EPUB or AO3 download is imported as a new unsaved document, so the original file is never rewritten; the AO3 importer keeps chapter headings, chapter text, and the author's summaries and notes, and drops the download's preface metadata and afterword links.
- **Insert a chapter.** **Insert > Chapter from File…** and **Chapter from Copied HTML** add another document's body after the caret as one undoable step. Styles are matched by name, list groups are kept apart, and clashing bookmarks and generated contents entries are left out. The open file is untouched until the writer saves.
- **Compare with Posted.** Mark a chapter as posted from the AO3 chapter window or the compare bar, then choose **View > Compare with Posted**. The editor becomes read-only and shows added text tinted green and removed text with a red bar. **Previous** and **Next** move through the changes; **Keep** dismisses one and **Revert to Posted** puts the posted text back as one undo step. Everything is stored beside the document: no chapter, account or text is uploaded. Front matter before the first chapter heading is not compared, and tables and generated contents lists are left out.
- **Page colour.** **Document > Page Colour** offers White, nine soft tints that keep black text readable, and a hex field for any other colour. It is shown in both themes exactly as chosen and kept in Libertum (`.lbtd`), Word (`.docx`), OpenDocument (`.odt`) and HTML files and in PDF and PDF/A exports. RTF and Markdown do not store it.
- **Better AO3 tag suggestions.** Suggestions now use a story's full names ("Mara Ellison", not only "Mara") and can include repeated two-word phrases. A local **tag library** remembers tags copied with **Copy tags** and the writer's own full names; it can import a text or CSV tag list and can be forgotten at any time. A built-in starter list of 354 common additional tags is on by default and can be switched off. Tags already copied to a work, or hidden with the new **–** button, are not suggested again in any chapter of that work until **Show Again** is used.
- **Spell checking that keeps up.** A check now runs 450 ms after the last edit instead of waiting for the fixed two-second timer, the dictionary is built on a background thread at start-up and after a language change, and underlines computed for older text are dropped instead of being drawn at the wrong place. Single characters and punctuation are no longer marked as misspellings.
- **Quieter logs.** The window library no longer logs its entire configuration, including the window icon's pixels (about a million lines on every start), and debug builds no longer print ICU4X's complex-script warnings on every line-break pass.

## Changes

- The page colour repaints every page strip and takes part in the page cache key, so a colour change is visible at once.
- The typing and compare hooks used for measurements and screenshots are documented in `CONTRIBUTING.md` and `docs/decisions.md`.
- New crates: `libertum-html` (HTML and EPUB import/export on `html5ever`) and `libertum-diff` (paragraph and word comparison on `similar`). Both are library crates with no UI code and use permissive licences.

## Packaging

- Windows: `Libertum-Setup.exe` and `Libertum-Setup-0.5.0.exe`.
- Linux: `.deb`, `.rpm` and `.AppImage`.
- macOS: `.dmg` and `.zip` for Apple Silicon and Intel.
- `SHA256SUMS` covers every attached file.
- Existing installations are offered 0.5.0 by the in-app updater; on Linux the `.deb` and `.rpm` install themselves through the desktop's administrator prompt.

## Notes

- Builds are unsigned. Windows SmartScreen may warn on first run; on macOS, right-click the app and choose **Open**.
- The AO3 HTML fixtures used by the importer are hand-made from AO3's download structure and have not yet been checked against a live download. HTML import never fetches remote images: they become their alt text with a warning.
- The built-in starter tag list was written from general knowledge, not copied from AO3 or a tag dataset, so its suggestions should be reviewed before use.
- Compare mode compares top-level paragraphs; a moved paragraph shows as one removal and one addition, and a table or generated contents list is not compared.
- Libertum still makes no request to AO3. The only network uses remain the update check and the opt-in MCP client.
