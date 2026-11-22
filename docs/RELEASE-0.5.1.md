# Libertum 0.5.1

## Features

- **Spelling waits for the word to be finished.** A misspelling under, or ending at, the caret is not underlined while it is being typed; moving the caret off the word checks it, so a half-typed word no longer flashes a red line. Skipping the word being typed is not counted as an out-of-date result, so it cannot start a recheck every couple of seconds.
- **The spell checker sleeps when there is nothing to check.** The timer thread waits on a condition variable for its next deadline instead of looking at the clock ten times a second, and the two-second fallback tick is gone: typing, scrolling or a layout pass schedules one check 450 ms after the activity stops. A failed or out-of-date check is tried again two seconds later, unless a sooner check is already due.
- **An unfocused window is idle.** Losing the window's focus stops the caret blink and stops the autosave timer when there is nothing unsaved; returning restores both. Losing focus with unsaved work writes a recovery snapshot at once, and autosave keeps its timer in the background while the document is dirty.
- **Cheaper frames and keystrokes.** The selection's word count is cached by document, selection and revision, so dragging or scrolling over a large selection no longer copies and splits its text on every event. Finding a paragraph's section returns at once for the usual one-section document instead of walking every paragraph, a walk that ran twice per keystroke. Recovery snapshots serialise the copy already made for the worker thread instead of cloning the document again.

## Changes

- Choosing a spelling suggestion, Ignore Once or Ignore in This Document from the right-click menu closes the menu.
- The caret's exact position, not just its paragraph, is part of the spell-check signature: moving off a word asks for that word to be checked.

## Packaging

- Windows: `Libertum-Setup.exe` and `Libertum-Setup-0.5.1.exe`.
- Linux: `.deb`, `.rpm` and `.AppImage`.
- macOS: `.dmg` and `.zip` for Apple Silicon and Intel.
- `SHA256SUMS` covers every attached file.
- Existing installations are offered 0.5.1 by the in-app updater; on Linux the `.deb` and `.rpm` install themselves through the desktop's administrator prompt.

## Notes

- Builds are unsigned. Windows SmartScreen may warn on first run; on macOS, right-click the app and choose **Open**.
- No document format changed in this release; files opened and saved by 0.5.0 are unaffected.
