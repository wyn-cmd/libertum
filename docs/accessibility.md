# Accessibility audit (Phase 8, 2026-11-11)

PLAN Phase 8 asks for "keyboard reachability of every sidebar control" and for screen-reader
support to be recorded as it stands, since it depends on Iced. This is that record.

## What is keyboard reachable

Every route below is declared in `crates/libertum-app/src/shortcuts.rs` and covered by the
shortcut tests, unless the row says otherwise.

| Action | Route |
|---|---|
| Open, Save, Save As | Cmd/Ctrl+O, Cmd/Ctrl+S, Cmd/Ctrl+Shift+S |
| Find & Replace | Cmd/Ctrl+F (bar), Enter/Next, Shift+Enter/Previous, F3 and Cmd/Ctrl+G forward, Shift+F3 and Cmd/Ctrl+Shift+G back |
| Settings | Cmd/Ctrl+, |
| Bold, italic, underline | Cmd/Ctrl+B, I, U |
| Superscript, subscript | Cmd/Ctrl+Shift+=, Cmd/Ctrl+= |
| Alignment | Ctrl+L, E, R, J (Cmd+{, |, } on macOS) |
| Page break | Cmd/Ctrl+Enter |
| Sidebar toggle | Cmd/Ctrl+Alt+I |
| Full screen | F11 (Ctrl+Cmd+F on macOS); Escape leaves it when nothing else is open |
| Escape | closes the front-most thing: a prompt (Cancel; Discard for the recovery offer), the Settings window, the Find bar, a menu, a context menu, and then full screen |
| Caret and selection | arrows, Home/End with the platform modifiers, Shift to extend, Cmd/Ctrl+arrows by word |
| Undo, redo | Cmd/Ctrl+Z, Cmd/Ctrl+Shift+Z and Cmd/Ctrl+Y |
| Cut, copy, paste, select all | Cmd/Ctrl+X, C, V, A |
| List levels | Tab and Shift+Tab in a list |
| Tab between table cells | Tab and Shift+Tab (Tab in the last cell adds a row) |
| Bookmark and reference insertion | via the Bookmarks pane's text fields (typing and Return work); no shortcut |

## What is mouse-only today

| Action | Note |
|---|---|
| Opening the toolbar menus (View, Zoom, Insert, Table) | Each *command* inside them has another route: everything in View has a shortcut or is a per-document choice; Insert's items are page break (shortcut), table (Table menu), image (no shortcut), footnote (no shortcut), TOC (no shortcut), page number (no shortcut); Table commands are also in the right-click menu; Zoom has the stepper and Cmd/Ctrl+wheel. |
| Toolbar buttons (Full Screen, Insert, Table, Image, Footnote, and the four inspector buttons) | Full Screen has F11, Page Break (in the Insert menu) has Cmd/Ctrl+Enter and the inspector buttons have Alt+Cmd/Ctrl+I; Image and Footnote are mouse-only. |
| Inspector controls | Every *field* can be typed into; the buttons and popups are pointer-only, except where a shortcut does the same thing (B/I/U, alignment, lists' Tab, baseline). |
| Table commands (insert/delete/merge/split/columns, styles) | Table menu and the right-click menu; no default shortcut (as in Word). |
| Image size, alt text, remove | Style pane and the right-click menu; no shortcut. |
| Home screen recent files | Pointer-only; the template chooser itself has arrows and Enter. |
| Page thumbnails (View > Page Thumbnails) | Pointer-only; from the keyboard, Page Up/Down move the caret ten paragraphs and Cmd/Ctrl+Home/End go to the start or end. |
| Watermark (Document pane) | The text field takes typing and Return applies it; Diagonal, Apply and Remove are pointer-only. |
| Modal prompt buttons | Pointer-only. Escape now takes the safe choice (Cancel, or Discard for the recovery offer) so a prompt never traps a keyboard user. |
| AO3 tag suggestions (chapter window) | The tag checkboxes, Copy tags and Edit vocabulary are pointer-only; Escape closes the vocabulary window and then the chapter window. The vocabulary window's name field takes typing and Return adds the tag, so a tag can be written without the mouse, but its category popup and every Remove button are pointer-only. |

## What Iced 0.14 cannot do

These are library limits, verified against the pinned source rather than assumed:

1. **No screen-reader support.** Iced 0.14 has no `accessibility`/accesskit feature (checked in
   `iced-0.14.0/Cargo.toml`), so the widget tree is not exposed to Windows Narrator,
   macOS VoiceOver or Orca at all. Nothing in Libertum can change that until Iced ships
   accesskit support; re-check when the pinned version moves.
2. **Buttons cannot take focus.** Only widgets that call `operation.focusable` participate in
   focus (text inputs do, buttons do not), so there is no Tab traversal across the toolbar or
   the inspector, and therefore no visible focus ring to draw.
3. **No programmatic focus.** `text_input` has `.id()` but no public "focus this" helper in
   0.14, so opening Find & Replace cannot put the caret in its query field. The workaround is
   the selection seed: Cmd/Ctrl+F fills the query from a one-line selection, so Enter finds the
   next match without touching the mouse.
4. **The canvas is opaque.** The page, caret and selection are a custom widget with no semantic
   tree, so an assistive technology could not read the document even with item 1 solved.

## What would move the needle

- Iced accesskit support: the single blocking dependency for screen readers. Watch the pinned
  release notes; when it lands, give the canvas a semantic text node (block text around the
  caret) and label the toolbar and inspector buttons.
- A menu-bar-style keyboard route (Alt+letter) for the four toolbar menus, with Up/Down/Enter
  inside a menu. Not built: on macOS Option+letter types characters, so it needs a
  platform-specific binding, and it is a UI project of its own.
- Keyboard access to the home screen's recent list (up/down from the template row).

## How this was checked

Manual audit of `shortcuts.rs` against the toolbar, the inspector panes, the home screen and
the prompts, plus a run of the app suite. There is no automated screen-reader test possible
while item 1 stands; the shortcut tests guard the bindings that do exist.
