# Libertum 0.4.2

## Features

- **Scrolling stays fast all the way down a long document.** The canvas used to add up every page before the one it was drawing, so a frame near the end of a large document cost far more than one at the start: scrolling was smooth when the document opened and stuttered after a while, the further in it went. Page positions now come from a running total, and the visible range and hit testing binary-search it, so the cost is the same at page 1 and page 4,000. On a 2,000-page document the frame interval stayed flat from the first page to the last (mean 15.6 ms, p95 19.3 ms, `draw()` 0.05 ms, no blank or stale pages).

## Changes

- Per-page geometry and used heights are cached between draws and scroll events and shared, instead of being rebuilt for the whole document several times per event. They are rebuilt once when the layout actually changes (an edit, pagination, a page-setup change).
- Strip plans more than 128 pages from the viewport are forgotten, so the plan map no longer grows with every page ever visited. Their strips stay in the bitmap cache and are reused when the page comes back.
- Existing installations are offered 0.4.2 by the in-app updater; on Linux the `.deb` and `.rpm` install themselves through the desktop's administrator prompt.

## Packaging

- Windows: `Libertum-Setup.exe` and `Libertum-Setup-0.4.2.exe`.
- Linux: `.deb`, `.rpm` and `.AppImage`.
- macOS: `.dmg` and `.zip` for Apple Silicon and Intel.
- `SHA256SUMS` covers every attached file.

## Notes

- Builds are unsigned. Windows SmartScreen may warn on first run; on macOS, right-click the app and choose **Open**.
- The performance figures come from a Fedora/GNOME Wayland machine with the 2,000-page generated document and the autoscroll test harness; your display, zoom and document will differ, but the frame time no longer grows with the page number.
