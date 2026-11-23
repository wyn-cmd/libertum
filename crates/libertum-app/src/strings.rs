//! All user-visible strings live here (PLAN §1: the UI must stay localisable).
//! British / Singapore English.

pub const APP_NAME: &str = "Libertum";
pub const UNTITLED: &str = "Untitled";

// Home screen (template chooser)
pub const HOME_TITLE: &str = "Choose a Template";
pub const HOME_SUBTITLE: &str = "Start a new document or story.";
pub const HOME_CREATE: &str = "Create";
pub const HOME_YOUR_TEMPLATES: &str = "Your templates";
pub const HOME_ADD_TEMPLATE: &str = "Add template…";
pub const HOME_ADD_TEMPLATE_HINT: &str = "Start from an ODT, DOCX or Libertum document.";
pub const HOME_REMOVE_TEMPLATE: &str = "Remove template";
pub const TEMPLATE_FORMAT_ODT: &str = "ODT template";
pub const TEMPLATE_FORMAT_DOCX: &str = "DOCX template";
pub const TEMPLATE_FORMAT_NATIVE: &str = "Libertum template";
pub const TEMPLATE_ADD_TITLE: &str = "Add a document template";
pub const TEMPLATE_FILTER: &str = "Document templates";
pub const TEMPLATE_ADD_FAILED: &str = "Could not add the template.";
pub const TEMPLATE_OPEN_FAILED: &str = "Could not open the template.";
pub const TEMPLATE_REMOVE_FAILED: &str = "Could not remove the template.";
pub const TEMPLATE_UNSUPPORTED: &str =
    "Only ODT, DOCX and Libertum (.lbtd) documents can be templates.";
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
pub const FILE_FILTER_SUPPORTED: &str =
    "Documents (*.lbtd, *.odt, *.docx, *.md, *.rtf, *.html, *.epub)";
pub const SAVE: &str = "Save";
pub const DONT_SAVE: &str = "Don't Save";
pub const CANCEL: &str = "Cancel";
pub const OK: &str = "OK";
pub const UNSAVED_TITLE: &str = "Do you want to save the changes you made?";
pub const UNSAVED_DETAIL: &str = "Your changes will be lost if you don't save them.";
pub const OPEN_FAILED: &str = "The document could not be opened.";
pub const TOC_NOTICE_TITLE: &str = "This document has a table of contents";
pub const TOC_NOTICE_DETAIL: &str = "Libertum has rebuilt it from the document's headings, so its look may differ from the original. It stays a live table of contents: it updates as you edit the headings and is saved in the same form, so Word and LibreOffice can update it too.";
pub const SAVE_FAILED: &str = "The document could not be saved.";

// Toolbar
pub const TB_VIEW: &str = "View";
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
pub const EXPORT_PLAIN_HTML: &str = "Plain HTML";
pub const EXPORT_AO3: &str = "AO3 Chapter";
pub const AO3_TITLE: &str = "AO3 Chapters";
pub const AO3_EMPTY: &str = "No chapters yet. Add headings at the selected levels.";
pub const AO3_HINT: &str = "Choose a chapter to copy its HTML into AO3's HTML editor.";
pub const AO3_COPY: &str = "Copy HTML";
pub const AO3_COPIED: &str = "Copied to the clipboard";
pub const AO3_CLOSE: &str = "Close";
pub const AO3_TITLE_COPY: &str = "Copy title";
pub const AO3_TITLE_COPIED: &str = "Title copied";
pub const AO3_TAGS: &str = "Tags";
pub const AO3_TAGS_HINT: &str = "Suggested from your tag vocabulary, ranked on this chapter only. Nothing leaves this computer.";
pub const AO3_TAGS_EMPTY: &str = "No tags suggested. Add a few to your vocabulary first.";
pub const AO3_TAGS_COPY: &str = "Copy tags";
pub const AO3_TAGS_COPIED: &str = "Tags copied";
/// Shown instead of Copy for the rating and the warnings, which AO3's form takes as choices.
pub const AO3_TAGS_CHOOSE_ON_AO3: &str = "Choose on AO3";
pub const TAG_NAME_CHOOSE: &str = "Choose\u{2026}";
pub const AO3_TAGS_EDIT: &str = "Edit vocabulary…";
pub const AO3_TAGS_AUTO: &str = "Also suggest characters, pairings and keywords from the text";
pub const AO3_TAGS_AUTO_HINT: &str = "Counted on this computer. Guesses to check: A/B only where romantic words appear; keywords can be noisy.";
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
pub const VIEW_NEW: &str = "New Document…";
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
pub const INSERT_LINKED_MEDIA: &str = "Linked Image or Video…";
pub const LINKED_TITLE: &str = "Linked Image or Video";
pub const LINKED_URL_PLACEHOLDER: &str = "Web address, e.g. https://example.com/picture.png";
pub const LINKED_ALT_PLACEHOLDER: &str = "Description (optional)";
pub const LINKED_KIND_IMAGE: &str = "Image";
pub const LINKED_KIND_VIDEO: &str = "Video";
pub const LINKED_PREVIEW: &str = "Download a preview now";
pub const LINKED_HINT: &str = "The chapter's AO3 HTML points at this address. A preview is a copy of the picture kept in your document so you can see it here; Libertum connects to the address once to get it, and only when you ask.";
pub const LINKED_INSERT: &str = "Insert";
pub const LINKED_DOWNLOADING: &str = "Downloading the preview…";
pub const LINKED_BAD_URL: &str = "Enter a web address that starts with https:// or http://.";
pub const LINKED_FETCH_PREVIEW: &str = "Fetch Preview";
pub const LINKED_PREVIEW_FAILED: &str = "The preview could not be downloaded:";
pub const LINKED_NO_PREVIEW: &str =
    "This video's address gives no picture to download, so it keeps its placeholder.";
pub const LINKED_VIDEO_NOTE: &str = "Video link";
pub const LINKED_IMAGE_NOTE: &str = "Image link";

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
pub const SECTION_LANGUAGE: &str = "Language";
pub const SECOND_LANGUAGE: &str = "Second language (optional)";
pub const SECTION_TEXT_DIRECTION: &str = "Text Direction";
pub const TEXT_HORIZONTAL: &str = "Horizontal";
pub const TEXT_VERTICAL: &str = "Vertical";
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
pub const EXT_HTML: &str = "html";
pub const INSERT_CHAPTER_FILE: &str = "Chapter from File\u{2026}";
pub const INSERT_CHAPTER_HTML: &str = "Chapter from Copied HTML";
pub const IMPORT_CHAPTER_TITLE: &str = "Import a Chapter";
pub const IMPORT_CHAPTER_FILTER: &str =
    "Chapters (*.html, *.htm, *.epub, *.lbtd, *.odt, *.docx, *.md, *.rtf)";
pub const IMPORT_CLIPBOARD_EMPTY: &str = "The clipboard has no text to import. Copy a chapter's HTML (for example from AO3's HTML editor) and try again.";
pub const IMPORT_CHAPTER_FAILED: &str = "The chapter could not be imported:";
pub const IMPORT_READ_ONLY: &str = "Leave Compare with Posted before importing a chapter.";
pub const EXT_EPUB: &str = "epub";
pub const FILE_FILTER_HTML: &str = "Web Page (*.html)";
pub const FILE_FILTER_PLAIN_HTML: &str = "Plain Web Page (*.html)";
pub const HTML_LOSS_TITLE: &str = "Saving as HTML will lose some formatting";
pub const IMPORTED_TITLE: &str = "Opened as a new document";
pub const IMPORTED_DETAIL: &str = "This file was imported (an AO3 download or an EPUB), so the original is left as it was. Use Save to keep the document in a format of your choice.";

/// "Untitled.odt — Edited", as Pages shows the document name and its save state.
pub fn window_title(name: &str, extension: &str, edited: bool) -> String {
    let state = if edited { EDITED } else { SAVED };
    format!("{name}.{extension} — {state}")
}

/// "Page 12 of 486 · 104,233 words", or the selected part when there is a selection.
pub fn word_count_pill(page: usize, pages: usize, words: usize, selected: Option<usize>) -> String {
    let noun = if words == 1 { "word" } else { "words" };
    let words = group_thousands(words);
    match selected {
        Some(selected) => {
            let selected = group_thousands(selected);
            format!("Page {page} of {pages} · {selected} selected of {words} {noun}")
        }
        None => format!("Page {page} of {pages} · {words} {noun}"),
    }
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
pub const SECTION_RUBY: &str = "Ruby";
pub const RUBY_READING: &str = "Reading (furigana, pinyin)";
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
pub const SPELL_SUGGESTIONS: &str = "Suggestions";
pub const SPELL_NO_SUGGESTIONS: &str = "No suggestions";
pub const SPELL_ADD_DICTIONARY: &str = "Add to dictionary";
pub const SPELL_IGNORE_ONCE: &str = "Ignore once";
pub const SPELL_IGNORE_DOCUMENT: &str = "Ignore in this document";
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
pub const SETTINGS_BENCHMARK: &str = "Benchmark";
pub const SETTINGS_BENCHMARK_RUN: &str = "Run benchmark";
pub const SETTINGS_BENCHMARK_HINT: &str = "Scrolls, types into and saves a generated document of growing size to find the largest one this computer handles at 60 frames per second (and 90 or 120 where the display allows). It can take several minutes, nothing in your own document is touched, the window cannot be used while it runs, and the result is saved so you can look at it again.";
pub const SETTINGS_BENCHMARK_LAST: &str = "Show last result";
pub const BENCH_REPORT_TITLE: &str = "Libertum benchmark";
pub const BENCH_RUNNING: &str = "Benchmark running";
pub const BENCH_CANCEL: &str = "Cancel";
pub const BENCH_CLOSE: &str = "Close";
pub const BENCH_COPY: &str = "Copy report";
pub const BENCH_COPIED: &str = "Copied";
pub const BENCH_STEP: &str = "Step";
pub const BENCH_PHASE_LAYOUT: &str = "Laying out the pages";
pub const BENCH_PHASE_SCROLL: &str = "Scrolling";
pub const BENCH_PHASE_TYPING: &str = "Typing";
pub const BENCH_DISPLAY: &str = "Display refresh rate (estimated from the lightest step):";
pub const BENCH_DISPLAY_SLOW: &str =
    "This computer did not reach 30 frames per second even on the smallest document.";
pub const BENCH_UNSUPPORTED: &str = "not available (the display does not refresh that fast)";
pub const BENCH_NOT_REACHED: &str = "not reached, even on the smallest test document; best was";
pub const BENCH_UP_TO: &str = "up to";
pub const BENCH_AT_LEAST: &str = "at least";
pub const BENCH_PAGES: &str = "pages";
pub const BENCH_WORDS: &str = "words";
pub const BENCH_MISSED_AT: &str = "missed at";
pub const BENCH_NO_LIMIT: &str = "no limit found in the sizes tested";
pub const BENCH_COL_TARGET: &str = "Target";
pub const BENCH_COL_LARGEST: &str = "Largest size";
pub const BENCH_COL_WORDS: &str = "Words";
pub const BENCH_COL_FIRST_MISS: &str = "First miss";
pub const BENCH_COL_PAGES: &str = "Pages";
pub const BENCH_COL_SCROLL: &str = "Scroll fps";
pub const BENCH_COL_SLOW: &str = "Slow frames";
pub const BENCH_COL_TYPING: &str = "Typing ms";
pub const BENCH_COL_SAVE: &str = "Save ms";
pub const BENCH_TABLE_NA: &str = "n/a";
pub const BENCH_TABLE_DISPLAY: &str = "display limit";
pub const BENCH_TABLE_NONE: &str = "none found";
pub const BENCH_TABLE_NOT_REACHED: &str = "not reached";
pub const BENCH_SAVED_TO: &str = "Saved to";
pub const BENCH_SAVED_AT: &str = "Saved at";
pub const BENCH_NOT_SAVED: &str = "This result could not be saved (no data folder).";
pub const BENCH_NOT_SAVED_BECAUSE: &str = "This result could not be saved:";
pub const BENCH_NO_RESULT: &str = "no result";
pub const BENCH_TABLE_HEAD: &str = " Pages     Words  Scroll fps Typing p95    Save ms\n(scroll: mean / slow-frame fps; typing and save in ms)";
pub const BENCH_SAVE_ESTIMATE: &str = "Estimated save time, scaled from the largest size measured";
pub const BENCH_NOTE: &str = "Typing is the time to apply a keystroke and update the layout, not the full time to the screen. Save is the time to build the file and write it to disk. Figures depend on the window size, zoom and what else the computer is doing.";
pub const SETTINGS_UPDATES: &str = "Check for updates automatically";
pub const SETTINGS_SPELLING: &str = "Spelling";
pub const SETTINGS_SPELLCHECK: &str = "Check spelling as you type";
pub const SETTINGS_SPELL_LANGUAGE: &str = "Default language";
pub const SETTINGS_SPELL_HINT: &str = "Underlines are checked on this machine. Right-click a word for suggestions, to add it to your dictionary, or to ignore it. New documents start in the default language; each document then keeps its own, which you set in the Document pane.";
pub const SETTINGS_UPDATES_HINT: &str = "Ask GitHub once a day whether a newer release exists. When you choose Update now, Libertum downloads the new version, checks it and installs it by itself; for a Linux package the system's own password prompt appears, and nothing is installed without you. No document or account information is sent.";
pub const SETTINGS_CHECK_NOW: &str = "Check now";
pub const SETTINGS_DEVELOPER: &str = "Developer";
pub const SETTINGS_DEV_MODE: &str = "Developer mode";
pub const SETTINGS_DEV_MODE_HINT: &str = "Switches on features meant for developers and power users. They are off until you turn this on, and turning it off switches them all off again.";
pub const SETTINGS_MCP: &str = "MCP connectors";
pub const SETTINGS_MCP_HINT: &str = "Runs a local MCP server so other MCP clients can read the open document. It listens on this computer only and needs the token below.";
pub const SETTINGS_MCP_ALLOW_EDITS: &str = "Allow edits";
pub const SETTINGS_MCP_ALLOW_EDITS_HINT: &str = "Lets connected tools change your document. Each tool still asks you before its first edit, and every edit is one step that Undo takes back. Off, they can only read.";
pub const SETTINGS_MCP_FORGET: &str = "Forget approved tools";
pub const MCP_APPROVAL_TITLE: &str = "wants to edit your document";
pub const MCP_APPROVAL_DETAIL: &str = "The name is whatever the tool calls itself, so allow only tools you started. You can undo any edit.";
pub const MCP_ALLOW_SESSION: &str = "Allow this session";
pub const MCP_ALLOW_ALWAYS: &str = "Always allow";
pub const MCP_DENY: &str = "Deny";
pub const MCP_EDIT_INSERT: &str = "Insert text";
pub const MCP_EDIT_REPLACE: &str = "Replace text";
pub const MCP_EDIT_DELETE: &str = "Delete text";
pub const MCP_EDIT_FORMAT: &str = "Change formatting";
pub const MCP_EDIT_STYLE: &str = "Set paragraph style";
pub const MCP_EDIT_AT: &str = "at paragraph";
pub const MCP_EDIT_FROM: &str = "from paragraph";
pub const MCP_EDIT_TO: &str = "to paragraph";
pub const MCP_EDIT_OFFSET: &str = "offset";
pub const MCP_EDIT_NEW_TEXT: &str = "New text:";
pub const MCP_EDIT_OLD_TEXT: &str = "Replaces:";
pub const MCP_EDIT_STYLE_NAMED: &str = "Style:";
pub const SETTINGS_SCRIPT_MEMORY: &str = "Memory one recipe script may use";
pub const SETTINGS_SCRIPT_TOTAL_MEMORY: &str = "Memory all stopped scripts may leave behind";
pub const SETTINGS_SCRIPT_TOTAL_MEMORY_HINT: &str = "Memory from a script that went over its limit is only given back when Libertum closes. Once the stopped scripts together reach this, no script runs until you restart Libertum.";
pub const SCRIPT_MEMORY_128: &str = "128 MB";
pub const SETTINGS_SCRIPT_MEMORY_HINT: &str = "A script is stopped if it holds more than this.";
pub const SCRIPT_MEMORY_256: &str = "256 MB";
pub const SCRIPT_MEMORY_512: &str = "512 MB";
pub const SCRIPT_MEMORY_1024: &str = "1 GB";
pub const SCRIPT_MEMORY_2048: &str = "2 GB";
pub const SCRIPT_MEMORY_4096: &str = "4 GB";
pub const SETTINGS_MCP_TOOLS: &str = "MCP tool runner…";
pub const RUNNER_TITLE: &str = "MCP tool runner";
pub const RUNNER_HINT: &str = "Connect to another MCP program or web address, pick one of its tools and run it yourself. Nothing runs until you press a button, and no assistant is involved.";
pub const RUNNER_SERVERS: &str = "Programs";
pub const RUNNER_NO_SERVERS: &str = "None saved yet. Add one below.";
pub const RUNNER_NAME_PLACEHOLDER: &str = "Name, for example Notes";
pub const RUNNER_COMMAND_PLACEHOLDER: &str =
    "Command or https:// address, for example npx -y some-mcp-server";
pub const RUNNER_ADD: &str = "Add";
pub const RUNNER_REMOVE: &str = "Remove";
pub const RUNNER_CONNECT: &str = "Connect";
pub const RUNNER_DISCONNECT: &str = "Disconnect";
pub const RUNNER_CONNECTING: &str = "Starting…";
pub const RUNNER_CONNECTED: &str = "Connected";
pub const RUNNER_APPROVE_TITLE: &str = "This will run the program below on your computer:";
pub const RUNNER_APPROVE_DETAIL: &str = "It runs with your permissions and can do anything you can. Approve it only if you know what it is. Changing the command asks again.";
pub const RUNNER_APPROVE_WEB_TITLE: &str = "This will send what you run to the web address below:";
pub const RUNNER_APPROVE_WEB_DETAIL: &str = "The server sees the text you give its tools and decides what to send back. Approve it only if you trust it. Changing the address asks again.";
pub const RUNNER_TOKEN_PLACEHOLDER: &str = "Token for an https:// address (optional)";
pub const RUNNER_TOKEN_SAVED: &str =
    "Sends your saved token (kept as plain text in the settings file) to this address only.";
pub const RUNNER_ERR_TOKEN: &str = "The server was saved, but the token was not: it must be one word, and only an https:// or localhost address takes one.";
pub const RUNNER_APPROVE_AND_CONNECT: &str = "Approve and connect";
pub const RUNNER_TOOLS: &str = "Tools";
pub const RUNNER_NO_TOOLS: &str = "This program offers no tools.";
pub const RUNNER_NO_ARGUMENTS: &str = "This tool takes no arguments.";
pub const RUNNER_REQUIRED: &str = "required";
pub const RUNNER_JSON_HINT: &str = "JSON";
pub const RUNNER_RUN: &str = "Run";
pub const RUNNER_RUNNING: &str = "Running…";
pub const RUNNER_RESULT: &str = "Result";
pub const RUNNER_RESULT_ERROR: &str = "The tool reported an error";
pub const RUNNER_RESULT_EMPTY: &str = "(nothing was returned)";
pub const RUNNER_RESULT_CUT: &str = "The result was cut because it was very long.";
pub const RUNNER_INSERT: &str = "Insert at cursor";
pub const RUNNER_COPY: &str = "Copy";
pub const RUNNER_CLOSE: &str = "Close";
pub const MACROS_TITLE: &str = "Macros";
pub const MACROS_HINT: &str = "A macro is a small recipe file (TOML) that reads your selection, calls tools of the connected program and puts the answer in your document. A whole run is one undo step. Recipes live in:";
pub const MACROS_NONE: &str =
    "No recipes yet. Put a .toml file in the folder above, then press Reload.";
pub const MACROS_RELOAD: &str = "Reload";
pub const MACROS_RUN: &str = "Run";
pub const MACROS_STOP: &str = "Stop";
pub const MACROS_RUNNING: &str = "Running";
pub const MACROS_NEEDS_SERVER: &str = "Connect this program first:";
pub const MACROS_DONE: &str = "Finished:";
pub const MACROS_STOPPED: &str = "Stopped:";
pub const MACROS_BAD_FILE: &str = "Cannot be used:";
pub const MACROS_ERR_NO_DOCUMENT: &str = "There is no document open.";
pub const MACROS_ERR_NO_SELECTION: &str = "Select some text first.";
pub const MACROS_ERR_SELECTION_CHANGED: &str = "the selection changed after it was read.";
pub const MACROS_ERR_DOCUMENT_CHANGED: &str = "the document was edited while the recipe ran.";
pub const MACROS_ERR_CUT: &str =
    "a tool result was cut because it was very long, so it was not used.";
pub const MACROS_ERR_TOO_MUCH: &str = "the recipe is holding too much text.";
pub const MACROS_ERR_DISCONNECTED: &str = "the program was disconnected.";
pub const MACROS_ERR_STOPPED: &str = "stopped by you.";
pub const RUNNER_ERR_ADD: &str =
    "Give the program a name and a command, and check the command has no unclosed quote.";
pub const RUNNER_ERR_FULL: &str = "The list is full. Remove a program first.";
pub const RUNNER_ERR_COMMAND_EMPTY: &str = "The command is empty.";
pub const RUNNER_ERR_COMMAND_QUOTE: &str = "The command has a quote that is never closed.";
pub const RUNNER_ERR_COMMAND_CONTROL: &str = "The command must be on one line.";
pub const RUNNER_ERR_CONNECT: &str = "Could not connect:";
pub const RUNNER_ERR_CALL: &str = "The call failed:";
pub const RUNNER_ERR_MISSING: &str = "Fill in the required field:";
pub const RUNNER_ERR_NUMBER: &str = "Not a number:";
pub const RUNNER_ERR_INTEGER: &str = "Not a whole number:";
pub const RUNNER_ERR_JSON: &str = "Not valid JSON:";
pub const SETTINGS_MCP_ADDRESS: &str = "Address:";
pub const SETTINGS_MCP_COPY_TOKEN: &str = "Copy token";
pub const SETTINGS_MCP_FAILED: &str = "The MCP server could not start:";
pub const UPDATES_CHECKING: &str = "Checking…";
pub const UPDATES_UP_TO_DATE: &str = "Libertum is up to date.";
pub const UPDATES_FAILED: &str = "Could not check for updates.";
pub const UPDATES_TITLE: &str = "A new version of Libertum is available";
pub const UPDATES_HINT: &str = "Libertum downloads it, checks it and installs it for you when it restarts. Your documents and settings are not affected.";
pub const UPDATES_HINT_PAGE: &str =
    "Open the release page to download it. Your documents and settings are not affected.";
pub const UPDATES_WORKING_TITLE: &str = "Downloading the update…";
pub const UPDATES_WORKING_DETAIL: &str = "This can take a minute. Libertum will restart by itself when the update is ready. You will be asked about unsaved changes first.";
pub const UPDATES_FAILED_TITLE: &str = "The update could not be installed automatically";
pub const UPDATES_OPEN_PAGE: &str = "Open the release page";
pub const UPDATES_COMMAND_TITLE: &str = "Finish the update in a terminal";
pub const UPDATES_COMMAND_DETAIL: &str = "The new version was downloaded and checked, but this computer has no system password prompt (pkexec), so run this command in a terminal, then start Libertum again:";
pub const UPDATES_SAVED_TO: &str = "Saved to";
pub const UPDATES_COPY_COMMAND: &str = "Copy command";
pub const UPDATES_CLOSE: &str = "Close";
pub const UPDATES_UPDATE: &str = "Update now";
pub const UPDATES_LATER: &str = "Remind me later";
pub const UPDATES_MUTE_WEEK: &str = "Mute 1 week";
pub const UPDATES_MUTE_MONTH: &str = "Mute 1 month";
pub const UPDATES_MUTE_VERSION: &str = "Skip this version";
pub const UPDATES_DISABLE: &str = "Turn off checks";
pub const UPDATES_OPEN_FAILED: &str = "Could not open the release page.";
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

// Page colour (Document pane).
pub const SECTION_PAGE_COLOUR: &str = "Page Colour";
pub const PAGE_COLOUR_NONE: &str = "White";
pub const PAGE_COLOUR_HEX_PLACEHOLDER: &str = "e.g. #F5EBDC";
pub const PAGE_COLOUR_APPLY: &str = "Apply";
pub const PAGE_COLOUR_BAD_HEX: &str = "Type six hex digits, such as #F5EBDC.";

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

pub const BORDER_TOP: &str = "Top";
pub const BORDER_RIGHT: &str = "Right";
pub const BORDER_BOTTOM: &str = "Bottom";
pub const BORDER_LEFT: &str = "Left";

// Compare with posted (View menu and the compare bar; AO3 plan stage B).
pub const VIEW_COMPARE: &str = "Compare with Posted";
pub const COMPARE_WORKING: &str = "Comparing with the posted copies\u{2026}";
pub const COMPARE_NONE_POSTED: &str = "No chapter is marked as posted yet. Put the caret in a chapter you have posted and choose Mark Chapter as Posted.";
pub const COMPARE_NO_FILE: &str =
    "Save the document first: posted copies are kept beside its file.";
pub const COMPARE_NOT_IN_CHAPTER: &str =
    "Put the caret in a chapter first (text before the first chapter heading is not compared).";
pub const COMPARE_MARK_FAILED: &str = "The posted copy could not be kept:";
pub const COMPARE_PREVIOUS: &str = "Previous";
pub const COMPARE_NEXT: &str = "Next";
pub const COMPARE_KEEP: &str = "Keep";
pub const COMPARE_REVERT: &str = "Revert to Posted";
pub const COMPARE_MARK: &str = "Mark Chapter as Posted";
pub const COMPARE_DONE: &str = "Done";

pub fn compare_no_changes(posted_chapters: usize) -> String {
    match posted_chapters {
        1 => "No changes since the chapter was posted.".to_owned(),
        n => format!("No changes in the {n} posted chapters."),
    }
}

pub fn compare_count(changes: usize, posted_chapters: usize) -> String {
    let changes = if changes == 1 {
        "1 change".to_owned()
    } else {
        format!("{changes} changes")
    };
    let chapters = if posted_chapters == 1 {
        "1 posted chapter".to_owned()
    } else {
        format!("{posted_chapters} posted chapters")
    };
    format!("{changes} since posting, in {chapters}. Choose Next to go through them.")
}

pub fn compare_position(position: usize, total: usize, chapter: &str) -> String {
    format!("Change {position} of {total} \u{b7} {chapter}")
}

/// The posted text of a change, shortened for the bar.
pub fn compare_posted_text(text: &str) -> String {
    const MAX: usize = 80;
    let short: String = text.chars().take(MAX).collect();
    let more = if text.chars().count() > MAX {
        "\u{2026}"
    } else {
        ""
    };
    format!("Posted: \u{201c}{short}{more}\u{201d}")
}

pub fn compare_marked(chapter: &str) -> String {
    format!("\u{201c}{chapter}\u{201d} is marked as posted.")
}

pub fn compare_too_large(chapter: &str, error: &str) -> String {
    format!("\u{201c}{chapter}\u{201d} could not be compared: {error}")
}

// Mark as Posted (the AO3 chapter window).
pub const AO3_POST_HINT: &str = "After pasting this chapter into AO3 and posting it, mark it here so Compare with Posted can show what you change later.";
pub const AO3_POST_MARK: &str = "Mark as Posted";
pub const AO3_POST_SAVE_FIRST: &str =
    "Save the document first: posted copies are kept beside its file.";

// The tag library (the AO3 tag vocabulary window).
pub const TAG_LIBRARY_TITLE: &str = "Tag Library";
pub const TAG_LIBRARY_HINT: &str = "Tags you copy are remembered here, with the full names of your characters and your pairings, so later chapters and stories suggest them. Everything stays on this computer.";
pub const TAG_STARTER_LIST: &str = "Also suggest common AO3 additional tags (built-in list)";
pub const TAG_IMPORT: &str = "Import Tags\u{2026}";
pub const TAG_FORGET_LEARNED: &str = "Forget Learned";
pub const TAG_IMPORT_TITLE: &str = "Import Tags";
pub const TAG_IMPORT_FILTER: &str = "Tag lists (*.txt, *.csv)";
pub const TAG_IMPORT_WORKING: &str = "Reading the tag file\u{2026}";
pub const TAG_IMPORT_FAILED: &str = "The tag file could not be read:";
pub const TAG_USED_BEFORE: &str = "used before";

pub fn tag_library_counts(learned: usize, imported: usize) -> String {
    format!("{learned} learned, {imported} imported")
}

pub fn tag_import_done(read: usize, added: usize) -> String {
    match (read, added) {
        (0, _) => "No tags were found in that file. Use one tag per line (optionally \u{201c}Character: Name\u{201d}), or a CSV with name and type columns.".to_owned(),
        (_, 0) => format!("{read} tags read; the library already had all of them."),
        _ => format!("{read} tags read, {added} added to the library."),
    }
}

// Tags hidden for one work (the AO3 chapter window).
pub const TAG_DISMISS_TIP: &str = "Not about this work: hide it in every chapter";
pub const TAG_SHOW_AGAIN: &str = "Show Again";

pub fn tags_hidden_for_work(count: usize) -> String {
    match count {
        1 => "1 tag is hidden for this work (copied already, or removed).".to_owned(),
        n => format!("{n} tags are hidden for this work (copied already, or removed)."),
    }
}

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
            word_count_pill(12, 486, 104_233, None),
            "Page 12 of 486 · 104,233 words"
        );
        assert_eq!(word_count_pill(1, 1, 1, None), "Page 1 of 1 · 1 word");
        assert_eq!(
            word_count_pill(12, 486, 104_233, Some(1_234)),
            "Page 12 of 486 · 1,234 selected of 104,233 words"
        );
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
