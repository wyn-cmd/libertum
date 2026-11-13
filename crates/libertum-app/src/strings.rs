//! All user-visible strings live here (PLAN §1: the UI must stay localisable).
//! British / Singapore English.

pub const APP_NAME: &str = "Libertum";
pub const UNTITLED: &str = "Untitled";

// Home screen (template chooser)
pub const HOME_TITLE: &str = "Choose a Template";
pub const HOME_SUBTITLE: &str = "Start a new document or story.";
pub const HOME_CREATE: &str = "Create";
pub const HOME_OPEN: &str = "Open…";
pub const TEMPLATE_BLANK: &str = "Blank";
pub const TEMPLATE_BLANK_DESC: &str = "An empty page.";
pub const TEMPLATE_MANUSCRIPT: &str = "Manuscript";
pub const TEMPLATE_MANUSCRIPT_DESC: &str = "Submission format: 12 pt serif, double-spaced.";
pub const TEMPLATE_BOOK: &str = "Book";
pub const TEMPLATE_BOOK_DESC: &str = "Title page, contents, then chapters.";

// Book (manuscript) template content
pub const MANUSCRIPT_TITLE: &str = "Story Title";
pub const MANUSCRIPT_BYLINE: &str = "by Author Name";
pub const MANUSCRIPT_CHAPTER: &str = "Chapter One";

// Book template content
pub const BOOK_TITLE: &str = "Book Title";
pub const BOOK_AUTHOR: &str = "Author Name";
pub const BOOK_CONTENTS: &str = "Contents";
pub const BOOK_CHAPTER: &str = "Chapter One";
pub const EDITED: &str = "Edited";
pub const SAVED: &str = "Saved";

// File actions and prompts
pub const FILE_OPEN_TITLE: &str = "Open a Document";
pub const FILE_SAVE_TITLE: &str = "Save Document";
pub const FILE_FILTER_DOCX: &str = "Word Document (*.docx)";
pub const FILE_FILTER_ODT: &str = "OpenDocument Text (*.odt)";
pub const FILE_FILTER_MARKDOWN: &str = "Markdown (*.md)";
pub const FILE_FILTER_RTF: &str = "Rich Text Format (*.rtf)";
pub const FILE_FILTER_PDF: &str = "PDF (*.pdf)";
pub const FILE_FILTER_EPUB: &str = "EPUB (*.epub)";
pub const FILE_FILTER_TEXT: &str = "Plain Text (*.txt)";
pub const EXPORT_SAVE_TITLE: &str = "Export Document";
pub const EXPORT_FAILED: &str = "The document could not be exported.";
pub const FILE_FILTER_SUPPORTED: &str = "Documents (*.lbtd, *.odt, *.docx, *.md, *.rtf)";
pub const SAVE: &str = "Save";
pub const DONT_SAVE: &str = "Don't Save";
pub const CANCEL: &str = "Cancel";
pub const OK: &str = "OK";
pub const UNSAVED_TITLE: &str = "Do you want to save the changes you made?";
pub const UNSAVED_DETAIL: &str = "Your changes will be lost if you don't save them.";
pub const OPEN_FAILED: &str = "The document could not be opened.";
pub const SAVE_FAILED: &str = "The document could not be saved.";

// Toolbar
pub const TB_VIEW: &str = "View";
pub const TB_ZOOM: &str = "Zoom";
pub const TB_FULL_SCREEN: &str = "Full Screen";
pub const TB_INSERT: &str = "Insert";
pub const TB_IMAGE: &str = "Image";
pub const TB_FOOTNOTE: &str = "Footnote";
pub const TB_EXPORT: &str = "Export";
pub const TB_FORMAT: &str = "Format";
pub const TB_DOCUMENT: &str = "Document";
pub const TB_HEADERS: &str = "Headers";
pub const TB_NOTES: &str = "Notes";
pub const PANE_NOTES: &str = "Notes";
pub const NOTES_EMPTY: &str =
    "No footnotes or endnotes yet. Use Insert > Footnote or Insert > Endnote.";
pub const NOTES_HINT: &str = "Edit a note here; click its number to jump to the reference.";
pub const PANE_BOOKMARKS: &str = "Bookmarks";
pub const BOOKMARKS_EMPTY: &str = "No bookmarks yet. Add one at the caret.";
pub const BOOKMARKS_HINT: &str =
    "Click a name to jump to it. Edit the name and press Return to rename.";
pub const BOOKMARK_ADD: &str = "Add Bookmark";
pub const BOOKMARK_NAME: &str = "Bookmark name";
pub const BOOKMARK_REMOVE: &str = "Remove";

// Export menu
pub const EXPORT_PDF: &str = "PDF";
pub const EXPORT_PDF_ARCHIVAL: &str = "PDF Archival";
pub const EXPORT_EPUB: &str = "EPUB";
pub const EXPORT_RTF: &str = "Rich Text Format (RTF)";
pub const EXPORT_TEXT: &str = "Plain Text";
pub const EXPORT_MARKDOWN: &str = "Markdown";
pub const EXPORT_AO3: &str = "AO3 Chapter";
pub const AO3_TITLE: &str = "AO3 Chapters";
pub const AO3_EMPTY: &str = "No chapters yet. Add headings at the selected levels.";
pub const AO3_HINT: &str = "Choose a chapter to copy its HTML into AO3's HTML editor.";
pub const AO3_COPY: &str = "Copy HTML";
pub const AO3_COPIED: &str = "Copied to the clipboard";
pub const AO3_CLOSE: &str = "Close";
pub const AO3_TAGS: &str = "Tags";
pub const AO3_TAGS_HINT: &str = "Suggested from your tag vocabulary, ranked on this chapter only. Nothing leaves this computer.";
pub const AO3_TAGS_EMPTY: &str = "No tags suggested. Add a few to your vocabulary first.";
pub const AO3_TAGS_COPY: &str = "Copy tags";
pub const AO3_TAGS_COPIED: &str = "Tags copied";
pub const AO3_TAGS_EDIT: &str = "Edit vocabulary…";
pub const TAG_VOCAB_TITLE: &str = "AO3 tags";
pub const TAG_VOCAB_HINT: &str =
    "Your own list. A chapter is ranked against it locally; no tags are ever added for you.";
pub const TAG_NAME_PLACEHOLDER: &str = "Tag name";
pub const TAG_ADD: &str = "Add tag";
pub const TAG_EMPTY: &str = "No tags yet.";
pub const TAG_REMOVE: &str = "Remove";
pub const TAG_DONE: &str = "Done";
pub const TAG_CATEGORY_RATING: &str = "Rating";
pub const TAG_CATEGORY_WARNING: &str = "Warning";
pub const TAG_CATEGORY_FANDOM: &str = "Fandom";
pub const TAG_CATEGORY_RELATIONSHIP: &str = "Relationship";
pub const TAG_CATEGORY_CHARACTER: &str = "Character";
pub const TAG_CATEGORY_FREEFORM: &str = "Freeform";

// View menu
pub const VIEW_SHOW_INSPECTOR: &str = "Show Inspector";
pub const VIEW_HIDE_INSPECTOR: &str = "Hide Inspector";
pub const VIEW_WORD_COUNT: &str = "Show Word Count";
pub const VIEW_THUMBNAILS: &str = "Page Thumbnails";
pub const VIEW_BOOKLET: &str = "Booklet View";
pub const VIEW_PAGELESS: &str = "Pageless";
pub const VIEW_HOME: &str = "Go to Home Screen";
pub const VIEW_STITCH: &str = "Stitch Documents…";
pub const HOME_STITCH: &str = "Stitch…";
pub const STITCH_TITLE: &str = "Stitch documents";
pub const STITCH_HINT: &str = "They are joined in this order. The first supplies the styles; each one after it starts a new section.";
pub const STITCH_ADD: &str = "Add Files…";
pub const STITCH_GO: &str = "Stitch";
pub const STITCH_EMPTY: &str = "Add at least one document to stitch.";
pub const VIEW_BOOKMARKS: &str = "Bookmarks";

// Zoom
pub const ZOOM_FIT_WIDTH: &str = "Fit Width";
pub const ZOOM_FIT_PAGE: &str = "Fit Page";
pub const ZOOM_IN: &str = "Zoom In";
pub const ZOOM_OUT: &str = "Zoom Out";
pub const ZOOM_ACTUAL: &str = "Actual Size (100%)";

// Insert menu
pub const INSERT_PAGE_BREAK: &str = "Page Break";
pub const INSERT_SECTION_BREAK: &str = "Section Break";
pub const INSERT_TABLE: &str = "Table";
pub const INSERT_IMAGE: &str = "Image";
pub const INSERT_FOOTNOTE: &str = "Footnote";
pub const INSERT_ENDNOTE: &str = "Endnote";
pub const INSERT_TOC: &str = "Table of Contents";
pub const INSERT_PAGE_NUMBER: &str = "Page Number";
pub const INSERT_DATE_TIME: &str = "Date & Time…";

// Date & Time chooser
pub const DATE_TIME_TITLE: &str = "Date & Time";
pub const DATE_TIME_SOURCE: &str = "Show";
pub const DATE_TIME_INSERTED: &str = "Inserted";
pub const DATE_TIME_UPDATED: &str = "Updated on save";
pub const DATE_TIME_UPDATED_HINT: &str = "An updated field changes whenever the document is saved.";
pub const DATE_TIME_GROUP_DATE_TIME: &str = "Date & time";
pub const DATE_TIME_GROUP_DATE: &str = "Date";
pub const DATE_TIME_GROUP_TIME: &str = "Time";
pub const DATE_TIME_INSERT: &str = "Insert";

// Table menu
pub const TABLE_INSERT: &str = "Insert Table";
pub const TABLE_ROW_ABOVE: &str = "Insert Row Above";
pub const TABLE_ROW_BELOW: &str = "Insert Row Below";
pub const TABLE_COLUMN_LEFT: &str = "Insert Column Left";
pub const TABLE_COLUMN_RIGHT: &str = "Insert Column Right";
pub const TABLE_DELETE_ROW: &str = "Delete Row";
pub const TABLE_DELETE_COLUMN: &str = "Delete Column";
pub const TABLE_DELETE: &str = "Delete Table";

// Inspector panes
pub const PANE_STYLE: &str = "Style";
pub const PANE_LAYOUT: &str = "Layout";
pub const PANE_MORE: &str = "More";
pub const PANE_DOCUMENT: &str = "Document";
pub const PANE_SECTION: &str = "Section";
pub const PANE_HEADERS: &str = "Headers";
pub const HEADERS_EMPTY: &str =
    "No headings yet. Apply a Heading style to a paragraph and it appears here.";
pub const HEADERS_HINT: &str = "Click a heading to jump to it.";
pub const MIXED: &str = "Mixed";
pub const MIXED_SUFFIX: &str = " (mixed)";
pub const UPDATE: &str = "Update";

pub const STYLE_TITLE: &str = "Title";
pub const STYLE_H1: &str = "Heading 1";
pub const STYLE_H2: &str = "Heading 2";
pub const STYLE_H3: &str = "Heading 3";
pub const STYLE_H4: &str = "Heading 4";
pub const STYLE_BODY: &str = "Body";
pub const STYLE_QUOTE: &str = "Quote";

// Style pane
pub const SECTION_FONT: &str = "Font";
pub const SECTION_CHARACTER_COLOUR: &str = "Character Colour";
pub const SECTION_ALIGNMENT: &str = "Alignment";
pub const SECTION_SPACING: &str = "Spacing";
pub const SECTION_LISTS: &str = "Bullets & Lists";
pub const FONT_WEIGHT_LIGHT: &str = "Light";
pub const FONT_WEIGHT_REGULAR: &str = "Regular";
pub const FONT_WEIGHT_MEDIUM: &str = "Medium";
pub const FONT_WEIGHT_SEMIBOLD: &str = "Semibold";
pub const FONT_WEIGHT_BOLD: &str = "Bold";
pub const COLOUR: &str = "Text";
pub const HIGHLIGHT: &str = "Highlight";
pub const NO_HIGHLIGHT: &str = "None";
pub const BASELINE: &str = "Baseline";
pub const BASELINE_DEFAULT: &str = "Default";
pub const BASELINE_SUPERSCRIPT: &str = "Superscript";
pub const BASELINE_SUBSCRIPT: &str = "Subscript";
pub const CHAR_SPACING: &str = "Character Spacing";
pub const FONT_OPTIONS_TIP: &str = "Advanced font options";
pub const UPDATE_STYLE_TIP: &str = "Update the paragraph style to match the selection";

pub const SWATCH_BLACK: &str = "Black";
pub const SWATCH_GREY: &str = "Grey";
pub const SWATCH_RED: &str = "Red";
pub const SWATCH_ORANGE: &str = "Orange";
pub const SWATCH_YELLOW: &str = "Yellow";
pub const SWATCH_GREEN: &str = "Green";
pub const SWATCH_BLUE: &str = "Blue";
pub const SWATCH_PURPLE: &str = "Purple";

pub const ALIGN_LEFT: &str = "Left";
pub const ALIGN_CENTRE: &str = "Centre";
pub const ALIGN_RIGHT: &str = "Right";
pub const ALIGN_JUSTIFY: &str = "Justify";

pub const SPACING_LINES: &str = "Lines";
pub const SPACING_AT_LEAST: &str = "At Least";
pub const SPACING_EXACTLY: &str = "Exactly";
pub const SPACING_BETWEEN: &str = "Between";
pub const SPACE_BEFORE: &str = "Before Paragraph";
pub const SPACE_AFTER: &str = "After Paragraph";

pub const LIST_NONE: &str = "None";
pub const LIST_BULLETED: &str = "Bullet";
pub const LIST_NUMBERED: &str = "Numbered";
pub const LIST_LETTERED: &str = "Lettered";
pub const LIST_HARVARD: &str = "Harvard";

// Layout pane
pub const SECTION_COLUMNS: &str = "Columns";
pub const COLUMNS_COUNT: &str = "Columns";
pub const COLUMNS_GUTTER: &str = "Gutter";
pub const SECTION_INDENTS: &str = "Indents";
pub const INDENT_FIRST: &str = "First";
pub const INDENT_LEFT: &str = "Left";
pub const INDENT_RIGHT: &str = "Right";
pub const SECTION_TABS: &str = "Tabs";
pub const TABS_DEFAULT: &str = "Default stops every 0.5 in";
pub const SECTION_BORDERS: &str = "Borders & Rules";
pub const BORDER_NONE: &str = "None";
pub const BORDER_THIN: &str = "Thin line";
pub const BORDER_MEDIUM: &str = "Medium line";
pub const BORDER_THICK: &str = "Thick line";
pub const BORDERS_HINT: &str = "Around each selected paragraph";

// More pane
pub const SECTION_PAGINATION: &str = "Pagination & Break";
pub const KEEP_LINES_TOGETHER: &str = "Keep lines together";
pub const KEEP_WITH_NEXT: &str = "Keep with next paragraph";
pub const PAGE_BREAK_BEFORE: &str = "Start paragraph on a new page";
pub const WIDOW_ORPHAN: &str = "Prevent widow & orphan lines";
pub const SECTION_HYPHENATION: &str = "Hyphenation";
pub const REMOVE_HYPHENATION: &str = "Remove hyphenation for paragraph";
pub const SECTION_LIGATURES: &str = "Ligatures";
pub const LIGATURE_DEFAULT: &str = "Use Default";
pub const LIGATURE_NONE: &str = "None";
pub const LIGATURE_ALL: &str = "All";

// Document pane
pub const SECTION_PAPER: &str = "Document Setup";
pub const ORIENT_PORTRAIT: &str = "Portrait";
pub const ORIENT_LANDSCAPE: &str = "Landscape";
pub const SECTION_HEADER_FOOTER: &str = "Header & Footer";
pub const HEADER: &str = "Header";
pub const FOOTER: &str = "Footer";
pub const HEADER_TEXT: &str = "Header text";
pub const FOOTER_TEXT: &str = "Footer text";
pub const FIRST_PAGE_HEADER: &str = "First page header";
pub const FIRST_PAGE_FOOTER: &str = "First page footer";
pub const EVEN_PAGE_HEADER: &str = "Even page header";
pub const EVEN_PAGE_FOOTER: &str = "Even page footer";
pub const DIFFERENT_FIRST_PAGE: &str = "Different first page";
pub const DIFFERENT_EVEN_ODD: &str = "Different odd & even pages";
pub const HF_TEXT_PLACEHOLDER: &str = "Type text, or {page} for the number";
pub const HF_FIELD_HINT: &str = "{page} is the page number, {pages} the page count";
pub const SECTION_MARGINS: &str = "Document Margins";
pub const MARGIN_TOP: &str = "Top";
pub const MARGIN_BOTTOM: &str = "Bottom";
pub const MARGIN_LEFT: &str = "Left";
pub const MARGIN_RIGHT: &str = "Right";
pub const SECTION_TYPOGRAPHY: &str = "Hyphenation & Ligatures";
pub const DOC_HYPHENATION: &str = "Hyphenation";
pub const DOC_LIGATURES: &str = "Ligatures";
pub const PAGE_SIZE_A4: &str = "A4";
pub const PAGE_SIZE_LETTER: &str = "Letter";
pub const PAGE_SIZE_A5: &str = "A5";
pub const PAGE_SIZE_LEGAL: &str = "Legal";

// Section pane
pub const SECTION_PAGE_NUMBERING: &str = "Page Numbering";
pub const SECTION_POSITION: &str = "Position";
pub const SECTION_LINKS: &str = "Section Links";
pub const LINK_HEADERS: &str = "Link headers to previous section";
pub const LINK_FOOTERS: &str = "Link footers to previous section";
pub const SECTION_FOOTNOTE: &str = "Footnote";
pub const FOOTNOTE_PLACEHOLDER: &str = "Footnote text";
pub const FOOTNOTE_NONE: &str = "No footnote on the caret paragraph. Use Insert > Footnote.";
pub fn footnote_number(number: u32) -> String {
    format!("Footnote {number}")
}
pub const NUMBERING_START_PAGE: &str = "Start numbering on page";
pub const NUMBERING_FIRST_NUMBER: &str = "First number";
pub const NUMBERING_USE_CURRENT: &str = "Use Current Page";
pub const NUMBERING_UPDATE: &str = "Update";
pub const NUMBERING_WHOLE_DOC: &str = "Applies to the whole document.";
pub const NUMBERING_SHOW: &str = "Show page numbers";
pub const NUMBERING_TOP: &str = "Top";
pub const NUMBERING_BOTTOM: &str = "Bottom";
pub const POSITION_LEFT: &str = "Left";
pub const POSITION_CENTRE: &str = "Centre";
pub const POSITION_RIGHT: &str = "Right";
pub const NUMBERING_MIRROR: &str = "Alternate sides (book)";
pub const HIDE_ON_FIRST_PAGE: &str = "Hide on first page";

// Units
pub const UNIT_PT: &str = "pt";
pub const UNIT_CM: &str = "cm";
pub const UNIT_IN: &str = "in";

pub const EXT_DOCX: &str = "docx";
pub const EXT_ODT: &str = "odt";
pub const EXT_NATIVE: &str = "lbtd";
pub const FORMAT_NATIVE: &str = "Libertum";
pub const FILE_FILTER_NATIVE: &str = "Libertum documents (*.lbtd)";
pub const EXT_MARKDOWN: &str = "md";
pub const EXT_RTF: &str = "rtf";

/// "Untitled.odt — Edited", as Pages shows the document name and its save state.
pub fn window_title(name: &str, extension: &str, edited: bool) -> String {
    let state = if edited { EDITED } else { SAVED };
    format!("{name}.{extension} — {state}")
}

/// "Page 12 of 486 · 104,233 words"
pub fn word_count_pill(page: usize, pages: usize, words: usize) -> String {
    let noun = if words == 1 { "word" } else { "words" };
    let words = group_thousands(words);
    format!("Page {page} of {pages} · {words} {noun}")
}

/// 104233 becomes "104,233".
fn group_thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "210 x 297 mm", or inches when the document is measured in inches.
pub fn paper_dimensions(width_mm: f32, height_mm: f32, inches: bool) -> String {
    if inches {
        format!("{:.2} x {:.2} in", width_mm / 25.4, height_mm / 25.4)
    } else {
        format!("{} x {} mm", trim_number(width_mm), trim_number(height_mm))
    }
}

fn trim_number(v: f32) -> String {
    let text = format!("{v:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_owned()
}

pub fn zoom_percent(percent: u16) -> String {
    format!("{percent}%")
}

// Images.
pub const SECTION_IMAGE: &str = "Image";
pub const IMAGE_ALT: &str = "Description (alt text)";
pub const IMAGE_REMOVE: &str = "Remove Image";
pub const IMAGE_SMALLER: &str = "Smaller";
pub const IMAGE_LARGER: &str = "Larger";
pub const IMAGE_PICK_TITLE: &str = "Insert Image";
pub const FILE_FILTER_IMAGES: &str = "Images (*.png, *.jpg, *.jpeg, *.gif, *.bmp)";
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp"];
pub const IMAGE_FAILED: &str = "The image could not be inserted.";

/// "6.35 cm wide" or "2.50 in wide" for an image's width.
pub fn image_width(width_pt: f32, inches: bool) -> String {
    if inches {
        format!("{:.2} in wide", width_pt / 72.0)
    } else {
        format!("{:.2} cm wide", width_pt / 72.0 * 2.54)
    }
}

// Cross-references (Bookmarks pane).
pub const CROSS_REF_TEXT: &str = "Insert Text Reference";
pub const CROSS_REF_PAGE: &str = "Insert Page Reference";
pub const CROSS_REF_UPDATE: &str = "Update References";
pub const CROSS_REF_HINT: &str = "A reference shows the bookmark's paragraph or its page number, and is updated before each save.";

// Right-click context menu.
pub const CONTEXT_CUT: &str = "Cut";
pub const CONTEXT_COPY: &str = "Copy";
pub const CONTEXT_PASTE: &str = "Paste";
pub const CONTEXT_SELECT_ALL: &str = "Select All";
pub const CONTEXT_BOLD: &str = "Bold";
pub const CONTEXT_ITALIC: &str = "Italic";
pub const CONTEXT_UNDERLINE: &str = "Underline";

// Settings window.
pub const SETTINGS_TITLE: &str = "Settings";
pub const SETTINGS_THEME: &str = "Appearance";
pub const SETTINGS_FORMAT: &str = "Default format for new documents";
pub const SETTINGS_AUTOSAVE: &str = "Autosave every";
pub const SETTINGS_UNITS: &str = "Measurement units";
pub const SETTINGS_CHAPTERS: &str = "Chapter breaks";
pub const SETTINGS_CHAPTERS_HINT: &str =
    "Headings at these levels start a chapter in AO3 and EPUB exports.";
pub const SETTINGS_TAGS: &str = "AO3 tags";
pub const SETTINGS_TAGS_HINT: &str =
    "Your own tag list. Suggested tags in the AO3 chapter window are ranked against it locally.";
pub const SETTINGS_CHAPTER_H1: &str = "Heading 1";
pub const SETTINGS_CHAPTER_H2: &str = "Heading 2";
pub const SETTINGS_CHAPTER_H3: &str = "Heading 3";
pub const SETTINGS_CHAPTER_H4: &str = "Heading 4";
pub const SETTINGS_ANTIALIASING: &str = "Smooth text and shapes on the page";
pub const SETTINGS_ANTIALIASING_HINT: &str = "Switch off for hard pixel edges on page text, lines and highlights. PDF export is not affected.";
pub const SETTINGS_PERFORMANCE: &str = "Show performance overlay";
pub const SETTINGS_PERFORMANCE_HINT: &str = "Frames per second and frame time in the top-right corner of the document view. It keeps the window redrawing while it is on.";
pub const PERFORMANCE_MEASURING: &str = "measuring…";
pub const SETTINGS_CLOSE: &str = "Done";
pub const VIEW_SETTINGS: &str = "Settings…";
pub const THEME_SYSTEM: &str = "System";
pub const THEME_LIGHT: &str = "Light";
pub const THEME_DARK: &str = "Dark";
pub const UNITS_AUTO: &str = "Automatic";
pub const UNITS_CENTIMETRES: &str = "cm";
pub const UNITS_INCHES: &str = "in";
pub const AUTOSAVE_OFF: &str = "Off";
pub const AUTOSAVE_30: &str = "30 s";
pub const AUTOSAVE_60: &str = "1 min";
pub const AUTOSAVE_300: &str = "5 min";
pub const FORMAT_ODT: &str = "ODT";
pub const FORMAT_DOCX: &str = "DOCX";
pub const FORMAT_RTF: &str = "RTF";
pub const FORMAT_MARKDOWN: &str = "Markdown";

// Home screen.
pub const HOME_RECENT: &str = "Recent";

// Crash recovery.
pub const RECOVERY_TITLE: &str = "Recover unsaved changes?";
pub const RECOVERY_DETAIL: &str =
    "Libertum did not close properly last time, so it kept a copy of the unsaved document.";
pub const RECOVERY_RESTORE: &str = "Restore";
pub const RECOVERY_DISCARD: &str = "Discard";

// Find and replace.
pub const FIND_PLACEHOLDER: &str = "Find";
pub const REPLACE_PLACEHOLDER: &str = "Replace";
pub const FIND_CASE: &str = "Aa";
pub const FIND_WHOLE_WORD: &str = "Ab";
pub const FIND_PREVIOUS: &str = "Prev";
pub const FIND_NEXT: &str = "Next";
pub const VIEW_FIND: &str = "Find…";
pub const FIND_REPLACE_ONE: &str = "Replace";
pub const FIND_REPLACE_ALL: &str = "All";
pub const FIND_NO_MATCHES: &str = "No matches";

/// "3 of 17" for the current match, or "17 found" when none of them is selected.
pub fn find_count(position: Option<usize>, total: usize) -> String {
    match position {
        Some(position) => format!("{position} of {total}"),
        None => format!("{total} found"),
    }
}

/// "Replaced 1 place" or "Replaced 4 places".
pub fn find_replaced(count: usize) -> String {
    match count {
        1 => "Replaced 1 place".to_owned(),
        n => format!("Replaced {n} places"),
    }
}

// Table menu: merging and styles.
pub const TABLE_MERGE_RIGHT: &str = "Merge with Cell to the Right";
pub const TABLE_MERGE_BELOW: &str = "Merge with Cell Below";
pub const TABLE_SPLIT_CELL: &str = "Split Cell";
pub const TABLE_STYLE_NONE: &str = "No Table Style";
pub const TABLE_COLUMN_NARROWER: &str = "Narrower";
pub const TABLE_COLUMN_WIDER: &str = "Wider";
pub const TABLE_COLUMN_EQUALISE: &str = "Equal Columns";
pub const TABLE_COLUMN_WIDTH: &str = "Column width";
pub const SECTION_TABLE: &str = "Table";

/// "4.23 cm" or "1.67 in" for a table column's width.
pub fn column_width(width_pt: f32, inches: bool) -> String {
    if inches {
        format!("{:.2} in", width_pt / 72.0)
    } else {
        format!("{:.2} cm", width_pt / 72.0 * 2.54)
    }
}

// Markdown mode.
pub const MARKDOWN_STYLE_NOTE: &str = "This is a Markdown file. Fonts, sizes, colours, underline, alignment and spacing cannot be stored in Markdown, so those controls are hidden.";
pub const MARKDOWN_LAYOUT_NOTE: &str = "This is a Markdown file. Columns, indents, borders, tabs and page-flow options cannot be stored in Markdown. Use Save As to keep them in an ODT, DOCX or RTF document.";
pub const MARKDOWN_DOCUMENT_NOTE: &str = "This is a Markdown file. Page size, margins, headers, footers and page numbers cannot be stored in Markdown. Use Save As to keep them in an ODT, DOCX or RTF document.";
pub const MARKDOWN_LOSS_TITLE: &str = "Saving as Markdown will lose some formatting";
pub const MARKDOWN_LOSS_MORE: &str = "and more";
pub const SAVE_ANYWAY: &str = "Save Anyway";

// Documents kept in a synced folder.
pub const CHANGED_ON_DISK_TITLE: &str = "This file was changed somewhere else";
pub const CHANGED_ON_DISK_DETAIL: &str = "Another device or program has saved a different version of this file since you opened it. Overwriting replaces that version (the replaced version is kept in the document's history). Saving a copy keeps both.";
pub const OVERWRITE: &str = "Overwrite";
pub const SAVE_A_COPY: &str = "Save a Copy\u{2026}";

// Full screen and page thumbnails (View menu).
pub const VIEW_ENTER_FULL_SCREEN: &str = "Enter Full Screen";
pub const VIEW_EXIT_FULL_SCREEN: &str = "Exit Full Screen";

// Watermark (Document pane).
pub const SECTION_WATERMARK: &str = "Watermark";
pub const WATERMARK_PLACEHOLDER: &str = "Text, for example DRAFT";
pub const WATERMARK_DIAGONAL: &str = "Diagonal";
pub const WATERMARK_APPLY: &str = "Apply";
pub const WATERMARK_REMOVE: &str = "Remove";
pub const WATERMARK_HINT: &str = "Shown faintly behind the text of every page, and kept in Libertum (.lbtd), Word (.docx) and OpenDocument (.odt) files and in PDF exports. RTF and Markdown files do not keep it.";

// Library, version history and sync conflicts
pub const VIEW_LIBRARY: &str = "Library\u{2026}";
pub const VIEW_HISTORY: &str = "Version History\u{2026}";
pub const HOME_LIBRARY: &str = "Library\u{2026}";
pub const LIBRARY_TITLE: &str = "Library";
pub const LIBRARY_HINT: &str = "The documents in your library folder, newest first. Put the folder inside a synced folder to keep it on several devices.";
pub const LIBRARY_NO_FOLDER: &str = "No library folder is chosen yet.";
pub const LIBRARY_EMPTY: &str = "No documents were found in this folder.";
pub const LIBRARY_CHOOSE: &str = "Choose Folder\u{2026}";
pub const LIBRARY_CHANGE: &str = "Change Folder\u{2026}";
pub const LIBRARY_CLOSE: &str = "Close";
pub const LIBRARY_OPEN: &str = "Open";
pub const LIBRARY_HISTORY: &str = "History";
pub const LIBRARY_CONFLICT_OF: &str = "A sync conflict copy of";
pub const LIBRARY_USE_COPY: &str = "Use This Copy";
pub const LIBRARY_KEEP_ORIGINAL: &str = "Keep Original";
pub const LIBRARY_CONFLICT_HINT: &str = "Use This Copy replaces the original with it. Keep Original puts the copy in the original's history. Either way nothing is lost.";
pub const HISTORY_TITLE: &str = "Version history";
pub const HISTORY_HINT: &str = "Earlier versions, kept when the document was saved over. Open one as a copy to look at it, or restore it (the current file is kept first).";
pub const HISTORY_EMPTY: &str = "This document has no earlier versions yet.";
pub const HISTORY_OPEN_COPY: &str = "Open Copy";
pub const HISTORY_RESTORE: &str = "Restore";
pub const HISTORY_BACK: &str = "Back";
pub const HISTORY_SAVE_FIRST: &str =
    "Save or close the document before restoring an earlier version of it.";
pub const HISTORY_NO_FILE: &str = "Save the document first: only a saved file has a history.";
pub const LIBRARY_ACTION_FAILED: &str = "That did not work:";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_are_grouped() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(104_233), "104,233");
        assert_eq!(
            word_count_pill(12, 486, 104_233),
            "Page 12 of 486 · 104,233 words"
        );
        assert_eq!(word_count_pill(1, 1, 1), "Page 1 of 1 · 1 word");
    }

    #[test]
    fn title_shows_extension_and_state() {
        assert_eq!(
            window_title("Untitled", "odt", true),
            "Untitled.odt — Edited"
        );
        assert_eq!(window_title("Report", "docx", false), "Report.docx — Saved");
    }

    #[test]
    fn paper_dimensions_follow_units() {
        assert_eq!(paper_dimensions(210.0, 297.0, false), "210 x 297 mm");
        assert_eq!(paper_dimensions(215.9, 279.4, false), "215.9 x 279.4 mm");
        assert_eq!(paper_dimensions(215.9, 279.4, true), "8.50 x 11.00 in");
    }
}
