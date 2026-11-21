# Libertum 0.4.3

## Performance

- **Pagination no longer leaves a very long document busy after it finishes.** The background paginator used to notify the window after every four pages, and each notification rebuilt every page's geometry. On the 5,000-page demo that work piled up after pagination. Notifications are now coalesced to at most ten a second (with one final message), page geometry is extended for newly paginated pages instead of rebuilt, and page-strip metrics are cached between draws and pointer events. The 5,000-page autoscroll held 10.65 ms mean and 16.42 ms p95 (p99 18.55 ms), with `draw()` at 0.09 ms; pagination completed in 5.6 s.
- **An edit re-hashes only the pages it can change.** Ordinary text edits start at the first affected page and leave earlier bitmap cache keys alone. Documents with a live table of contents or a `{pages}` header/footer still take the conservative full path, because an earlier page can display a later page's number. Finishing pagination re-hashes only pages that depend on the final page count.
- **Spellchecking costs less while idle.** Checks still run on a two-second timer, but the change test now uses the document revision, layout generation and viewport instead of cloning every visible paragraph. The dictionary is loaded once per pass, words are lowercased only when a personal or ignore list is in use, and the underline rectangles are cached until the text or layout changes.
- **Autosave skips unchanged snapshots.** A dirty document that has not changed since the last snapshot is no longer cloned and written again. `Ctrl/Cmd+S` on a clean document is a no-op.

## Changes

- **Update style to match** now takes the character formatting shared by the selected text inside the paragraph, not just formatting that covers the whole paragraph.
- Typing starts a short palm-rejection window: trackpad moves, clicks, scrolls and pinches on the page are ignored for 500 ms after a typed key.
- Removed unused workspace dependencies and cleaned the Cargo metadata.

## Packaging

- Windows: `Libertum-Setup.exe` and `Libertum-Setup-0.4.3.exe`.
- Linux: `.deb`, `.rpm` and `.AppImage`.
- macOS: `.dmg` and `.zip` for Apple Silicon and Intel.
- `SHA256SUMS` covers every attached file.

## Notes

- Builds are unsigned. Windows SmartScreen may warn on first run; on macOS, right-click the app and choose **Open**.
- The performance figures come from a Fedora/GNOME Wayland machine with the generated 5,000-page document and the autoscroll test harness.
