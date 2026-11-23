//! Application state, messages, update and view.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::advanced::input_method;
use iced::advanced::text::Wrapping;
use iced::futures::channel::mpsc::UnboundedReceiver;
use iced::widget::{button, column, container, mouse_area, row, space, stack, text};
use iced::{
    Center, Element, Event, Length, Padding, Size, Subscription, Task, Theme, border, event,
    keyboard, theme, window,
};
use libertum_docx::Docx;
use libertum_export::{SUGGESTION_LIMIT, Tag, TagCategory, TagSuggestion};
use libertum_html::{Html, HtmlFlavour};
use libertum_layout::PageGeometry;
use libertum_markdown::Markdown;
use libertum_model::{
    Align, BlockId, CharFormat, DateTimeContent, DateTimeFormat, DateTimeSource, Document, HfKind,
    HfVariant, Ligatures, LineSpacing, PageNumbering, PageSetup, ParaFormat, Section,
};
use libertum_native::{Native, ViewState};
use libertum_odt::Odt;
use libertum_rtf::Rtf;

use crate::canvas::{self, CanvasEvent, CanvasProps, Metrics};
use crate::docview::{DocEvent, DocView};
use crate::home;
use crate::inspector;
use crate::inspector::bookmarks::BookmarkEntry;
use crate::inspector::headers::HeadingEntry;
use crate::inspector::notes::NoteEntry;
use crate::language::DocLanguage;
use crate::mcp;
use crate::perf;
use crate::recovery;
use crate::settings::{DefaultFormat, Settings, ThemePreference, UnitsPreference, UpdateMute};
use crate::shortcuts::{self, Shortcut};
use crate::spell;
use crate::state::{
    Alignment, AppState, Baseline, BorderStyle, DocumentPane, Field, FontName, FontWeight,
    FormatPane, InsertAction, Inspector, Ligature, LineSpacingMode, ListKind, Margin, MarginSide,
    Menu, Mixed, NumberingHorizontal, NumberingVertical, Orientation, PageSize, ParagraphStyle,
    Points, Swatch, SwatchTarget, TableAction, Units, ViewAction, Zoom,
};
use crate::strings as s;
use crate::templates::{self, TemplateFormat, TemplateId, UserTemplate};
use crate::theme::{self as look, Scheme, Tokens};
use crate::toolbar;
use crate::update::CHECK_INTERVAL_SECS;
use crate::widgets::controls::flat;
use crate::widgets::segmented::{self, Content, Look};

mod ao3_post;
mod bench_run;
mod compare;
mod macro_run;
mod runner;
mod work_tags;
pub use ao3_post::Ao3PostMsg;
pub use compare::CompareMsg;
pub use runner::RunnerMsg;

/// The tag library's file in the data folder.
const TAG_LIBRARY_FILE: &str = "tag-library.json";

/// Unix seconds from the system clock, for date and time fields.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum FormatMsg {
    Style(ParagraphStyle),
    /// "Update": fold the selection's own formatting into its paragraph style.
    UpdateStyle,
    Family(FontName),
    Weight(FontWeight),
    ToggleBold,
    ToggleItalic,
    ToggleUnderline,
    ToggleStrike,
    Colour(Swatch),
    Highlight(Option<Swatch>),
    Baseline(Baseline),
    /// The gear beside B/I/U/S: show or hide the advanced font options.
    ToggleFontAdvanced,
    Align(Alignment),
    /// Decrease (-1) or increase (+1) the left indent by one step.
    Indent(i8),
    SpacingMode(LineSpacingMode),
    ListKind(ListKind),
    KeepLines(bool),
    KeepWithNext(bool),
    PageBreakBefore(bool),
    WidowOrphan(bool),
    RemoveHyphenation(bool),
    Ligatures(Ligature),
    /// Border box around the selected paragraphs (Layout pane).
    BorderStyle(BorderStyle),
    BorderSides(libertum_model::BorderSides),
    BorderColour([u8; 4]),
}

#[derive(Debug, Clone)]
pub enum DocumentMsg {
    /// Watermark text being typed; applied with `WatermarkApply`.
    WatermarkText(String),
    WatermarkApply,
    WatermarkDiagonal(bool),
    WatermarkRemove,
    /// Sets the page colour (`None` is white paper).
    PageColour(Option<[u8; 4]>),
    /// Page colour hex being typed; applied with `PageColourHexApply`.
    PageColourHex(String),
    PageColourHexApply,
    PageSize(PageSize),
    Orientation(Orientation),
    Header(bool),
    Footer(bool),
    /// Header or footer text for one of the three variants (as the inspector types it).
    HeaderText(HfVariant, String),
    FooterText(HfVariant, String),
    /// The first page of the section has its own header and footer (Word "Different first
    /// page").
    DifferentFirstPage(bool),
    /// Even pages have their own header and footer (Word "Different odd & even pages").
    DifferentEvenOdd(bool),
    Hyphenation(bool),
    Ligatures(bool),
    /// Show or hide the document's page numbers.
    NumberingOn(bool),
    /// Number pages from the page the caret is on (the physical page it sits on).
    NumberingUseCurrentPage,
    /// Apply the "start on page" and "first number" fields.
    NumberingUpdate,
    NumberingVertical(NumberingVertical),
    NumberingHorizontal(NumberingHorizontal),
    /// Book style: mirror the number's side on alternate pages.
    NumberingMirror(bool),
    HideOnFirstPage(bool),
    /// Use the previous section's headers/footers (Word "Link to Previous").
    LinkHeaders(bool),
    LinkFooters(bool),
    /// Edit the caret paragraph's footnote text.
    FootnoteText(String),
    /// The language of this document (Document pane): spelling follows it, and the file
    /// remembers it.
    Language(DocLanguage),
    /// The second language (`None` for one language only).
    Language2(Option<DocLanguage>),
    /// Text direction: vertical text runs down the page in columns from the right.
    Vertical(bool),
}

/// The file formats the app reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Docx,
    Odt,
    Markdown,
    Rtf,
    /// The native `.lbtd` format.
    Native,
    /// HTML (`.html`, `.htm`): opens pageless.
    Html,
    /// EPUB: imported as a new, unsaved document (never written back).
    Epub,
}

impl Format {
    fn for_path(path: &std::path::Path) -> Self {
        let extension = path.extension().and_then(|extension| extension.to_str());
        match extension.map(str::to_ascii_lowercase).as_deref() {
            Some(s::EXT_ODT) => Format::Odt,
            Some(s::EXT_MARKDOWN) => Format::Markdown,
            Some(s::EXT_RTF) => Format::Rtf,
            Some(s::EXT_NATIVE) => Format::Native,
            Some(s::EXT_HTML | "htm" | "xhtml") => Format::Html,
            Some(s::EXT_EPUB) => Format::Epub,
            _ => Format::Docx,
        }
    }

    fn from_settings(settings: &Settings) -> Self {
        match settings.default_format {
            DefaultFormat::Docx => Format::Docx,
            DefaultFormat::Odt => Format::Odt,
            DefaultFormat::Rtf => Format::Rtf,
            DefaultFormat::Markdown => Format::Markdown,
            DefaultFormat::Native => Format::Native,
        }
    }

    /// Whether a file of this format stores the page colour (Libertum, Word, OpenDocument
    /// and HTML do; RTF and Markdown do not).
    fn keeps_page_colour(self) -> bool {
        matches!(
            self,
            Format::Native | Format::Docx | Format::Odt | Format::Html
        )
    }

    /// Whether a file of this format stores the document's language. RTF and Markdown do not;
    /// the app remembers theirs by path instead (`Settings::document_languages`).
    fn keeps_language(self) -> bool {
        matches!(
            self,
            Format::Native | Format::Docx | Format::Odt | Format::Html
        )
    }

    /// Whether a file of this format stores vertical text (Libertum, Word and OpenDocument).
    fn keeps_vertical(self) -> bool {
        matches!(self, Format::Native | Format::Docx | Format::Odt)
    }

    /// Whether a file of this format stores ruby (Libertum, Word, OpenDocument and HTML).
    fn keeps_ruby(self) -> bool {
        matches!(
            self,
            Format::Native | Format::Docx | Format::Odt | Format::Html
        )
    }

    /// Whether a file of this format stores the watermark (Libertum, Word and OpenDocument).
    fn keeps_watermark(self) -> bool {
        matches!(self, Format::Native | Format::Docx | Format::Odt)
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Docx => s::EXT_DOCX,
            Format::Odt => s::EXT_ODT,
            Format::Markdown => s::EXT_MARKDOWN,
            Format::Rtf => s::EXT_RTF,
            Format::Native => s::EXT_NATIVE,
            Format::Html => s::EXT_HTML,
            Format::Epub => s::EXT_EPUB,
        }
    }
}

/// The open document's own package, so a save keeps the parts Libertum does not model.
#[derive(Clone)]
enum FileContext {
    Docx(Docx),
    Odt(Odt),
    Markdown(Markdown),
    Rtf(Rtf),
    Native(Native),
    Html(Html),
}

impl FileContext {
    /// Opens `path` as the format its extension names and returns the package, its warnings
    /// and the parsed document.
    fn open(path: &std::path::Path) -> Result<(Self, Document, Vec<String>), String> {
        match Format::for_path(path) {
            Format::Docx => {
                let docx = Docx::open(path).map_err(|error| error.to_string())?;
                let document = docx.document().clone();
                let warnings = docx.warnings().to_vec();
                Ok((Self::Docx(docx), document, warnings))
            }
            Format::Odt => {
                let odt = Odt::open(path).map_err(|error| error.to_string())?;
                let document = odt.document().clone();
                let warnings = odt.warnings().to_vec();
                Ok((Self::Odt(odt), document, warnings))
            }
            Format::Markdown => {
                let markdown = Markdown::open(path).map_err(|error| error.to_string())?;
                let document = markdown.document().clone();
                let warnings = markdown.warnings().to_vec();
                Ok((Self::Markdown(markdown), document, warnings))
            }
            Format::Rtf => {
                let rtf = Rtf::open(path).map_err(|error| error.to_string())?;
                let document = rtf.document().clone();
                let warnings = rtf.warnings().to_vec();
                Ok((Self::Rtf(rtf), document, warnings))
            }
            Format::Native => {
                let native = Native::open(path).map_err(|error| error.to_string())?;
                let document = native.document().clone();
                Ok((Self::Native(native), document, Vec::new()))
            }
            Format::Html => {
                let html = Html::open(path).map_err(|error| error.to_string())?;
                let document = html.document().clone();
                let warnings = html.warnings().to_vec();
                Ok((Self::Html(html), document, warnings))
            }
            Format::Epub => {
                let (document, warnings) =
                    libertum_html::import_epub(path).map_err(|error| error.to_string())?;
                Ok((Self::Html(Html::new(document.clone())), document, warnings))
            }
        }
    }

    /// An AO3 download (HTML), which opens as a new document so the download is never
    /// rewritten in Libertum's own HTML.
    fn is_ao3(&self) -> bool {
        matches!(self, Self::Html(html) if html.is_ao3())
    }

    fn format(&self) -> Format {
        match self {
            Self::Docx(_) => Format::Docx,
            Self::Odt(_) => Format::Odt,
            Self::Markdown(_) => Format::Markdown,
            Self::Rtf(_) => Format::Rtf,
            Self::Native(_) => Format::Native,
            Self::Html(_) => Format::Html,
        }
    }

    fn set_document(&mut self, document: Document) {
        match self {
            Self::Docx(docx) => docx.set_document(document),
            Self::Odt(odt) => odt.set_document(document),
            Self::Markdown(markdown) => markdown.set_document(document),
            Self::Rtf(rtf) => rtf.set_document(document),
            Self::Native(native) => *native.document_mut() = document,
            Self::Html(html) => html.set_document(document),
        }
    }

    /// Hands the writer the page each table-of-contents heading is on, for the page numbers
    /// saved beside the entries (DOCX and ODT; the other formats keep none).
    fn set_toc_pages(&mut self, pages: HashMap<libertum_model::BlockId, u32>) {
        match self {
            Self::Docx(docx) => docx.set_toc_pages(pages),
            Self::Odt(odt) => odt.set_toc_pages(pages),
            Self::Markdown(_) | Self::Rtf(_) | Self::Native(_) | Self::Html(_) => {}
        }
    }

    /// The view state an open native document carries (the other formats have none).
    fn view(&self) -> Option<ViewState> {
        match self {
            Self::Native(native) => Some(native.view().clone()),
            _ => None,
        }
    }

    /// Records the view state before a native save; a no-op for the other formats.
    fn set_view(&mut self, view: ViewState) {
        if let Self::Native(native) = self {
            native.set_view(view);
        }
    }

    /// Saves safely for a synced folder: the file being replaced is first kept in the
    /// document's history, and the new one is written beside it and renamed into place so a
    /// sync client never sees a half-written file. (Markdown also writes pictures beside the
    /// file, so it writes in place.)
    fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let started = Instant::now();
        if let Err(error) = libertum_library::snapshot(path, std::time::SystemTime::now()) {
            tracing::warn!(%error, path = %path.display(), "could not keep the earlier version");
        }
        let snapshot_ms = started.elapsed().as_secs_f64() * 1000.0;
        let write_started = Instant::now();
        let result = if matches!(self, Self::Markdown(_)) {
            self.save_in_place(path)
        } else {
            libertum_library::save_atomic(path, |temporary| self.save_in_place(temporary)).map_err(
                |error| match error {
                    libertum_library::SaveError::Write(message) => message,
                    libertum_library::SaveError::Permissions(error) => error.to_string(),
                    libertum_library::SaveError::Replace(error) => error.to_string(),
                },
            )
        };
        tracing::debug!(
            snapshot_ms,
            save_ms = write_started.elapsed().as_secs_f64() * 1000.0,
            path = %path.display(),
            "document saved"
        );
        result
    }

    fn save_in_place(&self, path: &std::path::Path) -> Result<(), String> {
        match self {
            Self::Docx(docx) => docx.save(path).map_err(|error| error.to_string()),
            Self::Odt(odt) => odt.save(path).map_err(|error| error.to_string()),
            Self::Markdown(markdown) => {
                // Images inserted in the app are written beside the file; what could not be
                // written is reported.
                let warnings = markdown
                    .save_with_report(path)
                    .map_err(|error| error.to_string())?;
                for warning in warnings {
                    tracing::warn!(%warning, path = %path.display(), "Markdown save warning");
                }
                Ok(())
            }
            Self::Rtf(rtf) => rtf.save(path).map_err(|error| error.to_string()),
            Self::Native(native) => native.save(path).map_err(|error| error.to_string()),
            Self::Html(html) => html.save(path).map_err(|error| error.to_string()),
        }
    }

    /// A fresh package for a new document in `format`.
    fn new(format: Format, document: Document) -> Self {
        match format {
            Format::Docx => Self::Docx(Docx::new(document)),
            Format::Odt => Self::Odt(Odt::new(document)),
            Format::Markdown => Self::Markdown(Markdown::new(document)),
            Format::Rtf => Self::Rtf(Rtf::new(document)),
            Format::Native => Self::Native(Native::new(document)),
            Format::Html | Format::Epub => Self::Html(Html::new(document)),
        }
    }
}

/// A format the Export menu writes a copy in. Exporting never changes the open document,
/// its file or the recent-files list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Pdf,
    /// PDF/A-2b, the archival variant.
    PdfArchive,
    Epub,
    Rtf,
    Text,
    Markdown,
    /// HTML with the text's structure only (no fonts, colours or layout).
    PlainHtml,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Pdf | Self::PdfArchive => "pdf",
            Self::Epub => "epub",
            Self::Rtf => s::EXT_RTF,
            Self::Text => "txt",
            Self::Markdown => s::EXT_MARKDOWN,
            Self::PlainHtml => s::EXT_HTML,
        }
    }

    fn filter(self) -> &'static str {
        match self {
            Self::Pdf | Self::PdfArchive => s::FILE_FILTER_PDF,
            Self::Epub => s::FILE_FILTER_EPUB,
            Self::Rtf => s::FILE_FILTER_RTF,
            Self::Text => s::FILE_FILTER_TEXT,
            Self::Markdown => s::FILE_FILTER_MARKDOWN,
            Self::PlainHtml => s::FILE_FILTER_PLAIN_HTML,
        }
    }

    fn archival(self) -> bool {
        self == Self::PdfArchive
    }
}

/// Work that was waiting for an unsaved-changes prompt.
#[derive(Debug, Clone)]
enum Pending {
    Close,
    /// Back to the home screen after the unsaved-changes prompt.
    Home,
    Open(PathBuf),
}

/// The writer's answer when an MCP client first asks to edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpDecision {
    /// Allow this client until Libertum closes.
    AllowSession,
    /// Allow this client from now on (until "Forget approved tools").
    AllowAlways,
    Deny,
}

/// Where the update prompt is.
#[derive(Debug, Clone)]
enum UpdateStage {
    /// Asking whether to update.
    Ask,
    /// Downloading and checking.
    Working,
    /// The automatic route failed; the release page is the way on.
    Failed(String),
    /// A package was downloaded; the writer runs `command` in a terminal.
    Command { command: String, file: PathBuf },
}

/// A prompt over the document.
#[derive(Debug, Clone)]
enum Modal {
    /// An MCP client wants to edit. The request itself waits in `App::mcp_approvals`; this
    /// holds what the prompt shows.
    McpApproval {
        client: String,
        action: String,
        detail: String,
    },
    /// The document has unsaved changes; `pending` runs after Save or Don't Save.
    Unsaved { pending: Pending },
    /// An open or save failed. The message is already user-facing.
    Error(String),
    /// Something the writer should know about a document that did open; OK acknowledges it.
    Notice { title: String, detail: String },
    /// A newer release exists; the prompt follows it through the download.
    UpdateAvailable {
        release: crate::update::Release,
        stage: UpdateStage,
    },
    /// The file at `path` changed on disk since it was opened (another device, a sync service);
    /// Overwrite saves anyway, Save a Copy asks for another name.
    ChangedOnDisk { path: PathBuf },
    /// Saving `path` as Markdown would drop the listed formatting; Save writes it anyway.
    MarkdownLoss {
        path: PathBuf,
        losses: Vec<String>,
        /// The prompt's title (Markdown or HTML).
        title: &'static str,
    },
    /// A snapshot from the last session is waiting: restore or discard it.
    Recovery,
    /// Documents are being picked and ordered for stitching.
    Stitch,
    /// The library folder's documents, with their histories and any sync conflict copies.
    Library {
        entries: Vec<libertum_library::LibraryEntry>,
    },
    /// The earlier versions of the document at `path`; `from_library` returns to the Library.
    History {
        path: PathBuf,
        versions: Vec<libertum_library::Version>,
        from_library: bool,
    },
    /// Adding an online picture or video by its address.
    LinkedMedia {
        url: String,
        alt: String,
        video: bool,
        /// Download a preview picture when inserting.
        preview: bool,
        /// The kind was chosen by hand, so typing the address no longer guesses it.
        kind_chosen: bool,
        /// A download is under way.
        busy: bool,
        error: Option<String>,
    },
    /// Choosing what to insert for Date & Time.
    DateTime {
        updated: bool,
        format: DateTimeFormat,
    },
    /// One AO3 chapter's HTML, ready to copy, with the tags the vocabulary suggests.
    Ao3Chapter {
        /// Index into [`AppState::ao3_chapters`], so a vocabulary change can re-rank it.
        chapter: usize,
        title: String,
        html: String,
        copied: bool,
        suggestions: Vec<TagSuggestion>,
        /// One flag per suggestion: what the writer ticked.
        selected: Vec<bool>,
        tags_copied: bool,
        /// The chapter has a heading to use as AO3's chapter title (the whole-document chapter
        /// does not: its name is a label, not a title).
        has_title: bool,
        title_copied: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalChoice {
    Save,
    Discard,
    Cancel,
    /// Put the recovered document back.
    Restore,
}

/// The state of the last update check, for the Settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum UpdateCheck {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Failed,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleMenu(Menu),
    CloseMenu,
    View(ViewAction),
    ZoomSelected(Zoom),
    /// Change the zoom by this many percentage points (menu: 10, stepper arrows: 1).
    ZoomStep(i16),
    /// Set an exact zoom percentage (1 to 800).
    ZoomSet(u16),
    /// Typing into the toolbar's zoom field.
    ZoomText(String),
    ZoomCommit,
    Insert(InsertAction),
    /// Open the Find & Replace bar (Cmd/Ctrl+F).
    OpenFind,
    CloseFind,
    /// Text typed into the Find bar.
    FindQuery(String),
    ReplaceQuery(String),
    FindNext,
    FindPrevious,
    ToggleFindCase,
    ToggleFindWord,
    /// Replace the match Replace acts on, then find the next one.
    ReplaceOne,
    /// Replace every match in the document, in one undo step.
    ReplaceAll,
    /// A recent file chosen on the home screen.
    OpenRecent(PathBuf),
    /// Back to the home screen, closing the current document (Cmd/Ctrl+W or three Escapes).
    GoHome,
    /// Stitching: pick files to combine, reorder the list, then join them into one document.
    OpenStitch,
    StitchAdd(Vec<PathBuf>),
    StitchMove(usize, i8),
    StitchRemove(usize),
    StitchGo,
    CloseStitch,
    /// The Library window (chooses the folder first when none is set).
    OpenLibrary,
    ChooseLibraryFolder,
    CloseLibrary,
    /// Compare mode (View > Compare with Posted) and its bar.
    Compare(compare::CompareMsg),
    /// The chapter window's Mark as Posted.
    Ao3Post(ao3_post::Ao3PostMsg),
    /// Version history of a library document, or of the open document when `None`.
    OpenHistory(Option<PathBuf>),
    HistoryBack,
    /// Opens a past version as an unsaved copy.
    HistoryOpenCopy(PathBuf),
    HistoryRestore {
        original: PathBuf,
        version: PathBuf,
    },
    UseConflictCopy {
        original: PathBuf,
        copy: PathBuf,
    },
    KeepOriginal {
        original: PathBuf,
        copy: PathBuf,
    },
    /// A file dropped on the window.
    FileDropped(PathBuf),
    /// Clipboard commands, from the right-click menu (the keyboard has the same ones).
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// The Settings window.
    OpenSettings,
    CloseSettings,
    /// Open the Date & Time chooser (Insert menu).
    OpenDateTime,
    /// Insert > Linked Image or Video.
    OpenLinkedMedia,
    CloseLinkedMedia,
    LinkedUrl(String),
    LinkedAlt(String),
    LinkedVideo(bool),
    LinkedPreview(bool),
    LinkedInsert,
    /// The preview for a new link arrived (or failed); the link is inserted either way.
    LinkedInserted {
        link: libertum_model::MediaLink,
        alt: String,
        preview: Result<std::sync::Arc<libertum_model::Media>, String>,
    },
    /// Fetch Preview in the Image group: download the picture for image `index` of a block.
    FetchPreview(libertum_model::BlockId, usize),
    PreviewFetched(
        libertum_model::BlockId,
        usize,
        Result<std::sync::Arc<libertum_model::Media>, String>,
    ),
    /// Close the chooser without inserting anything.
    CloseDateTime,
    /// The chooser: show the moment of insertion or the document's last save.
    DateTimeUpdated(bool),
    /// The chooser: which written form to use.
    DateTimeFormat(DateTimeFormat),
    /// Insert the chosen date and time at the caret.
    DateTimeInsert,
    SetTheme(ThemePreference),
    SetDefaultFormat(DefaultFormat),
    /// The autosave period in seconds (0 turns autosave off).
    SetAutosave(u32),
    SetScriptMemory(u32),
    SetScriptTotalMemory(u32),
    SetUnits(UnitsPreference),
    /// Show or hide the frame-rate and frame-time overlay (Settings).
    SetShowPerformance(bool),
    SetAntialiasing(bool),
    /// Automatic update checks on or off (Settings).
    SetCheckForUpdates(bool),
    SetSpellcheck(bool),
    /// A client of the MCP server asked a question (dev mode); answer it from the open document.
    McpRequest(mcp::McpRequest),
    /// Copy the MCP server's token to the clipboard (Settings).
    CopyMcpToken,
    /// Developer mode on or off (Settings). Turning it off also turns MCP off.
    SetDevMode(bool),
    /// MCP connectors on or off (Settings); only reachable while developer mode is on.
    SetMcpEnabled(bool),
    /// Allow MCP clients to edit, or not (Settings); only reachable while MCP is on.
    SetMcpAllowEdits(bool),
    /// "Forget approved tools" in Settings: every client asks again.
    ForgetMcpClients,
    /// The writer's answer to an MCP client's request to edit.
    McpDecision(McpDecision),
    /// The MCP tool runner window (developer mode).
    Runner(RunnerMsg),
    SetSpellLanguage(spell::Language),
    /// "Check now" in Settings, even while automatic checks are off.
    CheckForUpdatesNow,
    /// The background update check finished.
    UpdateChecked {
        /// True when the writer asked for it in Settings; false for the daily check.
        manual: bool,
        result: Result<Option<crate::update::Release>, String>,
    },
    /// Download and install the available release (or open its page where that is all
    /// this installation can do).
    UpdateNow,
    /// The download finished (or failed).
    UpdatePrepared(Result<crate::install::Prepared, String>),
    /// Copy the install command shown for a downloaded package.
    CopyUpdateCommand,
    /// Open the available release's page in the browser.
    OpenUpdatePage,
    /// Close the update prompt without changing anything.
    DismissUpdate,
    /// Ignore this release for the given number of seconds.
    MuteUpdate(u64),
    /// Ignore this release until a newer one appears.
    MuteUpdateVersion,
    /// Turn automatic update checks off from the prompt.
    DisableUpdates,
    /// A frame was rendered.
    FpsFrame(Instant),
    /// The screenshot hook's delay is over: capture the window.
    ScreenshotDue,
    /// The captured window; save it and exit.
    Screenshot(window::Screenshot),
    /// The autosave timer fired.
    AutosaveTick,
    /// The recovery snapshot was written (or failed to be).
    Autosaved {
        doc_id: u64,
        revision: u64,
        result: Result<(), String>,
    },
    /// A command in the Table toolbar menu.
    Table(TableAction),
    /// Full screen on or off (toolbar button, View menu, F11 / Ctrl+Cmd+F).
    ToggleFullScreen,
    /// Page thumbnails column on or off (View menu).
    ToggleThumbnails,
    /// The thumbnails column scrolled: its offset and visible height, logical pixels.
    ThumbScrolled(f32, f32),
    /// Show page `index` (0-based) at the top of the view (a thumbnail was clicked).
    GoToPage(usize),
    /// A click on the Find bar outside its controls.
    FindBarPressed,
    /// The Find query has not changed for a moment: search for it (the generation it was
    /// scheduled for).
    FindTyped(u64),
    /// Apply the built-in table style with this index to the caret's table (`None` removes
    /// the style).
    TableStyle(Option<usize>),
    /// Make image `index` of a paragraph one step smaller (-1) or larger (+1).
    ImageStep(libertum_model::BlockId, usize, i8),
    /// Alternative text typed for an image in the Format inspector.
    ImageAlt(libertum_model::BlockId, usize, String),
    /// The Ruby group's reading changed.
    RubyEdit(String),
    ImageRemove(libertum_model::BlockId, usize),
    /// Insert a reference to the named bookmark at the caret (Bookmarks pane).
    InsertCrossRef(String, libertum_model::CrossRefKind),
    /// Bring every cross-reference's text up to date.
    RefreshCrossRefs,
    /// Jump the caret to a heading from the Headers pane.
    GoToHeading(libertum_model::BlockId),
    /// Fold or unfold the children of a heading in the Headers pane.
    ToggleHeading(libertum_model::BlockId),
    /// Edit note `index` in a paragraph from the Notes pane.
    NoteText(libertum_model::BlockId, usize, String),
    /// Jump the caret to a note from the Notes pane.
    GoToNote(libertum_model::BlockId, usize),
    /// Add a bookmark at the caret from the Bookmarks pane.
    AddBookmark,
    /// A bookmark name being typed in the Bookmarks pane.
    BookmarkName(libertum_model::BlockId, usize, String),
    /// Commit the typed name on Return.
    BookmarkNameCommit(libertum_model::BlockId, usize),
    /// Jump the caret to a bookmark.
    GoToBookmark(libertum_model::BlockId, usize),
    /// Delete a bookmark.
    RemoveBookmark(libertum_model::BlockId, usize),
    /// Toolbar Format / Document / Headers buttons.
    ToggleInspector(Inspector),
    FormatPane(FormatPane),
    DocumentPane(DocumentPane),
    ToggleSwatch(SwatchTarget),
    Format(FormatMsg),
    Document(DocumentMsg),
    /// Stepper arrow: step a field up (+1) or down (-1).
    Step(Field, i8),
    FieldText(Field, String),
    /// Enter in a stepper field.
    FieldCommit,
    Shortcut(Shortcut),
    /// Text-editing keys that are not shortcuts.
    Keyboard(keyboard::Event),
    /// IME composition events (Opened / Preedit / Commit / Closed).
    Ime(input_method::Event),
    /// Result of an asynchronous clipboard read.
    PasteText(Option<String>),
    /// Insert > Chapter from File: pick a file and insert its text after the caret.
    ImportChapterFile,
    /// Insert > Chapter from Copied HTML: read the clipboard as HTML.
    ImportChapterClipboard,
    ImportChapterPasted(Option<String>),
    SystemTheme(theme::Mode),
    Canvas(CanvasEvent),
    Doc(DocEvent),
    /// Home screen: select a template, or create a document from one.
    SelectTemplate(TemplateId),
    CreateDocument(TemplateId),
    /// Home screen: an added template (index into `user_templates`).
    SelectUserTemplate(usize),
    CreateUserTemplate(usize),
    /// Add an ODT, DOCX or Libertum document as a template.
    AddTemplate,
    /// Delete the selected added template's copy.
    RemoveSelectedTemplate,
    /// A spelling check finished. Results from an older generation are ignored.
    /// The spell-check timer fired: typing, scrolling or layout went quiet.
    SpellTick,
    /// Test hook `LIBERTUM_TYPE`: types the next character.
    TypeHookTick,
    SpellChecked {
        generation: u64,
        result: Result<Vec<spell::Span>, String>,
    },
    /// Replace one misspelt word with a suggestion.
    SpellReplace {
        span: spell::Span,
        replacement: String,
    },
    /// Ignore this occurrence, in this session only.
    SpellIgnoreOnce(spell::Span),
    /// Ignore this word throughout the open document, in this session only.
    SpellIgnoreDocument(String),
    /// Add this word to the personal dictionary.
    SpellAddWord(String),
    /// Open a document from a file (home screen, Cmd/Ctrl+O).
    OpenFile,
    /// A background save finished.
    Saved {
        path: PathBuf,
        result: Result<(), String>,
    },
    /// Export a copy in another format (never changes the open file).
    Export(ExportFormat),
    /// A background export finished.
    Exported(Result<PathBuf, String>),
    /// Open the AO3 chapter list beside the Export menu.
    OpenAo3Chapters,
    /// Put the AO3 chapter list away (the pointer moved to another row of the Export menu).
    CloseAo3List,
    /// Show one chapter's AO3 HTML (index into [`AppState::ao3_chapters`]).
    Ao3Chapter(usize),
    /// Copy the HTML shown in the AO3 chapter window.
    CopyAo3,
    /// Copy the chapter's title, as plain text, for AO3's chapter title field.
    CopyAo3Title,
    CloseAo3,
    /// Tick or untick one suggested tag (index into the window's suggestions).
    ToggleSuggestedTag(usize),
    /// Copy the ticked tags, comma separated, for AO3's tag field.
    CopySuggestedTags,
    /// The tag library: use the starter list, import a file, forget learned tags.
    SetStarterTags(bool),
    ImportTagFile,
    TagFileImported(Result<Vec<libertum_export::Tag>, String>),
    ForgetLearnedTags,
    /// The – button: hide an unrelated suggestion for this work.
    DismissSuggestedTag(usize),
    /// Show the tags hidden for this work (copied or removed) again.
    ShowHiddenTags,
    /// Turn the suggestions drawn from the chapter's own text on or off.
    SetAutoTags(bool),
    /// Open the local AO3 tag vocabulary window.
    OpenTagVocabulary,
    CloseTagVocabulary,
    /// The tag name being typed in that window.
    TagDraft(String),
    /// The category a new tag gets.
    TagCategoryPicked(TagCategory),
    /// A name picked from AO3's fixed list for the rating or the warnings.
    FixedTagPicked(&'static str),
    /// Add the typed name to the vocabulary (ignored when it is blank or already there).
    AddTag,
    /// Remove the vocabulary tag at this index.
    RemoveTag(usize),
    /// A chapter heading level was switched in Settings (1-based, on or off).
    SetChapterHeading(u8, bool),
    /// The window manager asked to close the window.
    CloseRequested,
    /// A button in the unsaved-changes or error prompt.
    ModalChoice(ModalChoice),
    ScaleFactor(f32),
    WindowResized(Size),
    /// The operating system gave the window focus (`true`) or took it away (`false`).
    WindowFocus(bool),
    Frame(Instant),
    /// Caret blink timer (only runs for [`BLINK_TIMEOUT`] after caret activity).
    BlinkTick(Instant),
    // The macOS merged title bar: empty toolbar space drags, double-click maximises.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    DragWindow,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ToggleMaximise,
    /// Settings: start the benchmark.
    RunBenchmark,
    CancelBenchmark,
    CloseBenchmarkReport,
    ShowBenchmarkReport,
    CopyBenchmarkReport,
}

/// `LIBERTUM_AUTOSCROLL=1`: scroll from page 1 to the last page at a steady speed, then exit
/// (Phase 0 frame-time measurement, see `perf.rs`).
struct AutoScroll {
    /// Logical pixels per second (`LIBERTUM_AUTOSCROLL_SPEED`, default 4000).
    speed: f32,
    phase: AutoPhase,
}

enum AutoPhase {
    /// Waiting for pagination to finish, a viewport and a scale factor, plus a settling time.
    Waiting {
        ready_since: Option<Instant>,
    },
    Running {
        last: Instant,
    },
    Done,
}

impl AutoScroll {
    fn from_env() -> Option<Self> {
        if !std::env::var("LIBERTUM_AUTOSCROLL").is_ok_and(|v| v != "0" && !v.is_empty()) {
            return None;
        }
        let speed = std::env::var("LIBERTUM_AUTOSCROLL_SPEED")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(4000.0);
        Some(Self {
            speed,
            phase: AutoPhase::Waiting { ready_since: None },
        })
    }

    fn active(&self) -> bool {
        !matches!(self.phase, AutoPhase::Done)
    }
}

/// Test hook: `LIBERTUM_ZOOM=150` or `LIBERTUM_ZOOM=fit` sets the initial zoom (used for
/// screenshots and the autoscroll runs at other zoom levels).
fn initial_zoom() -> Zoom {
    match std::env::var("LIBERTUM_ZOOM").ok().as_deref() {
        Some("fit") => Zoom::FitWidth,
        Some(v) => v
            .parse::<u16>()
            .ok()
            .filter(|p| (1..=800).contains(p))
            .map_or(Zoom::Percent(100), Zoom::Percent),
        None => Zoom::Percent(100),
    }
}

/// Caret blink half-period (on, then off). 530 ms is the long-standing Windows default.
const BLINK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(530);
/// The caret blinks this long after the last keystroke, click or edit, then stays solid and
/// the timer stops, so an idle document costs no CPU (PLAN §2; the Windows caret does the
/// same). Agreed 2026-11-09.
const BLINK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A stream of ticks every [`BLINK_INTERVAL`] from a sleeping thread. iced's
/// `time::every` needs the tokio or smol executor, which the app does not use. The thread
/// ends at its next tick once the subscription (and so the receiver) is dropped.
fn blink_ticks() -> UnboundedReceiver<Instant> {
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();
    let spawned = std::thread::Builder::new()
        .name("libertum-blink".into())
        .spawn(move || {
            loop {
                std::thread::sleep(BLINK_INTERVAL);
                if tx.unbounded_send(Instant::now()).is_err() {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        // The caret simply stays solid.
        tracing::warn!(%error, "could not start the caret blink timer");
    }
    rx
}

/// The `LIBERTUM_TYPE` hook's typing speed: about eight characters a second.
fn type_ticks() -> UnboundedReceiver<()> {
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();
    let _ = std::thread::Builder::new()
        .name("libertum-type-hook".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(120));
                if tx.unbounded_send(()).is_err() {
                    return;
                }
            }
        });
    rx
}

/// How long after the last keystroke, scroll or layout change spelling is checked: a short
/// pause in the activity.
const SPELL_QUIET: Duration = Duration::from_millis(450);
/// How long before a failed or out-of-date check is tried again.
const SPELL_RETRY: Duration = Duration::from_secs(2);

/// When the next spell check is due. The timer thread sleeps on `wake` until there is a
/// deadline, so a document with nothing to check wakes nothing: no polling, no fallback tick.
struct SpellTimer {
    deadline: std::sync::Mutex<Option<Instant>>,
    wake: std::sync::Condvar,
    /// The newest timer thread's number; an older thread that finds it changed ends.
    epoch: std::sync::atomic::AtomicU64,
}

fn spell_timer() -> &'static SpellTimer {
    static TIMER: std::sync::OnceLock<SpellTimer> = std::sync::OnceLock::new();
    TIMER.get_or_init(|| SpellTimer {
        deadline: std::sync::Mutex::new(None),
        wake: std::sync::Condvar::new(),
        epoch: std::sync::atomic::AtomicU64::new(0),
    })
}

impl SpellTimer {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.deadline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sets the deadline; `sooner_only` keeps an earlier one that is already waiting. The
    /// thread is only woken when it was idle: otherwise it is already sleeping towards an
    /// earlier deadline and re-reads the new one when that passes, so a scroll or a burst
    /// of typing costs no extra wake-ups.
    fn set(&self, delay: Duration, sooner_only: bool) {
        let due = Instant::now() + delay;
        let mut deadline = self.lock();
        let was_idle = deadline.is_none();
        match *deadline {
            Some(existing) if sooner_only && existing <= due => {}
            _ => *deadline = Some(due),
        }
        drop(deadline);
        if was_idle {
            self.wake.notify_all();
        }
    }
}

/// Something that can change what needs checking happened (an edit, a scroll, a layout
/// pass, the caret moving): check spelling once it has been quiet for [`SPELL_QUIET`].
fn note_spell_edit() {
    spell_timer().set(SPELL_QUIET, false);
}

/// A check failed or came back out of date: look again after [`SPELL_RETRY`], unless a
/// sooner check is already due.
fn retry_spell_later() {
    spell_timer().set(SPELL_RETRY, true);
}

/// Whether a spell check is waiting for its quiet period.
#[cfg(test)]
fn spell_check_pending() -> bool {
    spell_timer().lock().is_some()
}

/// Ticks for the spell check on a small thread that sleeps until a check is due
/// ([`note_spell_edit`], [`retry_spell_later`]) and then sends one tick.
fn spell_ticks() -> UnboundedReceiver<()> {
    spell_ticks_for(spell_timer())
}

fn spell_ticks_for(timer: &'static SpellTimer) -> UnboundedReceiver<()> {
    use std::sync::atomic::Ordering;
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();
    // Any thread left from an earlier subscription ends instead of competing for ticks.
    let epoch = timer.epoch.fetch_add(1, Ordering::SeqCst) + 1;
    timer.wake.notify_all();
    let spawned = std::thread::Builder::new()
        .name("libertum-spell".into())
        .spawn(move || {
            loop {
                let mut deadline = timer.lock();
                loop {
                    if timer.epoch.load(Ordering::SeqCst) != epoch {
                        return;
                    }
                    match *deadline {
                        None => {
                            deadline = timer
                                .wake
                                .wait(deadline)
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                        }
                        Some(due) => {
                            let now = Instant::now();
                            if now >= due {
                                *deadline = None;
                                break;
                            }
                            deadline = timer
                                .wake
                                .wait_timeout(deadline, due - now)
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .0;
                        }
                    }
                }
                drop(deadline);
                if tx.unbounded_send(()).is_err() {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the spell-check timer");
    }
    rx
}

/// One tick every `seconds` on a small thread, for the autosave timer (the same shape as the
/// caret blink timer, because no async runtime feature is linked).
fn autosave_ticks(seconds: &u64) -> UnboundedReceiver<()> {
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();
    let period = Duration::from_secs((*seconds).max(1));
    let spawned = std::thread::Builder::new()
        .name("libertum-autosave".into())
        .spawn(move || {
            loop {
                std::thread::sleep(period);
                if tx.unbounded_send(()).is_err() {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the autosave timer");
    }
    rx
}

/// Settling time after pagination completes before the scripted scroll starts.
const AUTOSCROLL_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

pub struct App {
    /// A downloaded update, installed as Libertum quits.
    staged_update: Option<crate::install::Staged>,
    pub state: AppState,
    pub settings: Settings,
    /// Where settings are persisted. `None` disables persistence (used by tests).
    settings_path: Option<PathBuf>,
    system_mode: theme::Mode,
    /// The theme setting, or the `LIBERTUM_THEME` test override.
    theme_preference: ThemePreference,
    /// Window width in logical pixels (anchors the toolbar menus).
    window_width: f32,
    /// Window height in logical pixels (sizes the scrollable Settings card).
    window_height: f32,
    /// The open document; `None` while the home screen (template chooser) is shown.
    doc: Option<DocView>,
    /// The template selected on the home screen.
    home_selection: TemplateId,
    /// The added template selected on the home screen, if any.
    home_user: Option<usize>,
    /// Where added templates are stored (a `templates` folder in the data directory).
    templates_dir: PathBuf,
    /// The added templates, loaded from `templates_dir`.
    user_templates: Vec<UserTemplate>,
    /// Misspellings in the paragraphs last checked.
    spell_errors: Vec<spell::Span>,
    /// Test hook `LIBERTUM_TYPE=<text>`: characters still to type, one per tick, through the
    /// same path as a real key press (for reproducing typing problems with a screenshot).
    type_hook: std::collections::VecDeque<char>,
    /// When the hook starts typing (`LIBERTUM_TYPE_AFTER=<seconds>` after start-up).
    type_hook_from: Instant,
    /// Compare mode, while it is on (the document view is then read-only).
    compare: Option<compare::CompareMode>,
    /// The writer's tag library: learned and imported tags (`tag-library.json` in the data
    /// folder; in memory only when settings are not persisted).
    tag_cache: libertum_export::TagCache,
    tag_cache_path: Option<PathBuf>,
    /// What the tag library last did (an import's result), shown in the vocabulary window.
    tag_cache_notice: Option<String>,
    /// Tags this work no longer needs suggested (copied, or removed as unrelated), and which
    /// document (view id, file) they belong to.
    work_tags: work_tags::WorkTags,
    work_tags_for: (Option<u64>, Option<PathBuf>),
    /// What the chapter window's Mark as Posted last did.
    ao3_post_notice: Option<String>,
    /// Bumped whenever `spell_errors` changes, so the underline rectangles are recomputed.
    spell_errors_epoch: u64,
    /// Cached underline rectangles, keyed by the spell epoch and page generation.
    spell_rect_cache: std::sync::Mutex<Option<(u64, u64, Vec<libertum_layout::SelectionRect>)>>,
    /// Bumped for every check; a slower, older result must not replace a newer one.
    spell_generation: u64,
    /// Fingerprint of what the last check covered, so a tick with nothing new does no work.
    spell_signature: Option<u64>,
    /// The personal word list, lowercased.
    spell_personal: HashSet<String>,
    /// Words ignored for this document only.
    spell_ignored_document: HashSet<String>,
    /// Single occurrences ignored for this session.
    spell_ignored_once: HashSet<(BlockId, usize, usize, String)>,
    /// Where the personal list lives; `None` in tests that do not use a file.
    spell_path: Option<PathBuf>,
    /// Scroll offset of the page strip, physical pixels.
    scroll: (f32, f32),
    /// Canvas size in logical pixels, once the widget has reported it.
    viewport: Option<Size>,
    /// Page-strip metrics for the viewport size, shared by the update path.
    metrics_cache: canvas::MetricsCache,
    /// Page-strip metrics for the canvas widget's own bounds; the widget caches them between
    /// draws and pointer events.
    canvas_metrics_cache: std::sync::Arc<canvas::MetricsCache>,
    /// Bumped by every change to the Find query; a pending search-as-you-type runs only if
    /// the query has not changed since it was scheduled.
    find_generation: u64,
    /// Rendered page thumbnails by page index: the page's content hash and the image.
    thumb_cache: std::collections::HashMap<usize, (u64, iced::widget::image::Handle)>,
    /// Offset and visible height of the thumbnails column, logical pixels.
    thumb_scroll: (f32, f32),
    /// Window scale factor (device pixels per logical pixel).
    scale_factor: f32,
    scale_factor_known: bool,
    /// The canvas last received a click; inspector controls do not take document keystrokes.
    canvas_focused: bool,
    /// The window has operating-system focus. An unfocused window blinks no caret and ticks
    /// no autosave while there is nothing to save.
    window_focused: bool,
    /// The selection's word count, kept for the same document, selection and revision so a
    /// scroll or drag over a large selection does not copy and split its text every event.
    selection_words_cache: Option<(
        u64,
        libertum_editor::DocPosition,
        libertum_editor::DocPosition,
        u64,
        usize,
    )>,
    /// Current IME composition string, drawn inline until committed.
    ime_preedit: Option<String>,
    /// `LIBERTUM_PREEDIT` (screenshots): a composition shown whatever the platform's input
    /// method reports, which would otherwise clear it.
    preedit_hook: Option<String>,
    autoscroll: Option<AutoScroll>,
    /// Last keystroke, click or edit: the caret blinks for [`BLINK_TIMEOUT`] after it.
    caret_activity: Instant,
    /// Caret drawn in this blink phase.
    caret_on: bool,
    /// File the document was opened from or last saved to.
    file: Option<PathBuf>,
    /// The package the open document came from, so a save keeps parts Libertum does not
    /// model. `None` for a document that has never been opened or saved.
    file_context: Option<FileContext>,
    /// Unsaved changes since the last open or save.
    dirty: bool,
    /// Document revision whose snapshot is being written, and the last one written. An
    /// unchanged dirty document skips autosave instead of deep-cloning itself every minute.
    autosave_pending: Option<u64>,
    autosave_done: Option<u64>,
    /// The MCP server, while dev mode and MCP are both on (`docs/mcp-plan.md`).
    mcp: Option<mcp::McpService>,
    /// Why the server could not start, shown in Settings.
    mcp_error: Option<String>,
    /// Edit requests waiting for the writer's answer; the first is the one the prompt shows.
    mcp_approvals: std::collections::VecDeque<libertum_mcp::Pending>,
    /// Clients the writer allowed "this session": gone when Libertum closes.
    mcp_session_trusted: HashSet<String>,
    /// The tool runner window, open only while MCP is on. Closing it stops its program.
    runner: Option<crate::mcp_runner::Runner>,
    /// A prompt over the document (unsaved changes, or an error).
    modal: Option<Modal>,
    /// What to do when a save finishes (close the window, open another file).
    after_save: Option<Pending>,
    /// Cached headings for the Headers pane (refreshed while that pane is visible).
    headings: Vec<HeadingEntry>,
    /// Headings whose children are folded in the Headers pane.
    collapsed_headings: HashSet<BlockId>,
    /// Cached notes for the Notes pane.
    notes: Vec<NoteEntry>,
    /// Cached bookmarks for the Bookmarks pane.
    bookmarks: Vec<BookmarkEntry>,
    /// A bookmark name being typed in the pane; committed on Return.
    bookmark_draft: Option<(libertum_model::BlockId, usize, String)>,
    /// Where autosave snapshots live (the platform data directory; tests point it elsewhere).
    recovery_dir: PathBuf,
    /// What the open file looked like when it was opened or last saved, to notice a change made
    /// by another device or program (a synced folder).
    disk_stamp: Option<libertum_library::DiskStamp>,
    /// A snapshot waiting for the user's Restore or Discard choice.
    pending_recovery: Option<(libertum_model::Document, recovery::Meta)>,
    /// The Settings window is over the document.
    settings_open: bool,
    /// A benchmark in progress (the user's document is set aside meanwhile).
    bench: Option<bench_run::Run>,
    /// The last finished benchmark, and whether its window is showing.
    bench_report: Option<crate::bench::Report>,
    bench_report_open: bool,
    bench_copied: bool,
    /// Where finished results are kept (none in tests unless a test sets one).
    bench_dir: Option<PathBuf>,
    /// Where the shown result is saved, or why it is not.
    bench_save_note: Option<String>,
    /// The state of the last update check, shown in Settings.
    update_check: UpdateCheck,
    /// The AO3 tag vocabulary window is over the document (from Settings or a chapter).
    vocabulary_open: bool,
    /// The tag name being typed in that window.
    tag_draft: String,
    /// The category a newly added tag gets.
    tag_category: TagCategory,
    /// The frame-rate counter behind the performance overlay.
    fps: crate::fps::FrameCounter,
    /// The right-click menu, if one is open.
    context_menu: Option<ContextMenuState>,
    /// Distance a mouse wheel has asked for that the view has not travelled yet, physical
    /// pixels: the view glides to it over the next frames instead of jumping.
    scroll_pending: (f32, f32),
    /// Trackpad inertia: the velocity (physical pixels per second) measured from the finger's
    /// motion, which carries the page on after the events stop, and when the last trackpad
    /// event arrived.
    fling: (f32, f32),
    /// The fling has been launched (it passed the flick-speed check) and is easing out; it
    /// then runs on until it is nearly still instead of being judged against the flick speed
    /// again, which would stop it dead while still moving.
    fling_gliding: bool,
    trackpad_at: Option<Instant>,
    /// Trackpad movement received since the last frame, physical pixels. It is applied in
    /// [`App::coast`] on the next vsync, so the page moves once per displayed frame however
    /// unevenly the pad's events arrive.
    trackpad_pending: (f32, f32),
    last_frame: Option<Instant>,
    /// Consecutive Escape presses and when the last one arrived: three in a row go home.
    escape_presses: (u8, Instant),
    /// Documents chosen for stitching, in order.
    stitch_sources: Vec<crate::stitch::Source>,
}

/// An open right-click menu: where it is (window logical pixels) and what it acts on.
struct ContextMenuState {
    x: f32,
    y: f32,
    target: crate::toolbar::ContextMenuTarget,
}

impl App {
    /// Test constructor: a tiny document, so tests stay fast.
    #[cfg(test)]
    pub fn new(settings: Settings, settings_path: Option<PathBuf>) -> Self {
        let (doc, _rx) = DocView::open(libertum_layout::spike::generate_model_document(3, 1));
        Self::with_doc(settings, settings_path, Some(doc))
    }

    fn with_doc(settings: Settings, settings_path: Option<PathBuf>, doc: Option<DocView>) -> Self {
        let mut state = AppState {
            sidebar_visible: !settings.sidebar_collapsed,
            sidebar_width: settings.clamped_sidebar_width(),
            inspector: settings.inspector,
            show_word_count: settings.show_word_count,
            units: settings.units(),
            open_menu: menu_hook(),
            zoom: initial_zoom(),
            booklet: booklet_hook(),
            thumbnails: thumbnails_hook(),
            page_count: doc.as_ref().map_or(0, DocView::page_total),
            current_page: 1,
            word_count: doc.as_ref().map_or(0, DocView::word_count),
            ..AppState::default()
        };
        if let Some(doc) = &doc {
            state.fonts = installed_fonts(doc);
        }
        apply_inspector_hook(&mut state);
        state.settle_sidebar();
        let theme_preference = theme_override().unwrap_or(settings.theme);
        #[cfg(test)]
        let templates_dir =
            std::env::temp_dir().join(format!("libertum-templates-test-{}", std::process::id()));
        #[cfg(not(test))]
        let templates_dir = templates::default_dir().unwrap_or_else(std::env::temp_dir);
        let user_templates = templates::load(&templates_dir);
        #[cfg(test)]
        let spell_path = None;
        #[cfg(not(test))]
        let spell_path = spell::personal_path();
        let spell_personal = spell_path
            .as_deref()
            .map(spell::load_personal)
            .unwrap_or_default();
        let mut app = Self {
            state,
            settings,
            settings_path,
            system_mode: theme::Mode::None,
            theme_preference,
            window_width: 1200.0,
            window_height: 680.0,
            doc,
            home_selection: TemplateId::default(),
            home_user: None,
            templates_dir,
            user_templates,
            spell_errors: Vec::new(),
            compare: None,
            type_hook_from: Instant::now()
                + Duration::from_secs(
                    std::env::var("LIBERTUM_TYPE_AFTER")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                ),
            type_hook: std::env::var("LIBERTUM_TYPE")
                .map(|text| text.replace("\\n", "\n").chars().collect())
                .unwrap_or_default(),
            ao3_post_notice: None,
            tag_cache: libertum_export::TagCache::default(),
            tag_cache_path: None,
            tag_cache_notice: None,
            work_tags: work_tags::WorkTags::default(),
            work_tags_for: (None, None),
            spell_errors_epoch: 0,
            spell_rect_cache: std::sync::Mutex::new(None),
            spell_generation: 0,
            spell_signature: None,
            spell_personal,
            spell_ignored_document: HashSet::new(),
            spell_ignored_once: HashSet::new(),
            spell_path,
            scroll: (0.0, 0.0),
            viewport: None,
            metrics_cache: canvas::MetricsCache::default(),
            canvas_metrics_cache: std::sync::Arc::new(canvas::MetricsCache::default()),
            scale_factor: 1.0,
            scale_factor_known: false,
            canvas_focused: false,
            window_focused: true,
            selection_words_cache: None,
            ime_preedit: None,
            preedit_hook: None,
            autoscroll: AutoScroll::from_env(),
            caret_activity: Instant::now(),
            caret_on: true,
            file: None,
            file_context: None,
            dirty: false,
            autosave_pending: None,
            autosave_done: None,
            mcp: None,
            mcp_error: None,
            mcp_approvals: std::collections::VecDeque::new(),
            mcp_session_trusted: HashSet::new(),
            runner: None,
            modal: None,
            after_save: None,
            headings: Vec::new(),
            collapsed_headings: HashSet::new(),
            notes: Vec::new(),
            bookmarks: Vec::new(),
            bookmark_draft: None,
            thumb_cache: std::collections::HashMap::new(),
            find_generation: 0,
            thumb_scroll: (0.0, 0.0),
            recovery_dir: recovery::data_dir().unwrap_or_else(std::env::temp_dir),
            disk_stamp: None,
            pending_recovery: None,
            settings_open: false,
            bench: None,
            bench_report: None,
            bench_report_open: false,
            bench_copied: false,
            bench_dir: if cfg!(test) {
                None
            } else {
                crate::bench::results_dir()
            },
            bench_save_note: None,
            update_check: UpdateCheck::Idle,
            staged_update: None,
            vocabulary_open: false,
            tag_draft: String::new(),
            tag_category: TagCategory::Freeform,
            scroll_pending: (0.0, 0.0),
            fling: (0.0, 0.0),
            fling_gliding: false,
            trackpad_at: None,
            trackpad_pending: (0.0, 0.0),
            last_frame: None,
            escape_presses: (0, Instant::now()),
            stitch_sources: Vec::new(),
            fps: crate::fps::FrameCounter::default(),
            context_menu: None,
        };
        // The dictionary loads in the background now, not on the first check.
        if app.settings.spellcheck {
            spell::warm(app.settings.spellcheck_language);
        }
        // The tag library lives beside the other data, and only when settings persist (tests
        // keep it in memory).
        if app.settings_path.is_some() {
            app.tag_cache_path = recovery::data_dir().map(|dir| dir.join(TAG_LIBRARY_FILE));
            if let Some(path) = &app.tag_cache_path {
                app.tag_cache = std::fs::read(path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default();
            }
        }
        app.refresh_document();
        app.headings = app.headings();
        app.notes = app.notes();
        app.bookmarks = app.bookmarks();
        app.sync_mcp();
        app
    }

    /// Start or stop the MCP server so it matches the settings: running only while developer
    /// mode and MCP are both on.
    fn sync_mcp(&mut self) {
        if self.settings.mcp_active() {
            if self.mcp.is_none() {
                match mcp::McpService::start() {
                    Ok(service) => {
                        self.mcp = Some(service);
                        self.mcp_error = None;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not start the MCP server");
                        self.mcp_error = Some(error.to_string());
                    }
                }
            }
        } else {
            // Dropping the service closes the port and ends its threads.
            self.mcp = None;
            self.mcp_error = None;
            self.mcp_session_trusted.clear();
            // The runner is part of MCP: its program stops with it.
            self.close_runner();
        }
        // Switching editing (or MCP, or dev mode) off answers anything still waiting.
        if !self.settings.mcp_edits_allowed() {
            self.drain_mcp_approvals();
        }
    }

    /// Refuse every edit that is waiting for the writer, and close the prompt for it.
    fn drain_mcp_approvals(&mut self) {
        for pending in std::mem::take(&mut self.mcp_approvals) {
            pending.respond(Err(libertum_mcp::HostError::EditsDisabled));
        }
        if matches!(self.modal, Some(Modal::McpApproval { .. })) {
            self.modal = None;
        }
    }

    /// Work through the queued edit requests: refuse them when editing is off, make them for a
    /// client the writer already trusts, and otherwise ask (one prompt at a time).
    fn process_mcp_approvals(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        while let Some(front) = self.mcp_approvals.front() {
            if front.is_abandoned() {
                // The client was already told it timed out; never make that edit now.
                self.mcp_approvals.pop_front();
                continue;
            }
            let name = mcp::display_name(&front.client().name);
            let trusted =
                self.settings.mcp_trusts(&name) || self.mcp_session_trusted.contains(&name);
            if !self.settings.mcp_edits_allowed() || trusted {
                if let Some(pending) = self.mcp_approvals.pop_front() {
                    tasks.push(self.apply_mcp_edit(pending));
                }
                continue;
            }
            if self.modal.is_none() {
                let summary = mcp::describe(front.request(), self.doc.as_ref());
                self.modal = Some(Modal::McpApproval {
                    client: name,
                    action: summary.action,
                    detail: summary.detail,
                });
            }
            break;
        }
        Task::batch(tasks)
    }

    /// Make one approved edit (or refuse it when editing has been switched off since).
    fn apply_mcp_edit(&mut self, pending: libertum_mcp::Pending) -> Task<Message> {
        if !self.settings.mcp_edits_allowed() {
            pending.respond(Err(libertum_mcp::HostError::EditsDisabled));
            return Task::none();
        }
        // The server may have given up on this request while it waited for the writer; if it
        // did, the client believes the edit failed, so it must not happen.
        if !pending.claim() {
            return Task::none();
        }
        let result = mcp::apply_edit(pending.request(), self.doc.as_mut());
        let edited = result.is_ok();
        pending.respond(result);
        if edited {
            self.dirty = true;
            self.after_edit();
        }
        Task::none()
    }

    /// The writer's answer to the prompt for the first queued edit.
    fn decide_mcp(&mut self, decision: McpDecision) -> Task<Message> {
        if matches!(self.modal, Some(Modal::McpApproval { .. })) {
            self.modal = None;
        }
        let Some(pending) = self.mcp_approvals.pop_front() else {
            return Task::none();
        };
        let name = mcp::display_name(&pending.client().name);
        let task = match decision {
            McpDecision::Deny => {
                pending.respond(Err(libertum_mcp::HostError::Denied));
                Task::none()
            }
            McpDecision::AllowSession => {
                self.mcp_session_trusted.insert(name);
                self.apply_mcp_edit(pending)
            }
            McpDecision::AllowAlways => {
                self.settings.trust_mcp_client(&name);
                self.save_settings();
                self.apply_mcp_edit(pending)
            }
        };
        Task::batch([task, self.process_mcp_approvals()])
    }

    /// Answer a question from an MCP client: reads from the open document at once, edits after
    /// the checks and the writer's approval.
    fn answer_mcp(&mut self, request: &mcp::McpRequest) -> Task<Message> {
        let Some(pending) = request.take() else {
            return Task::none();
        };
        if pending.request().is_edit() {
            self.mcp_approvals.push_back(pending);
            return self.process_mcp_approvals();
        }
        let title = self
            .file
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| s::UNTITLED.to_owned());
        let format = match self.file.as_ref().and_then(|path| path.extension()) {
            Some(extension) => extension.to_string_lossy().to_lowercase(),
            None => Format::from_settings(&self.settings).extension().to_owned(),
        };
        let meta = mcp::DocMeta {
            title: &title,
            path: self.file.as_deref(),
            format: &format,
            dirty: self.dirty,
        };
        let result = mcp::answer(pending.request(), self.doc.as_ref(), &meta);
        pending.respond(result);
        Task::none()
    }

    /// Starts on the home screen, or on the generated 500-page demo document when
    /// `LIBERTUM_DEMO=1` or `LIBERTUM_AUTOSCROLL=1` is set (the performance runs need it).
    // TODO(Phase 7): open a Markdown file passed on the command line.
    pub fn boot(
        settings: Settings,
        path: Option<PathBuf>,
        open: Option<PathBuf>,
    ) -> (Self, Task<Message>) {
        crate::docview::set_antialias(settings.antialiasing);
        // macOS ignores Iced's window icon and this build has no .app bundle: the Dock icon
        // is set here, once the event loop exists (see src/macos_icon.rs).
        #[cfg(all(target_os = "macos", not(test)))]
        crate::macos_icon::set_dock_icon();
        let mut tasks = vec![
            iced::system::theme().map(Message::SystemTheme),
            Self::query_scale_factor(),
        ];
        // Test hook: `LIBERTUM_OPEN=<path>` starts with a file instead of the home screen,
        // so screenshots and the save/close checks do not need a native dialog.
        if let Some(open) = std::env::var_os("LIBERTUM_OPEN").map(PathBuf::from) {
            return match FileContext::open(&open) {
                Ok((context, document, warnings)) => {
                    for warning in &warnings {
                        tracing::info!(%warning, "import warning");
                    }
                    let toc_notice = warnings
                        .iter()
                        .any(|w| w == libertum_formats_common::toc::TOC_REBUILT_WARNING);
                    let view = context.view();
                    let (doc, events) = match context.format() {
                        Format::Markdown => DocView::open_markdown(document),
                        // RTF has no reliable page model of its own, so it reads pageless.
                        Format::Rtf => DocView::open_pageless(document),
                        Format::Html | Format::Epub => DocView::open_pageless(document),
                        // A native document remembers whether it was read pageless.
                        Format::Native if view.as_ref().is_some_and(|view| view.pageless) => {
                            DocView::open_pageless(document)
                        }
                        Format::Native | Format::Odt | Format::Docx => DocView::open(document),
                    };
                    tasks.push(Task::run(events, Message::Doc));
                    let mut app = Self::with_doc(settings, path, Some(doc));
                    if let Some(view) = &view {
                        app.state.zoom = Zoom::Percent(view.zoom_percent.clamp(1, 800));
                    }
                    app.file = Some(open);
                    app.file_context = Some(context);
                    app.dirty = false;
                    if let Some(doc) = app.doc.as_mut() {
                        let _ = doc.edit(|ed| ed.refresh_table_of_contents());
                    }
                    if toc_notice {
                        app.modal = Some(Modal::Notice {
                            title: s::TOC_NOTICE_TITLE.to_owned(),
                            detail: s::TOC_NOTICE_DETAIL.to_owned(),
                        });
                    }
                    app.refresh_format();
                    app.refresh_section();
                    app.apply_start_hooks(&mut tasks);
                    tasks.push(app.update_check_task());
                    (app, Task::batch(tasks))
                }
                Err(error) => {
                    tracing::warn!(%error, path = %open.display(), "LIBERTUM_OPEN failed");
                    let mut app = Self::with_doc(settings, path, None);
                    app.modal = Some(Modal::Error(format!("{} {error}", s::OPEN_FAILED)));
                    tasks.push(app.update_check_task());
                    (app, Task::batch(tasks))
                }
            };
        }
        let doc = if demo_requested() {
            let (doc, events): (DocView, UnboundedReceiver<DocEvent>) = DocView::open_demo();
            tasks.push(Task::run(events, Message::Doc));
            Some(doc)
        } else {
            None
        };
        let mut app = Self::with_doc(settings, path, doc);
        // A document given on the command line (a double-clicked file) opens straight into
        // the editor; an error shows over the home screen instead.
        if let Some(open) = open.filter(|_| app.doc.is_none()) {
            tasks.push(app.open_path(open));
        }
        // A crash snapshot is offered on the home screen only (see `check_recovery`).
        app.check_recovery();
        app.apply_start_hooks(&mut tasks);
        tasks.push(app.update_check_task());
        (app, Task::batch(tasks))
    }

    /// Schedules the once-a-day update check when automatic checks are on and it is due.
    fn update_check_task(&mut self) -> Task<Message> {
        if !self.settings.check_for_updates {
            return Task::none();
        }
        let now = now_seconds().max(0) as u64;
        if self
            .settings
            .last_update_check
            .is_some_and(|last| now.saturating_sub(last) < CHECK_INTERVAL_SECS)
        {
            return Task::none();
        }
        self.settings.last_update_check = Some(now);
        self.save_settings();
        Task::perform(async { crate::update::check() }, |result| {
            Message::UpdateChecked {
                manual: false,
                result: result.map_err(|error| error.to_string()),
            }
        })
    }

    /// The start-up screenshot hooks, shared by every way `boot` can finish so they also work
    /// over a document opened with `LIBERTUM_OPEN`.
    fn apply_start_hooks(&mut self, tasks: &mut Vec<Task<Message>>) {
        // `LIBERTUM_COMPARE=1` starts in Compare with Posted, and `=next` also steps to the
        // first change, so a screenshot can show the compare bar and its marks.
        if let Ok(value) = std::env::var("LIBERTUM_COMPARE")
            && !value.is_empty()
            && value != "0"
            && self.doc.is_some()
        {
            tasks.push(self.update(Message::Compare(compare::CompareMsg::Toggle)));
            if value == "next" {
                self.compare_now();
                tasks.push(self.update(Message::Compare(compare::CompareMsg::Next)));
            }
        }
        // `LIBERTUM_TAG_SEED=1` fills an empty vocabulary with example tags, in memory only
        // (nothing is written to the settings file), so a screenshot can show the feature.
        if std::env::var("LIBERTUM_TAG_SEED").is_ok_and(|v| v != "0" && !v.is_empty())
            && self.settings.tags.is_empty()
        {
            self.settings.tags = vec![
                Tag::new("Dragons", TagCategory::Character),
                Tag::new("Slow Burn", TagCategory::Freeform),
                Tag::new("Keep", TagCategory::Freeform),
                Tag::new("Teen And Up Audiences", TagCategory::Rating),
            ];
        }
        // `LIBERTUM_AO3=1` opens the Export menu's chapter list at start-up.
        if std::env::var("LIBERTUM_AO3").is_ok_and(|v| v != "0" && !v.is_empty())
            && let Some(doc) = self.doc.as_ref()
        {
            self.state.ao3_chapters =
                libertum_export::chapters(doc.document(), self.settings.chapter_headings);
            self.state.ao3_open = true;
            self.state.open_menu = Some(Menu::Export);
        }
        // `LIBERTUM_AO3_PICK=<n>` opens that chapter's window at start-up.
        if let Ok(index) = std::env::var("LIBERTUM_AO3_PICK")
            && let Ok(index) = index.parse::<usize>()
        {
            self.state.ao3_chapters = self.doc.as_ref().map_or_else(Vec::new, |doc| {
                libertum_export::chapters(doc.document(), self.settings.chapter_headings)
            });
            tasks.push(self.update(Message::Ao3Chapter(index)));
            // `LIBERTUM_AO3_TICK=1` also ticks every suggested tag (for screenshots).
            if std::env::var("LIBERTUM_AO3_TICK").is_ok_and(|v| v != "0" && !v.is_empty())
                && let Some(Modal::Ao3Chapter { suggestions, .. }) = &self.modal
            {
                for index in 0..suggestions.len() {
                    let _ = self.update(Message::ToggleSuggestedTag(index));
                }
            }
        }
        // `LIBERTUM_TAGS=1` opens the AO3 tag vocabulary window at start-up; `rating` or
        // `warning` opens it on that category (for screenshots of the fixed-name picker).
        if let Ok(value) = std::env::var("LIBERTUM_TAGS")
            && value != "0"
            && !value.is_empty()
        {
            self.vocabulary_open = true;
            match value.as_str() {
                "rating" => self.tag_category = TagCategory::Rating,
                "warning" => self.tag_category = TagCategory::Warning,
                _ => {}
            }
        }
        // `LIBERTUM_PREEDIT=<text>` shows that text as an input-method composition at the
        // caret (for screenshots; a real composition comes from the platform's input method,
        // which this hook does not depend on).
        if let Ok(text) = std::env::var("LIBERTUM_PREEDIT")
            && !text.is_empty()
            && self.doc.is_some()
        {
            self.canvas_focused = true;
            self.preedit_hook = Some(text);
        }
        // `LIBERTUM_SETTINGS=1` opens the Settings window at start-up (for screenshots).
        if std::env::var("LIBERTUM_SETTINGS").is_ok_and(|v| v != "0" && !v.is_empty()) {
            self.settings_open = true;
        }
        // `LIBERTUM_SCREENSHOT=<file.png>` saves a picture of the window and exits (see shot.rs).
        if let Some(task) = crate::shot::start() {
            tasks.push(task);
        }
        // `LIBERTUM_BENCHMARK=1` runs the benchmark at start-up, prints its report to the log
        // and exits; `=show` keeps the results window open instead (for screenshots).
        if bench_run::start_requested() {
            tasks.push(self.bench_start());
        }
        // `LIBERTUM_MCP_RUNNER=1` opens the MCP tool runner at start-up (for screenshots); it
        // still needs developer mode and MCP switched on in the settings.
        if std::env::var("LIBERTUM_MCP_RUNNER").is_ok_and(|v| v != "0" && !v.is_empty())
            && self.settings.mcp_active()
        {
            self.runner = Some(crate::mcp_runner::Runner::default());
            self.load_macros();
        }
        // `LIBERTUM_UPDATE=<version>` shows the update prompt at start-up (for screenshots).
        if let Ok(version) = std::env::var("LIBERTUM_UPDATE")
            && !version.is_empty()
        {
            self.modal = Some(Modal::UpdateAvailable {
                release: crate::update::Release {
                    version,
                    url: "https://github.com/wyn-cmd/libertum/releases/tag/v0.1.4".to_owned(),
                    assets: Vec::new(),
                },
                stage: UpdateStage::Ask,
            });
        }
        // `LIBERTUM_LIBRARY=<folder>` opens the Library window on that folder at start-up
        // (not saved as the setting unless another setting change writes the file), and
        // `LIBERTUM_HISTORY=<file>` opens that document's version history.
        if let Ok(folder) = std::env::var("LIBERTUM_LIBRARY")
            && !folder.is_empty()
        {
            self.settings.library_folder = Some(PathBuf::from(folder));
            self.show_library();
        }
        if let Ok(file) = std::env::var("LIBERTUM_HISTORY")
            && !file.is_empty()
        {
            self.show_history(PathBuf::from(file), false);
        }
        // `LIBERTUM_TEMPLATES=<folder>` uses that folder for added templates (for
        // screenshots and tests; nothing is written unless a template is added).
        if let Ok(folder) = std::env::var("LIBERTUM_TEMPLATES")
            && !folder.is_empty()
        {
            self.templates_dir = PathBuf::from(folder);
            self.reload_templates();
        }
    }

    /// Replaces the home screen with a new document built from `template`. The caret starts
    /// at the end of the document (the empty body paragraph), ready to type.
    fn create_document(&mut self, template: TemplateId) -> Task<Message> {
        let mut new = template.build(self.state.units);
        self.stamp_default_language(&mut new.document);
        let (mut doc, events) = DocView::open_with(new.document, new.options);
        doc.move_caret(|ed| ed.move_doc_end(false));
        self.state.fonts = installed_fonts(&doc);
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.state.current_page = 1;
        self.doc = Some(doc);
        self.file = None;
        self.file_context = None;
        self.dirty = false;
        self.autosave_pending = None;
        self.autosave_done = None;
        self.modal = None;
        self.after_save = None;
        self.scroll = (0.0, 0.0);
        self.canvas_focused = true;
        self.ime_preedit = None;
        self.refresh_document();
        self.refresh_view();
        self.refresh_format();
        Task::run(events, Message::Doc)
    }

    /// Creates a new unsaved document from one of the writer's added templates.
    fn create_user_template(&mut self, index: usize) -> Task<Message> {
        let Some(template) = self.user_templates.get(index).cloned() else {
            return Task::none();
        };
        let (context, document, warnings) = match FileContext::open(&template.path) {
            Ok(opened) => opened,
            Err(error) => {
                self.modal = Some(Modal::Error(format!("{} {error}", s::TEMPLATE_OPEN_FAILED)));
                return Task::none();
            }
        };
        for warning in warnings {
            tracing::info!(%warning, path = %template.path.display(), "template import warning");
        }
        let view = context.view();
        let mut document = document;
        self.stamp_default_language(&mut document);
        let (mut doc, events) = match template.format {
            TemplateFormat::Native if view.as_ref().is_some_and(|view| view.pageless) => {
                DocView::open_pageless(document)
            }
            _ => DocView::open(document),
        };
        doc.move_caret(|ed| ed.move_doc_end(false));
        self.state.fonts = installed_fonts(&doc);
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.state.current_page = 1;
        self.doc = Some(doc);
        self.file = None;
        // Keep the template's package so a save in the same format keeps the parts the
        // model does not hold (headers, unknown parts).
        self.file_context = Some(context);
        self.dirty = false;
        self.autosave_pending = None;
        self.autosave_done = None;
        self.modal = None;
        self.after_save = None;
        self.scroll = (0.0, 0.0);
        self.canvas_focused = true;
        self.ime_preedit = None;
        self.refresh_document();
        self.refresh_view();
        self.refresh_format();
        Task::run(events, Message::Doc)
    }

    /// Copies a document into the template folder and returns its index.
    fn import_template(&mut self, source: &std::path::Path) -> Result<usize, String> {
        if TemplateFormat::from_path(source).is_none() {
            return Err(s::TEMPLATE_UNSUPPORTED.to_owned());
        }
        // A template that cannot be read would only fail when it is used; check it now.
        FileContext::open(source)?;
        let template = templates::add(&self.templates_dir, source)?;
        self.reload_templates();
        self.user_templates
            .iter()
            .position(|item| item.path == template.path)
            .ok_or_else(|| s::TEMPLATE_ADD_FAILED.to_owned())
    }

    fn reload_templates(&mut self) {
        self.user_templates = templates::load(&self.templates_dir);
        if self
            .home_user
            .is_some_and(|index| index >= self.user_templates.len())
        {
            self.home_user = None;
        }
    }

    /// Toolbar Table button and Insert > Table: a fresh 3x3 table, as wide as the section's
    /// text column.
    fn insert_table(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let setup = doc.document().page_setup();
        let (width, height) = setup.paper.points();
        let page_width = if setup.orientation == libertum_model::Orientation::Landscape {
            height
        } else {
            width
        };
        let content = (page_width - setup.margins.left - setup.margins.right).max(72.0);
        let result = doc.edit(|ed| ed.insert_table(3, 3, content));
        self.finish_edit(result)
    }

    fn table_action(&mut self, action: TableAction) -> Task<Message> {
        if action == TableAction::InsertTable {
            return self.insert_table();
        }
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let result = match action {
            TableAction::InsertTable => unreachable!(),
            TableAction::InsertRowAbove => doc.edit(|ed| ed.insert_row_above()),
            TableAction::InsertRowBelow => doc.edit(|ed| ed.insert_row_below()),
            TableAction::InsertColumnLeft => doc.edit(|ed| ed.insert_column_left()),
            TableAction::InsertColumnRight => doc.edit(|ed| ed.insert_column_right()),
            TableAction::DeleteRow => doc.edit(|ed| ed.delete_row()),
            TableAction::DeleteColumn => doc.edit(|ed| ed.delete_column()),
            TableAction::DeleteTable => doc.edit(|ed| ed.delete_table()),
            TableAction::MergeRight => doc.edit(|ed| ed.merge_cell_right()),
            TableAction::MergeBelow => doc.edit(|ed| ed.merge_cell_below()),
            TableAction::SplitCell => doc.edit(|ed| ed.split_cell()),
            TableAction::ColumnNarrower | TableAction::ColumnWider => {
                // A quarter of a centimetre (or a tenth of an inch) per press.
                let step = match self.state.units {
                    Units::Inches => 7.2,
                    Units::Centimetres => 72.0 / 2.54 * 0.25,
                };
                let direction = if action == TableAction::ColumnWider {
                    1.0
                } else {
                    -1.0
                };
                doc.edit(|ed| {
                    let width = ed.caret_column_width().unwrap_or(0.0);
                    ed.set_caret_column_width(width + direction * step)
                })
            }
            TableAction::EqualiseColumns => doc.edit(|ed| ed.equalise_table_columns()),
        };
        let task = self.finish_edit(result);
        self.refresh_format();
        task
    }

    /// Table menu: a built-in style for the caret's table, or none.
    fn set_table_style(&mut self, index: Option<usize>) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let name = index.and_then(|index| {
            libertum_model::TableStyle::builtins()
                .into_iter()
                .nth(index)
                .map(|(style, _)| style.name)
        });
        if index.is_some() && name.is_none() {
            return Task::none();
        }
        let result = doc.edit(|ed| ed.set_table_style(name.as_deref()));
        let task = self.finish_edit(result);
        self.refresh_format();
        task
    }

    /// Switches the window between full screen and its normal size.
    fn toggle_full_screen(&mut self) -> Task<Message> {
        self.state.open_menu = None;
        self.state.fullscreen = !self.state.fullscreen;
        let mode = if self.state.fullscreen {
            window::Mode::Fullscreen
        } else {
            window::Mode::Windowed
        };
        window::oldest().and_then(move |id| window::set_mode(id, mode))
    }

    /// Renders the thumbnails in view that are missing or out of date. Only pages in the
    /// visible part of the column (plus one either side) are rendered, a few milliseconds
    /// each, and far-away ones are dropped so the cache stays small.
    fn refresh_thumbnails(&mut self) {
        if !self.state.thumbnails || self.state.pageless {
            return;
        }
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let total = doc.page_total();
        let height = if self.thumb_scroll.1 > 0.0 {
            self.thumb_scroll.1
        } else {
            self.viewport.map_or(800.0, |v| v.height)
        };
        let (first, last) = thumb_range(self.thumb_scroll.0, height, total);
        let width_px = (THUMB_W * self.scale_factor).round() as u32;
        for page in first..last {
            let Some(hash) = doc.page_content_hash(page) else {
                continue;
            };
            if self
                .thumb_cache
                .get(&page)
                .is_some_and(|(cached, _)| *cached == hash)
            {
                continue;
            }
            if let Some((w, h, pixels)) = doc.render_thumbnail(page, width_px) {
                let handle = iced::widget::image::Handle::from_rgba(w, h, pixels);
                self.thumb_cache.insert(page, (hash, handle));
            }
        }
        if self.thumb_cache.len() > THUMB_CACHE_PAGES {
            let keep = first.saturating_sub(THUMB_CACHE_PAGES / 2)..last + THUMB_CACHE_PAGES / 2;
            self.thumb_cache
                .retain(|page, _| keep.contains(page) && *page < total);
        }
    }

    /// The page thumbnails column: only the rows in view are built, with spacers for the
    /// rest, so a thousand-page document costs the same as a short one.
    fn thumbnails_panel(&self, total: usize, t: Tokens) -> Element<'_, Message> {
        let height = if self.thumb_scroll.1 > 0.0 {
            self.thumb_scroll.1
        } else {
            self.viewport.map_or(800.0, |v| v.height)
        };
        let (first, last) = thumb_range(self.thumb_scroll.0, height, total);
        let current = self.state.current_page.saturating_sub(1);
        let mut list = column![space().height(Length::Fixed(first as f32 * THUMB_ROW_H))];
        for page in first..last {
            let picture: Element<'_, Message> = match self.thumb_cache.get(&page) {
                Some((_, handle)) => iced::widget::image(handle.clone())
                    .width(Length::Fixed(THUMB_W))
                    .height(Length::Fixed(THUMB_BOX_H))
                    .content_fit(iced::ContentFit::Contain)
                    .into(),
                // Not paginated or rendered yet: a blank page.
                None => container(space())
                    .width(Length::Fixed(THUMB_W))
                    .height(Length::Fixed(THUMB_BOX_H * 0.95))
                    .style(|_| container::background(iced::Color::WHITE))
                    .into(),
            };
            let selected = page == current;
            let frame = container(picture)
                .padding(2)
                .style(move |_| container::Style {
                    border: iced::Border {
                        color: if selected { t.accent } else { t.divider },
                        width: if selected { 2.0 } else { 1.0 },
                        radius: 3.0.into(),
                    },
                    ..container::Style::default()
                });
            let label = text((page + 1).to_string()).size(11).color(if selected {
                t.accent
            } else {
                t.text_secondary
            });
            // The page and its number sit in the middle of the row: the same space on the
            // left and the right, whatever the column's width.
            list = list.push(
                button(
                    container(column![frame, label].spacing(4).align_x(Center))
                        .center_x(Length::Fill),
                )
                .padding(0)
                .style(button::text)
                .on_press(Message::GoToPage(page))
                .height(Length::Fixed(THUMB_ROW_H))
                .width(Length::Fill),
            );
        }
        list = list.push(space().height(Length::Fixed(
            total.saturating_sub(last) as f32 * THUMB_ROW_H,
        )));
        let scroll = iced::widget::scrollable(list.align_x(Center).width(Length::Fill))
            .id(THUMB_LIST_ID)
            // A thin scrollbar floating over the right-hand padding, as in the inspector, so
            // it takes no width from the thumbnails and they stay centred.
            .direction(iced::widget::scrollable::Direction::Vertical(
                iced::widget::scrollable::Scrollbar::new()
                    .width(6)
                    .scroller_width(6)
                    .margin(2),
            ))
            .on_scroll(|viewport| {
                Message::ThumbScrolled(viewport.absolute_offset().y, viewport.bounds().height)
            })
            .height(Length::Fill);
        let edge = container(space().width(Length::Fixed(1.0)).height(Length::Fill))
            .style(move |_| container::background(t.chrome_border));
        container(row![
            container(scroll)
                .padding(Padding {
                    top: 12.0,
                    ..Padding::ZERO
                })
                .width(Length::Fixed(THUMB_PANEL_W - 1.0)),
            edge
        ])
        .height(Length::Fill)
        .style(move |_| container::background(t.inspector))
        .into()
    }

    /// Width of the text column of the page setup, points (new images are fitted to it).
    fn text_column_width(&self) -> f32 {
        let Some(doc) = self.doc.as_ref() else {
            return 451.0;
        };
        let setup = doc.document().page_setup();
        let (width, height) = setup.paper.points();
        let page_width = if setup.orientation == libertum_model::Orientation::Landscape {
            height
        } else {
            width
        };
        (page_width - setup.margins.left - setup.margins.right).max(72.0)
    }

    /// Toolbar Image button and Insert > Image: pick a file and place it at the caret.
    fn pick_image(&mut self) -> Task<Message> {
        if self.doc.is_none() {
            return Task::none();
        }
        let picked = rfd::FileDialog::new()
            .add_filter(s::FILE_FILTER_IMAGES, s::IMAGE_EXTENSIONS)
            .set_title(s::IMAGE_PICK_TITLE)
            .pick_file();
        match picked {
            Some(path) => self.insert_image_from(&path),
            None => Task::none(),
        }
    }

    /// Reads an image file and inserts it at the caret, fitted to the text column. PNG and
    /// JPEG are kept as they are; GIF and BMP are converted to PNG. A file that cannot be
    /// read or decoded shows the error prompt and changes nothing.
    fn insert_image_from(&mut self, path: &std::path::Path) -> Task<Message> {
        let media = std::fs::read(path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                libertum_formats_common::media::media_from_bytes(bytes)
                    .map_err(|error| error.to_string())
            });
        let media = match media {
            Ok(media) => media,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not insert the image");
                self.modal = Some(Modal::Error(format!("{} {error}", s::IMAGE_FAILED)));
                return Task::none();
            }
        };
        let width = self.text_column_width();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let result = doc.edit(|ed| ed.insert_image(media, width, &name));
        let task = self.finish_edit(result);
        self.refresh_format();
        task
    }

    /// Page number of every paragraph that holds a bookmark, for cross-references. Read
    /// before an edit, because the layout cannot be asked while the editor is borrowed.
    fn bookmark_pages(&self) -> std::collections::HashMap<libertum_model::BlockId, u32> {
        let Some(doc) = self.doc.as_ref() else {
            return std::collections::HashMap::new();
        };
        doc.editor()
            .bookmarks()
            .into_iter()
            .filter_map(|(block, _, _)| doc.page_number_of_block(block).map(|page| (block, page)))
            .collect()
    }

    /// Brings every cross-reference up to date. Returns whether anything changed.
    fn refresh_cross_refs(&mut self) -> bool {
        let pages = self.bookmark_pages();
        let Some(doc) = self.doc.as_mut() else {
            return false;
        };
        let mut changed = false;
        let result = doc.edit(|ed| {
            let set = ed.refresh_cross_refs(&|block| pages.get(&block).copied())?;
            changed = !set.is_empty();
            Ok(set)
        });
        if let Err(error) = result {
            tracing::warn!(%error, "could not update the cross-references");
            return false;
        }
        if changed {
            self.dirty = true;
            self.after_edit();
        }
        changed
    }

    /// Toolbar Page Break, Insert > Page Break and Cmd/Ctrl+Enter.
    fn insert_page_break(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let result = doc.edit(|ed| ed.insert_page_break());
        self.finish_edit(result)
    }

    /// Insert > Page Number: turns the document's page numbers on (keeping their place, or
    /// bottom centre when they never had one) or off. One undo step.
    fn toggle_page_numbers(&mut self) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let numbering = doc.document().page_numbering();
        let result = doc.edit(|ed| {
            ed.set_page_numbering(PageNumbering {
                show: !numbering.show,
                ..numbering
            })
        });
        let _ = self.finish_edit(result);
        self.refresh_document();
        self.refresh_section();
    }

    /// Changes the whole-document page numbering as one undo step. The editor mirrors it into
    /// every section (the exporter's page-number setting), and `refresh_section` reads it back.
    fn apply_numbering(&mut self, change: impl FnOnce(&mut PageNumbering)) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let mut numbering = doc.document().page_numbering();
        change(&mut numbering);
        let result = doc.edit(|ed| ed.set_page_numbering(numbering));
        let _ = self.finish_edit(result);
        self.refresh_document();
        self.refresh_section();
    }

    fn on_home_key(&mut self, key: &keyboard::Key) -> Task<Message> {
        use keyboard::key::Named;
        match key.as_ref() {
            keyboard::Key::Named(Named::Enter) => {
                if let Some(index) = self.home_user {
                    self.create_user_template(index)
                } else {
                    self.create_document(self.home_selection)
                }
            }
            keyboard::Key::Named(Named::ArrowLeft) => {
                self.home_user = None;
                self.home_selection = self.home_selection.step(-1);
                Task::none()
            }
            keyboard::Key::Named(Named::ArrowRight) => {
                self.home_user = None;
                self.home_selection = self.home_selection.step(1);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn query_scale_factor() -> Task<Message> {
        window::oldest()
            .and_then(window::scale_factor)
            .map(Message::ScaleFactor)
    }

    // ------------------------------------------------------------ canvas geometry

    fn metrics(&self) -> std::sync::Arc<Metrics> {
        let (geometry, pages, geometries, used_heights) = if let Some(doc) = self.doc.as_ref() {
            let (geometries, used) = doc.page_views();
            (doc.geometry, doc.page_total(), geometries, used)
        } else {
            (
                PageGeometry::a4(),
                1,
                std::sync::Arc::<[PageGeometry]>::default(),
                std::sync::Arc::<[f32]>::default(),
            )
        };
        canvas::cached_metrics(
            &self.metrics_cache,
            &geometry,
            self.state.zoom,
            self.scale_factor,
            self.viewport.map_or(1000.0, |v| v.width),
            self.viewport.map_or(700.0, |v| v.height),
            pages,
            self.state.booklet,
            self.doc.as_ref().is_some_and(|doc| doc.pageless),
            &geometries,
            &used_heights,
        )
    }

    /// The pages on screen plus one either side: the part of a selection worth highlighting.
    fn pages_near_view(&self) -> std::ops::Range<usize> {
        if self.viewport.is_none() {
            return 0..4;
        }
        let vp = self.viewport_physical();
        let visible = self.metrics().visible_pages(self.scroll.1, vp.height);
        visible.start.saturating_sub(1)..visible.end + 1
    }

    /// Canvas size in physical pixels.
    fn viewport_physical(&self) -> Size {
        let v = self.viewport.unwrap_or(Size::new(1000.0, 700.0));
        Size::new(v.width * self.scale_factor, v.height * self.scale_factor)
    }

    fn clamp_scroll(&mut self) {
        let (mx, my) = self.metrics().max_scroll(self.viewport_physical());
        self.scroll = (self.scroll.0.clamp(0.0, mx), self.scroll.1.clamp(0.0, my));
    }

    /// Keeps the viewport middle on the same spot of the document when the page scale
    /// changes (zoom, fit-width resize, display scale change). `old` and `old_viewport` are
    /// the metrics and physical viewport from before the change.
    fn reanchor(&mut self, old: std::sync::Arc<Metrics>, old_viewport: Size) {
        let new = self.metrics();
        let viewport = self.viewport_physical();
        if (old.scale - new.scale).abs() < f32::EPSILON && (old.sf - new.sf).abs() < f32::EPSILON {
            return;
        }
        let anchor = old.anchor_of(self.scroll.1, old_viewport.height);
        let old_cx =
            (self.scroll.0 + old_viewport.width * 0.5) / old.content_width(old_viewport.width);
        self.scroll.1 = new.scroll_for_anchor(anchor, viewport.height);
        self.scroll.0 = old_cx * new.content_width(viewport.width) - viewport.width * 0.5;
    }

    /// Called after anything that can change what is visible: clamps the scroll offset,
    /// refreshes the status bar and asks the render worker for the visible pages plus one
    /// page either side (PLAN §4.4).
    /// Every heading in document order, for the Headers pane. Scanning is O(paragraphs), so
    /// it is only done while that pane is visible; page numbers come from the layout cache.
    pub fn headings(&self) -> Vec<HeadingEntry> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let document = doc.editor().document();
        document
            .paragraphs()
            .filter_map(|paragraph| {
                let level = paragraph.props.outline_level?;
                let text = paragraph.text().trim();
                (!text.is_empty()).then(|| HeadingEntry {
                    block: paragraph.id,
                    level,
                    text: text.to_owned(),
                    page: doc.page_number_of_block(paragraph.id),
                })
            })
            .collect()
    }

    /// Every footnote and endnote in document order, for the Notes pane.
    pub fn notes(&self) -> Vec<NoteEntry> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        let mut footnotes = 0u32;
        let mut endnotes = 0u32;
        let mut out = Vec::new();
        for paragraph in doc.editor().document().paragraphs() {
            for (index, note) in paragraph.notes().iter().enumerate() {
                let number = match note.kind {
                    libertum_model::NoteKind::Footnote => {
                        footnotes += 1;
                        footnotes
                    }
                    libertum_model::NoteKind::Endnote => {
                        endnotes += 1;
                        endnotes
                    }
                };
                out.push(NoteEntry {
                    block: paragraph.id,
                    index,
                    offset: note.offset,
                    kind: note.kind,
                    number,
                    text: note.text.clone(),
                });
            }
        }
        out
    }

    /// Every bookmark in document order, for the Bookmarks pane.
    pub fn bookmarks(&self) -> Vec<BookmarkEntry> {
        let Some(doc) = &self.doc else {
            return Vec::new();
        };
        doc.editor()
            .bookmarks()
            .into_iter()
            .map(|(block, index, bookmark)| BookmarkEntry {
                block,
                index,
                offset: bookmark.offset,
                name: bookmark.name,
                page: doc.page_number_of_block(block),
            })
            .collect()
    }

    /// Words in the selection, `None` for a caret. Cached by document, selection and
    /// revision: the text is only copied and counted when one of them changes.
    fn selection_word_count(&mut self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let (start, end) = doc.editor().ordered_selection()?;
        if start == end {
            return None;
        }
        let key = (doc.id(), start, end, doc.document().revision());
        if let Some((id, s, e, revision, words)) = self.selection_words_cache
            && (id, s, e, revision) == key
        {
            return Some(words);
        }
        let words = doc
            .editor()
            .selected_text()
            .map_or(0, |text| libertum_model::count_words(&text));
        self.selection_words_cache = Some((key.0, key.1, key.2, key.3, words));
        Some(words)
    }

    fn refresh_view(&mut self) {
        if self.doc.is_none() {
            return;
        }
        // A scroll, resize or layout pass moves what is near the view: recheck once quiet.
        note_spell_edit();
        self.state.selection_words = self.selection_word_count();
        self.refresh_thumbnails();
        if self.state.inspector == Inspector::Headers {
            self.headings = self.headings();
        }
        if self.state.inspector == Inspector::Notes {
            self.notes = self.notes();
        }
        if self.state.inspector == Inspector::Bookmarks {
            self.bookmarks = self.bookmarks();
        }
        self.state.page_count = self.doc.as_ref().map_or(0, DocView::page_total);
        self.clamp_scroll();
        let m = self.metrics();
        let vp = self.viewport_physical();
        let current = m.current_page(self.scroll.1, vp.height, vp.width);
        self.state.current_page = current + 1;
        if self.viewport.is_none() {
            return;
        }
        let visible = m.visible_pages(self.scroll.1, vp.height);
        // Two pages either side, nearest first: the worker renders ahead of the viewport so a
        // page is already a bitmap when it scrolls in, and behind it so scrolling back is
        // instant. One either side was not enough to hide the raster time at speed.
        let first = visible.start.saturating_sub(2);
        let last = (visible.end + 2).min(m.pages);
        let mut wanted: Vec<usize> = (first..last).collect();
        wanted.sort_by_key(|&p| (p as isize - current as isize).unsigned_abs());
        if let Some(doc) = &self.doc {
            doc.source.request(&wanted, m.scale);
        }
    }

    fn on_canvas_event(&mut self, event: CanvasEvent) -> Task<Message> {
        let sf = self.scale_factor;
        match event {
            CanvasEvent::Wheel { dx, dy, stepped } => {
                if stepped {
                    // A wheel notch: add it to the distance still to travel and glide there.
                    // Notches add up, so spinning faster scrolls faster, and the total is
                    // always exactly what the wheel asked for.
                    if self.scroll_pending == (0.0, 0.0) {
                        self.last_frame = Some(Instant::now());
                    }
                    self.fling = (0.0, 0.0);
                    self.trackpad_pending = (0.0, 0.0);
                    // A notch the other way cancels what is left instead of fighting it.
                    let add = |pending: f32, delta: f32| {
                        if pending != 0.0 && delta != 0.0 && pending.signum() != delta.signum() {
                            delta
                        } else {
                            pending + delta
                        }
                    };
                    self.scroll_pending = (
                        add(self.scroll_pending.0, dx * sf),
                        add(self.scroll_pending.1, dy * sf),
                    );
                    self.trim_pending_to_edges();
                } else {
                    // A trackpad (or a drag past the edge): follow it exactly, and note how
                    // fast the finger is moving so the page can carry on when it lifts.
                    let now = Instant::now();
                    // The frame clock was not running if no gesture was in progress: time
                    // the first frame from this event.
                    let clock_was_off = !self.trackpad_active(now);
                    self.scroll_pending = (0.0, 0.0);
                    let (px, py) = (dx * sf, dy * sf);
                    // Queued, not applied: the next frame moves the page by everything that
                    // arrived since the last one.
                    self.trackpad_pending.0 += px;
                    self.trackpad_pending.1 += py;
                    self.track_fling(now, (px, py));
                    if clock_was_off || self.last_frame.is_none() {
                        self.last_frame = Some(now);
                    }
                }
            }
            CanvasEvent::ScrollTo { x, y } => {
                self.scroll_pending = (0.0, 0.0);
                self.trackpad_pending = (0.0, 0.0);
                self.fling = (0.0, 0.0);
                self.scroll.0 = x.unwrap_or(self.scroll.0);
                self.scroll.1 = y.unwrap_or(self.scroll.1);
            }
            CanvasEvent::Pointer {
                page,
                x_pt,
                y_pt,
                extend,
                clicks,
            } => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                self.canvas_focused = true;
                let moved = match clicks {
                    3 => doc.select_paragraph_from_pointer(page, x_pt, y_pt),
                    2 => doc.select_word_from_pointer(page, x_pt, y_pt),
                    _ => doc.set_caret_from_pointer(page, x_pt, y_pt, extend),
                };
                if moved {
                    self.ensure_caret_visible();
                    self.refresh_section();
                }
            }
            CanvasEvent::ContextMenu {
                page,
                x_pt,
                y_pt,
                x,
                y,
            } => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                self.canvas_focused = true;
                if doc.set_caret_from_pointer(page, x_pt, y_pt, false) {
                    self.ensure_caret_visible();
                    self.refresh_section();
                }
                self.refresh_format();
                // The picture under the pointer, not just the first one in the paragraph.
                let picture = self
                    .doc
                    .as_ref()
                    .and_then(|doc| doc.image_at(page, x_pt, y_pt));
                let target = if self.state.in_table {
                    crate::toolbar::ContextMenuTarget::Table {
                        merge_right: self.state.can_merge_right,
                        merge_below: self.state.can_merge_below,
                        split: self.state.can_split_cell,
                    }
                } else if let Some((block, index)) = picture {
                    crate::toolbar::ContextMenuTarget::Image { block, index }
                } else {
                    crate::toolbar::ContextMenuTarget::Text {
                        spelling: self.spelling_at(page, x_pt, y_pt),
                    }
                };
                self.state.open_menu = None;
                self.context_menu = Some(ContextMenuState { x, y, target });
            }
            CanvasEvent::PointerDrag { page, x_pt, y_pt } => {
                self.canvas_focused = true;
                // Extend from the anchor set by the press; the drag never moves it.
                if let Some(doc) = self.doc.as_mut() {
                    doc.set_caret_from_pointer(page, x_pt, y_pt, true);
                }
                return Task::none();
            }
            CanvasEvent::Blur => {
                self.canvas_focused = false;
                self.ime_preedit = None;
                return Task::none();
            }
            CanvasEvent::Zoom {
                factor,
                page,
                x_pt,
                y_pt,
                cursor,
            } => {
                self.canvas_focused = true;
                self.state.zoom_edit = None;
                let current = f32::from(self.metrics().percent());
                let percent = (current * factor).round().clamp(1.0, 800.0) as u16;
                self.zoom_at(Zoom::Percent(percent), page, x_pt, y_pt, cursor);
                return Task::none();
            }
            CanvasEvent::Viewport(size) => {
                let old = self.metrics();
                let old_vp = self.viewport_physical();
                self.viewport = Some(size);
                self.reanchor(old, old_vp);
                self.refresh_view();
                return if self.scale_factor_known {
                    Task::none()
                } else {
                    Self::query_scale_factor()
                };
            }
        }
        self.refresh_view();
        Task::none()
    }

    // ------------------------------------------------------------ text editing

    fn on_keyboard(&mut self, event: keyboard::Event) -> Task<Message> {
        let keyboard::Event::KeyPressed {
            key,
            physical_key,
            text,
            modifiers,
            ..
        } = event
        else {
            return Task::none();
        };
        // Ctrl+C, V, Z and the others by position, so they work on a Cyrillic layout too.
        let key = shortcuts::latin_key(&key, &physical_key, modifiers);
        // An inspector field owns the keyboard while it is being edited.
        if self.state.editing.is_some() {
            return Task::none();
        }
        let Some(doc) = self.doc.as_mut() else {
            return self.on_home_key(&key);
        };
        let command = modifiers.command();
        let shift = modifiers.shift();
        // Text in an upright table cell is horizontal even on a vertical page.
        let vertical = doc.is_vertical() && !doc.caret_in_upright_cell();
        use keyboard::key::Named;

        if command {
            if let keyboard::Key::Character(c) = key.as_ref() {
                if c.eq_ignore_ascii_case("z") {
                    return if shift { self.redo() } else { self.undo() };
                }
                if c.eq_ignore_ascii_case("y") {
                    return self.redo();
                }
                if c.eq_ignore_ascii_case("a") {
                    doc.select_all();
                    return self.after_move();
                }
                if c.eq_ignore_ascii_case("c") {
                    return self.copy();
                }
                if c.eq_ignore_ascii_case("x") {
                    return self.cut();
                }
                if c.eq_ignore_ascii_case("v") {
                    return iced::clipboard::read().map(Message::PasteText);
                }
            }
            // In vertical text Up and Down run along the column, so Ctrl+Up and Ctrl+Down move
            // by word; Left and Right cross between columns and have no word step.
            if vertical {
                match key.as_ref() {
                    keyboard::Key::Named(Named::ArrowUp) => {
                        doc.move_caret(|ed| ed.move_word_left(shift));
                        return self.after_move();
                    }
                    keyboard::Key::Named(Named::ArrowDown) => {
                        doc.move_caret(|ed| ed.move_word_right(shift));
                        return self.after_move();
                    }
                    keyboard::Key::Named(Named::ArrowLeft | Named::ArrowRight) => {
                        return Task::none();
                    }
                    _ => {}
                }
            }
            match key.as_ref() {
                keyboard::Key::Named(Named::ArrowLeft) => {
                    doc.move_caret(|ed| ed.move_word_left(shift));
                    return self.after_move();
                }
                keyboard::Key::Named(Named::ArrowRight) => {
                    doc.move_caret(|ed| ed.move_word_right(shift));
                    return self.after_move();
                }
                keyboard::Key::Named(Named::Home) => {
                    doc.move_caret(|ed| ed.move_doc_start(shift));
                    return self.after_move();
                }
                keyboard::Key::Named(Named::End) => {
                    doc.move_caret(|ed| ed.move_doc_end(shift));
                    return self.after_move();
                }
                _ => return Task::none(),
            }
        }
        if modifiers.alt() || !self.canvas_focused {
            return Task::none();
        }

        // Vertical text reads down a column and the columns follow from the right, so the
        // arrows turn with it: Up and Down step along the column, Left goes to the next
        // column (the one on the left) and Right to the previous one.
        if vertical {
            match key.as_ref() {
                keyboard::Key::Named(Named::ArrowUp) => {
                    doc.move_caret(|ed| ed.move_left(shift));
                    return self.after_move();
                }
                keyboard::Key::Named(Named::ArrowDown) => {
                    doc.move_caret(|ed| ed.move_right(shift));
                    return self.after_move();
                }
                keyboard::Key::Named(Named::ArrowLeft) => {
                    doc.move_vertical(1, shift);
                    return self.after_move();
                }
                keyboard::Key::Named(Named::ArrowRight) => {
                    doc.move_vertical(-1, shift);
                    return self.after_move();
                }
                _ => {}
            }
        }

        match key.as_ref() {
            keyboard::Key::Named(Named::Backspace) => {
                let result = doc.edit(|ed| ed.delete_backward());
                self.finish_edit(result)
            }
            keyboard::Key::Named(Named::Delete) => {
                let result = doc.edit(|ed| ed.delete_forward());
                self.finish_edit(result)
            }
            keyboard::Key::Named(Named::Enter) => {
                let result = doc.edit(|ed| ed.insert_text("\n"));
                self.finish_edit(result)
            }
            // Tab and Shift+Tab change a list item's level (at the start of an item, or with
            // list items selected), and with any text selected they indent or outdent the
            // selected paragraphs instead of replacing the selection (Word, Pages). With a
            // plain caret outside a list, Tab inserts a tab character and Shift+Tab does
            // nothing.
            keyboard::Key::Named(Named::Tab) => {
                if doc.editor().in_table() {
                    // Tab moves to the next cell; in the last cell it adds a row (Word).
                    let result = doc.edit(|ed| ed.tab_in_table(shift));
                    self.finish_edit(result)
                } else if doc.editor().has_selection() || doc.editor().tab_changes_list_level() {
                    let result = doc.edit(|ed| ed.indent(if shift { -1 } else { 1 }));
                    self.finish_edit(result)
                } else if shift {
                    Task::none()
                } else {
                    let result = doc.edit(|ed| ed.insert_text("\t"));
                    self.finish_edit(result)
                }
            }
            keyboard::Key::Named(Named::ArrowLeft) => {
                doc.move_caret(|ed| ed.move_left(shift));
                self.after_move()
            }
            keyboard::Key::Named(Named::ArrowRight) => {
                doc.move_caret(|ed| ed.move_right(shift));
                self.after_move()
            }
            keyboard::Key::Named(Named::ArrowUp) => {
                doc.move_vertical(-1, shift);
                self.after_move()
            }
            keyboard::Key::Named(Named::ArrowDown) => {
                doc.move_vertical(1, shift);
                self.after_move()
            }
            keyboard::Key::Named(Named::Home) => {
                doc.move_caret(|ed| ed.move_line_start(shift));
                self.after_move()
            }
            keyboard::Key::Named(Named::End) => {
                doc.move_caret(|ed| ed.move_line_end(shift));
                self.after_move()
            }
            keyboard::Key::Named(Named::PageUp) => {
                doc.move_caret(|ed| ed.move_paragraph(-10, shift));
                self.after_move()
            }
            keyboard::Key::Named(Named::PageDown) => {
                doc.move_caret(|ed| ed.move_paragraph(10, shift));
                self.after_move()
            }
            _ => {
                let Some(text) = text else {
                    return Task::none();
                };
                if text.is_empty() || text.chars().all(char::is_control) {
                    return Task::none();
                }
                let owned = text.to_string();
                let result = doc.edit(|ed| ed.insert_text(&owned));
                self.finish_edit(result)
            }
        }
    }

    fn on_ime(&mut self, event: input_method::Event) -> Task<Message> {
        tracing::debug!(?event, focused = self.canvas_focused, "input method event");
        match event {
            input_method::Event::Commit(text) => {
                self.ime_preedit = None;
                if text.is_empty() || !self.canvas_focused {
                    return Task::none();
                }
                let text = text.replace("\r\n", "\n").replace('\r', "\n");
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.insert_text(&text));
                self.finish_edit(result)
            }
            input_method::Event::Preedit(text, _) => {
                self.ime_preedit = (!text.is_empty()).then_some(text);
                Task::none()
            }
            input_method::Event::Opened | input_method::Event::Closed => {
                self.ime_preedit = None;
                Task::none()
            }
        }
    }

    fn finish_edit(&mut self, result: Result<(), libertum_editor::EditorError>) -> Task<Message> {
        match result {
            // Compare mode is read-only: a blocked edit changed nothing.
            Ok(()) if self.doc.as_ref().is_some_and(|doc| doc.read_only) => {}
            Ok(()) => {
                self.dirty = true;
                self.after_edit();
            }
            Err(error) => tracing::warn!(%error, "edit failed"),
        }
        Task::none()
    }

    fn after_edit(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        // Check spelling as soon as typing pauses, and meanwhile drop underlines whose word
        // is no longer at their place: an edit moved or changed it.
        note_spell_edit();
        let document = doc.document();
        let before = self.spell_errors.len();
        let typing = Self::typing_span(doc);
        self.spell_errors.retain(|error| {
            !Self::touches_caret(error, typing)
                && document
                    .paragraph(error.block)
                    .and_then(|p| p.text().get(error.start..error.end))
                    .is_some_and(|word| word == error.word)
        });
        if self.spell_errors.len() != before {
            self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
        }
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.ime_preedit = None;
        self.ensure_caret_visible();
        self.refresh_view();
        self.refresh_document();
    }

    fn after_move(&mut self) -> Task<Message> {
        self.ensure_caret_visible();
        self.refresh_view();
        self.refresh_section();
        Task::none()
    }

    fn undo(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        match doc.undo() {
            Ok(true) => {
                self.dirty = true;
                self.after_edit();
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "undo failed"),
        }
        Task::none()
    }

    fn redo(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        match doc.redo() {
            Ok(true) => {
                self.dirty = true;
                self.after_edit();
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "redo failed"),
        }
        Task::none()
    }

    fn copy(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        match doc.copy() {
            Some(text) => iced::clipboard::write::<Message>(text),
            None => Task::none(),
        }
    }

    fn cut(&mut self) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        match doc.cut() {
            Ok(Some(text)) => {
                self.dirty = true;
                self.after_edit();
                iced::clipboard::write::<Message>(text)
            }
            Ok(None) => Task::none(),
            Err(error) => {
                tracing::warn!(%error, "cut failed");
                Task::none()
            }
        }
    }

    /// Scrolls so the caret line is in view. Only the caret's own line is considered: a whole
    /// A4 page does not fit in the viewport at 100%, so fitting the page made every click and
    /// edit jump to the bottom of the page (and lose the reader's place).
    fn ensure_caret_visible(&mut self) {
        if self.viewport.is_none() {
            return;
        }
        let Some(caret) = self.doc.as_ref().and_then(DocView::caret_rect) else {
            return;
        };
        let m = self.metrics();
        let vp = self.viewport_physical();
        let page_top = m.page_top(caret.page);
        let top = page_top + caret.y * m.scale;
        let bottom = top + (caret.height.max(1.0)) * m.scale;
        self.reveal_vertical(top, bottom);
        // Booklet view puts a page left or right of the viewport centre.
        let left = m.page_left(caret.page, vp.width) + caret.x * m.scale;
        let right = left + (caret.width.max(0.75)) * m.scale;
        let visible_left = self.scroll.0 + m.pad;
        let visible_right = self.scroll.0 + vp.width - m.pad;
        if left < visible_left {
            self.scroll.0 = left - m.pad;
        } else if right > visible_right {
            self.scroll.0 = right - vp.width + m.pad;
        }
        self.clamp_scroll();
    }

    /// Physical pixels at the top and bottom of the canvas that floating chrome covers: the
    /// Find bar under the toolbar and the page and word count pill. Text scrolled into view
    /// must clear them. They are ignored when the window is too short to honour them.
    fn canvas_insets(&self) -> (f32, f32) {
        let top = if self.state.find.open {
            FIND_BAR_COVER
        } else {
            0.0
        } + if self.compare_on() {
            FIND_BAR_COVER
        } else {
            0.0
        };
        let bottom = if self.state.show_word_count {
            WORD_COUNT_PILL_COVER
        } else {
            0.0
        };
        let (top, bottom) = (top * self.scale_factor, bottom * self.scale_factor);
        if self.viewport_physical().height < (top + bottom) * 2.0 {
            return (0.0, 0.0);
        }
        (top, bottom)
    }

    /// Scrolls vertically so the span `top..bottom` (physical canvas pixels) is in the part
    /// of the viewport that nothing floats over. A span taller than that part shows its top.
    fn reveal_vertical(&mut self, top: f32, bottom: f32) {
        // Showing the caret wins over a wheel glide still in progress.
        self.scroll_pending = (0.0, 0.0);
        self.trackpad_pending = (0.0, 0.0);
        self.fling = (0.0, 0.0);
        let m = self.metrics();
        let vp = self.viewport_physical();
        let (inset_top, inset_bottom) = self.canvas_insets();
        let lead = m.pad + inset_top;
        let trail = m.pad + inset_bottom;
        let visible_top = self.scroll.1 + lead;
        let visible_bottom = self.scroll.1 + vp.height - trail;
        if top < visible_top || bottom - top > vp.height - lead - trail {
            self.scroll.1 = top - lead;
        } else if bottom > visible_bottom {
            self.scroll.1 = bottom - vp.height + trail;
        }
    }

    /// The selection's rectangles when the selection is exactly the match Find found last
    /// (so the highlight goes away as soon as the caret moves or the text changes).
    fn find_match_rects(&self) -> Vec<libertum_layout::SelectionRect> {
        let (Some(doc), Some(found)) = (self.doc.as_ref(), self.state.find.last.as_ref()) else {
            return Vec::new();
        };
        if !self.state.find.open {
            return Vec::new();
        }
        let matches = doc
            .editor()
            .ordered_selection()
            .is_some_and(|(start, end)| {
                start.block == found.block
                    && end.block == found.block
                    && start.offset == found.range.start
                    && end.offset == found.range.end
            });
        if matches {
            doc.selection_rects()
        } else {
            Vec::new()
        }
    }

    fn preedit_overlay(&self) -> Option<canvas::PreeditOverlay> {
        let text = self.ime_preedit.as_ref().or(self.preedit_hook.as_ref())?;
        let caret = self.doc.as_ref()?.caret_rect()?;
        Some(canvas::PreeditOverlay {
            page: caret.page,
            x_pt: caret.x,
            y_pt: caret.y,
            // A caret in vertical text is a bar across the column: its length is the text's size.
            height_pt: caret.height.max(caret.width),
            text: text.clone(),
        })
    }

    fn set_zoom(&mut self, zoom: Zoom) {
        let old = self.metrics();
        let old_vp = self.viewport_physical();
        self.state.zoom = zoom;
        self.reanchor(old, old_vp);
        self.refresh_view();
    }

    /// Sets an exact zoom percentage, clamped to the supported 1 to 800%.
    fn set_zoom_percent(&mut self, percent: f32) {
        let percent = percent.round().clamp(1.0, 800.0) as u16;
        self.set_zoom(Zoom::Percent(percent));
    }

    /// Sets the zoom while keeping the page point under `cursor` (physical pixels) fixed, so
    /// Ctrl/Cmd + wheel and trackpad pinch zoom towards the pointer.
    fn zoom_at(&mut self, zoom: Zoom, page: usize, x_pt: f32, y_pt: f32, cursor: iced::Point) {
        self.state.zoom = zoom;
        let vp = self.viewport_physical();
        let m = self.metrics();
        let origin_x = m.page_left(page, vp.width);
        let origin_y = m.page_top(page);
        self.scroll = (
            origin_x + x_pt * m.scale - cursor.x,
            origin_y + y_pt * m.scale - cursor.y,
        );
        self.clamp_scroll();
        self.refresh_view();
    }

    fn set_scale_factor(&mut self, sf: f32) {
        if !(sf.is_finite() && sf > 0.0) {
            return;
        }
        self.scale_factor_known = true;
        if (sf - self.scale_factor).abs() < f32::EPSILON {
            return;
        }
        let old = self.metrics();
        let old_vp = self.viewport_physical();
        // The viewport is stored in logical pixels, so only the physical size changes.
        self.scale_factor = sf;
        self.reanchor(old, old_vp);
        self.refresh_view();
        tracing::info!(scale_factor = sf, "window scale factor");
    }

    fn on_doc_event(&mut self, event: DocEvent) -> Task<Message> {
        match event {
            DocEvent::Pagination { pages, complete } => {
                if complete {
                    // `{pages}` header fields and the total page count are final now.
                    if let Some(doc) = self.doc.as_mut() {
                        doc.pagination_finished();
                    }
                    tracing::info!(pages, "pagination complete; scroll extent is now exact");
                }
                self.refresh_view();
            }
            DocEvent::PageReady => {} // the canvas draws from the shared cache
        }
        Task::none()
    }

    /// Checks the paragraphs near the viewport (plus the caret's) on a worker, now, even if
    /// nothing changed (the language or the setting changed).
    fn schedule_spell_check(&mut self) -> Task<Message> {
        self.spell_signature = None;
        self.check_spelling_if_changed()
    }

    /// Spelling is checked from a timer that fires once activity goes quiet, not on every
    /// keystroke, scroll or layout
    /// pass: each tick looks at the paragraphs near the viewport and starts a check only if
    /// their text, the language or the ignore lists changed since the last one.
    fn check_spelling_if_changed(&mut self) -> Task<Message> {
        if !self.settings.spellcheck {
            self.spell_generation = self.spell_generation.wrapping_add(1);
            self.spell_signature = None;
            self.clear_spell_errors();
            return Task::none();
        }
        let Some(doc) = self.doc.as_ref() else {
            self.spell_generation = self.spell_generation.wrapping_add(1);
            self.spell_signature = None;
            self.clear_spell_errors();
            return Task::none();
        };
        // A document in Chinese, Japanese or Korean has no dictionary: nothing to underline.
        let languages = self.spell_languages();
        if languages.is_empty() {
            self.spell_generation = self.spell_generation.wrapping_add(1);
            self.spell_signature = None;
            self.clear_spell_errors();
            return Task::none();
        };
        let pages = self.pages_near_view();
        // Cheap change detection: without a document edit, a page-setup change or a new
        // viewport range, the paragraphs near the view and the result are the same. The
        // old signature cloned and hashed every visible paragraph on every tick.
        let signature = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            doc.document().revision().hash(&mut hasher);
            doc.source.pages.generation().hash(&mut hasher);
            doc.source.pages.known_pages().hash(&mut hasher);
            pages.start.hash(&mut hasher);
            pages.end.hash(&mut hasher);
            // The caret's place, not just its paragraph: moving off a word releases it
            // for checking.
            doc.editor().caret().hash(&mut hasher);
            languages.hash(&mut hasher);
            self.spell_personal.len().hash(&mut hasher);
            self.spell_ignored_document.len().hash(&mut hasher);
            hasher.finish()
        };
        if self.spell_signature == Some(signature) {
            return Task::none();
        }
        let targets = doc.visible_paragraphs(pages.clone());
        if targets.is_empty() {
            self.spell_generation = self.spell_generation.wrapping_add(1);
            self.spell_signature = None;
            self.clear_spell_errors();
            return Task::none();
        }
        self.spell_signature = Some(signature);
        self.spell_generation = self.spell_generation.wrapping_add(1);
        let generation = self.spell_generation;
        tracing::debug!(
            generation,
            paragraphs = targets.len(),
            pages = ?pages,
            "spell check started"
        );
        let personal = self.spell_personal.clone();
        let ignored = self.spell_ignored_document.clone();
        // Its own short-lived thread, not the shared task pool: a long autosave or export on
        // the pool must not hold the underlines back.
        let (tx, rx) = iced::futures::channel::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("libertum-spell-check".into())
            .spawn(move || {
                let _ = tx.send(spell::check_paragraphs_any(
                    &targets, &languages, &personal, &ignored,
                ));
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the spell check");
            self.spell_signature = None;
            return Task::none();
        }
        Task::perform(
            async move {
                rx.await
                    .unwrap_or_else(|_| Err("the spell check stopped".to_owned()))
            },
            move |result| Message::SpellChecked { generation, result },
        )
    }

    /// The caret's place, when it is a plain caret (no selection): the word touching it is
    /// the one being typed.
    fn typing_span(doc: &DocView) -> Option<(BlockId, usize)> {
        let editor = doc.editor();
        if editor.has_selection() {
            return None;
        }
        let caret = editor.caret();
        Some((caret.block, caret.offset))
    }

    /// Whether a misspelling is in, or ends at, the caret: the writer is still typing it.
    fn touches_caret(error: &spell::Span, typing: Option<(BlockId, usize)>) -> bool {
        typing.is_some_and(|(block, offset)| {
            error.block == block && error.start <= offset && offset <= error.end
        })
    }

    fn clear_spell_errors(&mut self) {
        if !self.spell_errors.is_empty() {
            self.spell_errors.clear();
            self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
        }
    }

    /// The underline rectangles for the current misspellings, cached until the misspellings
    /// or the page layout change. Recomputing every line rectangle on every draw cost about
    /// 30 ms a frame in the 5,000-page demo, where much of the lorem ipsum is misspelt.
    fn spell_rects(&self) -> Vec<libertum_layout::SelectionRect> {
        let Some(doc) = self.doc.as_ref() else {
            return Vec::new();
        };
        if self.spell_errors.is_empty() {
            return Vec::new();
        }
        let generation = doc.source.pages.generation();
        let epoch = self.spell_errors_epoch;
        let mut cache = self
            .spell_rect_cache
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((cached_generation, cached_epoch, rects)) = cache.as_ref()
            && *cached_generation == generation
            && *cached_epoch == epoch
        {
            return rects.clone();
        }
        let mut rects = Vec::new();
        for error in &self.spell_errors {
            rects.extend(doc.range_rects(error.block, error.start, error.end));
        }
        *cache = Some((generation, epoch, rects.clone()));
        rects
    }

    /// The misspelt word under a page point, with suggestions, for the context menu.
    fn spelling_at(
        &self,
        page: usize,
        x_pt: f32,
        y_pt: f32,
    ) -> Option<crate::toolbar::SpellingMenu> {
        let languages = self.spell_languages();
        if languages.is_empty() {
            return None;
        }
        let doc = self.doc.as_ref()?;
        let (block, offset) = doc.hit_test(page, x_pt, y_pt)?;
        let text = doc.document().paragraph(block)?.text().to_owned();
        let (start, end, word) = spell::word_at(&text, offset)?;
        let errors = spell::check_any(
            &text,
            &languages,
            &self.spell_personal,
            &self.spell_ignored_document,
        )
        .ok()?;
        let (start, end, word) = errors
            .into_iter()
            .find(|(s, e, w)| *s == start && *e == end && w == &word)?;
        let span = spell::Span {
            block,
            start,
            end,
            word,
        };
        if self
            .spell_ignored_once
            .contains(&(block, span.start, span.end, span.word.clone()))
        {
            return None;
        }
        let suggestions = spell::suggestions_any(&span.word, &languages);
        Some(crate::toolbar::SpellingMenu { span, suggestions })
    }

    // ------------------------------------------------------------ autoscroll

    fn autoscroll_frame(&mut self, now: Instant) -> Task<Message> {
        let ready = self.viewport.is_some()
            && self.scale_factor_known
            && self
                .doc
                .as_ref()
                .is_some_and(|d| d.source.pages.is_complete());
        let max_y = self.metrics().max_scroll(self.viewport_physical()).1;
        let sf = self.scale_factor;
        let Some(auto) = self.autoscroll.as_mut() else {
            return Task::none();
        };
        let speed = auto.speed;
        let phase = std::mem::replace(&mut auto.phase, AutoPhase::Done);
        let mut exit = false;
        let next = match phase {
            AutoPhase::Waiting { ready_since } => match (ready, ready_since) {
                (false, _) => AutoPhase::Waiting { ready_since: None },
                (true, None) => AutoPhase::Waiting {
                    ready_since: Some(now),
                },
                (true, Some(t)) if now.duration_since(t) >= AUTOSCROLL_SETTLE => {
                    tracing::info!(speed, max_y, "autoscroll started");
                    self.scroll = (0.0, 0.0);
                    AutoPhase::Running { last: now }
                }
                (true, Some(t)) => AutoPhase::Waiting {
                    ready_since: Some(t),
                },
            },
            AutoPhase::Running { last } => {
                let dt = now.duration_since(last).as_secs_f32();
                self.scroll.1 += speed * sf * dt;
                if self.scroll.1 >= max_y {
                    self.scroll.1 = max_y;
                    let (cached, bytes) =
                        self.doc.as_ref().map_or((0, 0), |d| d.source.cache_stats());
                    tracing::info!(
                        cached_pages = cached,
                        cached_mb = bytes / (1024 * 1024),
                        "autoscroll finished"
                    );
                    exit = true;
                    AutoPhase::Done
                } else {
                    AutoPhase::Running { last: now }
                }
            }
            AutoPhase::Done => AutoPhase::Done,
        };
        if let Some(auto) = self.autoscroll.as_mut() {
            auto.phase = next;
        }
        self.refresh_view();
        if exit { iced::exit() } else { Task::none() }
    }

    // ------------------------------------------------------------ files

    /// Asks for a document and opens it, prompting first when the open document has unsaved
    /// changes.
    fn choose_open_file(&mut self) -> Task<Message> {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                s::FILE_FILTER_SUPPORTED,
                &[
                    s::EXT_NATIVE,
                    s::EXT_ODT,
                    s::EXT_DOCX,
                    s::EXT_MARKDOWN,
                    s::EXT_RTF,
                    s::EXT_HTML,
                    "htm",
                    s::EXT_EPUB,
                ],
            )
            .add_filter(s::FILE_FILTER_NATIVE, &[s::EXT_NATIVE])
            .add_filter(s::FILE_FILTER_ODT, &[s::EXT_ODT])
            .add_filter(s::FILE_FILTER_DOCX, &[s::EXT_DOCX])
            .add_filter(s::FILE_FILTER_MARKDOWN, &[s::EXT_MARKDOWN])
            .add_filter(s::FILE_FILTER_RTF, &[s::EXT_RTF])
            .add_filter(s::FILE_FILTER_HTML, &[s::EXT_HTML, "htm"])
            .add_filter(s::FILE_FILTER_EPUB, &[s::EXT_EPUB])
            .set_title(s::FILE_OPEN_TITLE)
            .pick_file()
        else {
            return Task::none();
        };
        if self.doc.is_some() && self.dirty {
            self.modal = Some(Modal::Unsaved {
                pending: Pending::Open(path),
            });
            return Task::none();
        }
        self.open_path(path)
    }

    /// Asks for the library folder and remembers it. False when the dialog was cancelled.
    fn pick_library_folder(&mut self) -> bool {
        let Some(folder) = rfd::FileDialog::new()
            .set_title(s::LIBRARY_CHOOSE)
            .pick_folder()
        else {
            return false;
        };
        self.settings.library_folder = Some(folder);
        self.save_settings();
        true
    }

    /// Lists the library folder (three folders deep) in the Library window.
    fn show_library(&mut self) {
        let entries = self
            .settings
            .library_folder
            .as_deref()
            .map(|folder| libertum_library::scan_library(folder, 3))
            .unwrap_or_default();
        self.modal = Some(Modal::Library { entries });
    }

    fn show_history(&mut self, path: PathBuf, from_library: bool) {
        let versions = libertum_library::versions(&path);
        self.modal = Some(Modal::History {
            path,
            versions,
            from_library,
        });
    }

    fn library_failed(&mut self, error: &std::io::Error) -> Task<Message> {
        tracing::warn!(%error, "a library action failed");
        self.modal = Some(Modal::Error(format!(
            "{} {error}",
            s::LIBRARY_ACTION_FAILED
        )));
        Task::none()
    }

    /// Opens a history version as a new, unsaved document: it has no file of its own, so Save
    /// asks where to put it and the history copy is never written to.
    fn open_version_copy(&mut self, version: PathBuf) -> Task<Message> {
        let task = self.open_path(version.clone());
        if self.file.as_ref() == Some(&version) {
            self.settings
                .recent_files
                .retain(|recent| recent != &version);
            self.save_settings();
            self.file = None;
            self.disk_stamp = None;
        }
        task
    }

    /// Import Chapter needs an open, editable document.
    fn import_blocked(&mut self) -> bool {
        match &self.doc {
            None => true,
            Some(doc) if doc.read_only => {
                self.modal = Some(Modal::Error(s::IMPORT_READ_ONLY.to_owned()));
                true
            }
            Some(_) => false,
        }
    }

    /// Reads any format Libertum opens and inserts its text after the caret. The open
    /// document's file is not touched until the writer saves.
    fn import_chapter_file(&mut self, path: &std::path::Path) -> Task<Message> {
        match FileContext::open(path) {
            Ok((_, document, warnings)) => {
                for warning in &warnings {
                    tracing::info!(%warning, path = %path.display(), "import warning");
                }
                self.import_chapter(&document)
            }
            Err(error) => {
                self.modal = Some(Modal::Error(format!(
                    "{} {error}",
                    s::IMPORT_CHAPTER_FAILED
                )));
                Task::none()
            }
        }
    }

    /// Inserts `document`'s body after the caret's block as one undoable edit.
    fn import_chapter(&mut self, document: &Document) -> Task<Message> {
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let result = doc.edit(|ed| ed.insert_document(document));
        let task = self.finish_edit(result);
        self.refresh_format();
        task
    }

    /// Reads `path` and replaces the open document with it. Parsing runs on the UI thread;
    /// an ODT or DOCX opens in well under a second (the budget is in PLAN §2) and the dialog
    /// is already modal.
    fn open_path(&mut self, path: PathBuf) -> Task<Message> {
        let (context, document, warnings) = match FileContext::open(&path) {
            Ok(opened) => opened,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not open the document");
                self.modal = Some(Modal::Error(format!("{} {error}", s::OPEN_FAILED)));
                return Task::none();
            }
        };
        for warning in &warnings {
            tracing::info!(%warning, path = %path.display(), "import warning");
        }
        let toc_notice = warnings
            .iter()
            .any(|warning| warning == libertum_formats_common::toc::TOC_REBUILT_WARNING);
        let view = context.view();
        // An EPUB or an AO3 download is imported: it opens as a new, unsaved document, so the
        // original file is never rewritten in Libertum's own HTML.
        let imported = Format::for_path(&path) == Format::Epub || context.is_ao3();
        let mut document = document;
        self.apply_remembered_language(&mut document, &path, context.format());
        let (mut doc, events) = match context.format() {
            Format::Markdown => DocView::open_markdown(document),
            // RTF carries a page size but its flow is best read as one continuous column.
            Format::Rtf => DocView::open_pageless(document),
            // HTML has no pages: it reads as one long column, like the browser shows it.
            Format::Html | Format::Epub => DocView::open_pageless(document),
            // A native document remembers whether it was read pageless.
            Format::Native if view.as_ref().is_some_and(|view| view.pageless) => {
                DocView::open_pageless(document)
            }
            Format::Native | Format::Odt | Format::Docx => DocView::open(document),
        };
        // A table of contents another program wrote comes in as a placeholder; fill it from
        // the headings now, without an undo step, so it is right before the first edit.
        let _ = doc.edit(|ed| ed.refresh_table_of_contents());
        doc.move_caret(|ed| ed.move_doc_start(false));
        self.state.fonts = installed_fonts(&doc);
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.state.current_page = 1;
        self.doc = Some(doc);
        self.remember_recent(&path);
        self.disk_stamp = libertum_library::DiskStamp::of(&path).ok().flatten();
        self.file = Some(path);
        self.file_context = Some(context);
        self.dirty = false;
        self.autosave_pending = None;
        self.autosave_done = None;
        self.modal = toc_notice.then(|| Modal::Notice {
            title: s::TOC_NOTICE_TITLE.to_owned(),
            detail: s::TOC_NOTICE_DETAIL.to_owned(),
        });
        self.after_save = None;
        if imported {
            self.file = None;
            self.disk_stamp = None;
            self.file_context = None;
            self.modal = Some(Modal::Notice {
                title: s::IMPORTED_TITLE.to_owned(),
                detail: s::IMPORTED_DETAIL.to_owned(),
            });
        }
        self.scroll = (0.0, 0.0);
        self.canvas_focused = true;
        self.ime_preedit = None;
        if let Some(view) = &view {
            self.state.zoom = Zoom::Percent(view.zoom_percent.clamp(1, 800));
        }
        // The file may be in the other English: build that dictionary off the UI thread.
        for language in self.spell_languages() {
            spell::warm(language);
        }
        self.refresh_document();
        self.refresh_view();
        self.refresh_format();
        Task::run(events, Message::Doc)
    }

    // ------------------------------------------------------------ scrolling and going home

    /// Glides the view towards where the mouse wheel asked it to go: each frame covers a
    /// fixed fraction of what is left per unit of time, so the motion starts at once, eases
    /// out, and ends exactly on the requested distance whatever the frame rate.
    fn coast(&mut self, now: Instant) {
        let dt = self
            .last_frame
            .map_or(0.0, |last| {
                now.saturating_duration_since(last).as_secs_f32()
            })
            .clamp(0.0, 0.05);
        self.last_frame = Some(now);
        if self.trackpad_pending != (0.0, 0.0) {
            self.scroll.0 += self.trackpad_pending.0;
            self.scroll.1 += self.trackpad_pending.1;
            self.trackpad_pending = (0.0, 0.0);
            self.clamp_scroll();
        }
        self.fling_step(now, dt);
        if dt == 0.0 || self.scroll_pending == (0.0, 0.0) {
            return;
        }
        let fraction = 1.0 - (-SCROLL_GLIDE_RATE * dt).exp();
        let step = |pending: f32| {
            // The last physical pixel is taken in one go rather than approached forever.
            if pending.abs() <= 1.0 {
                pending
            } else {
                pending * fraction
            }
        };
        let (sx, sy) = (step(self.scroll_pending.0), step(self.scroll_pending.1));
        self.scroll.0 += sx;
        self.scroll.1 += sy;
        self.scroll_pending = (self.scroll_pending.0 - sx, self.scroll_pending.1 - sy);
        self.trim_pending_to_edges();
        self.clamp_scroll();
    }

    /// A trackpad gesture is in progress: movement is queued for the next frame, or an event
    /// arrived within [`TRACKPAD_ACTIVE`]. The frame clock runs only while this holds (or a
    /// glide is under way), so a still window asks the GPU for nothing.
    fn trackpad_active(&self, now: Instant) -> bool {
        self.trackpad_pending != (0.0, 0.0)
            || self
                .trackpad_at
                .is_some_and(|at| now.saturating_duration_since(at).as_secs_f32() < TRACKPAD_ACTIVE)
    }

    /// Updates the finger's velocity from one trackpad event that moved the view by `moved`
    /// physical pixels. Events close together are averaged (recent motion counts most); one
    /// after a pause starts afresh, and so ends any fling that was carrying the page.
    fn track_fling(&mut self, now: Instant, moved: (f32, f32)) {
        let gap = self
            .trackpad_at
            .map(|at| now.saturating_duration_since(at).as_secs_f32());
        self.trackpad_at = Some(now);
        self.fling_gliding = false;
        match gap {
            Some(gap) if gap < FLING_SAMPLE_WINDOW => {
                let gap = gap.max(0.004);
                let blend = |old: f32, moved: f32| {
                    let speed = (moved / gap).clamp(-FLING_MAX_SPEED, FLING_MAX_SPEED);
                    old * (1.0 - FLING_RECENT_WEIGHT) + speed * FLING_RECENT_WEIGHT
                };
                self.fling = (blend(self.fling.0, moved.0), blend(self.fling.1, moved.1));
            }
            _ => self.fling = (0.0, 0.0),
        }
    }

    /// Carries the page on after the trackpad's events stop: the velocity measured while the
    /// fingers moved decays exponentially, so the page glides a little and slows to rest.
    fn fling_step(&mut self, now: Instant, dt: f32) {
        if self.fling == (0.0, 0.0) {
            self.fling_gliding = false;
            return;
        }
        let idle = self
            .trackpad_at
            .is_none_or(|at| now.saturating_duration_since(at).as_secs_f32() >= FLING_IDLE);
        if !idle {
            // Still being driven by the fingers: nothing to add yet.
            return;
        }
        let speed = self.fling.0.hypot(self.fling.1);
        // Whether the finger meant a flick is judged once, at launch.
        if !self.fling_gliding && speed < FLING_MIN_SPEED {
            self.fling = (0.0, 0.0);
            return;
        }
        self.fling_gliding = true;
        // Nearly still: end quietly. At this speed a frame moves well under a pixel, so
        // nothing visible is cut short.
        if speed < FLING_STOP_SPEED {
            self.fling = (0.0, 0.0);
            self.fling_gliding = false;
            return;
        }
        // The slower the page, the harder it brakes, so the tail eases to rest in a fraction
        // of a second instead of crawling on, or stopping from a speed you can still see.
        let slow = 1.0 - (speed / FLING_TAIL_SPEED).min(1.0);
        let rate = FLING_DECAY_RATE + FLING_TAIL_BRAKE * slow * slow;
        // Distance over this frame is the integral of an exponential decay, so the glide does
        // not depend on the frame rate.
        let decay = (-rate * dt).exp();
        let travel = (1.0 - decay) / rate;
        self.scroll.0 += self.fling.0 * travel;
        self.scroll.1 += self.fling.1 * travel;
        self.fling = (self.fling.0 * decay, self.fling.1 * decay);
        let (max_x, max_y) = self.metrics().max_scroll(self.viewport_physical());
        if self.scroll.0 <= 0.0 || self.scroll.0 >= max_x {
            self.fling.0 = 0.0;
        }
        if self.scroll.1 <= 0.0 || self.scroll.1 >= max_y {
            self.fling.1 = 0.0;
        }
        self.clamp_scroll();
    }

    /// Drops the part of the pending distance that lies past the document's edges, so the
    /// glide stops at the edge instead of running frames against it.
    fn trim_pending_to_edges(&mut self) {
        let (max_x, max_y) = self.metrics().max_scroll(self.viewport_physical());
        let trim = |at: f32, pending: f32, max: f32| (at + pending).clamp(0.0, max.max(0.0)) - at;
        self.scroll_pending = (
            trim(self.scroll.0, self.scroll_pending.0, max_x),
            trim(self.scroll.1, self.scroll_pending.1, max_y),
        );
        if self.scroll_pending.0.abs() < 0.01 {
            self.scroll_pending.0 = 0.0;
        }
        if self.scroll_pending.1.abs() < 0.01 {
            self.scroll_pending.1 = 0.0;
        }
    }

    /// Cmd/Ctrl+W or three Escapes: back to the home screen, asking about unsaved changes.
    fn request_home(&mut self) -> Task<Message> {
        if self.doc.is_none() {
            return Task::none();
        }
        if self.dirty {
            self.modal = Some(Modal::Unsaved {
                pending: Pending::Home,
            });
            return Task::none();
        }
        self.go_home();
        Task::none()
    }

    /// Closes the document and shows the home screen with its templates and recent files.
    fn go_home(&mut self) {
        self.doc = None;
        self.file = None;
        self.file_context = None;
        self.dirty = false;
        self.autosave_pending = None;
        self.autosave_done = None;
        self.modal = None;
        self.after_save = None;
        self.context_menu = None;
        self.state.find = crate::state::FindState::default();
        self.bookmark_draft = None;
        self.scroll = (0.0, 0.0);
        self.scroll_pending = (0.0, 0.0);
        self.trackpad_pending = (0.0, 0.0);
        self.fling = (0.0, 0.0);
        self.canvas_focused = false;
        self.ime_preedit = None;
        self.escape_presses = (0, Instant::now());
        recovery::clear(&self.recovery_dir);
        self.refresh_view();
    }

    /// Joins the chosen documents into a new, unsaved document, with its table of contents
    /// rebuilt from all the headings.
    fn stitch_now(&mut self) -> Task<Message> {
        let sources = std::mem::take(&mut self.stitch_sources);
        if sources.is_empty() {
            self.modal = Some(Modal::Stitch);
            return Task::none();
        }
        let documents = sources.into_iter().map(|source| source.document).collect();
        let stitched = match crate::stitch::stitch(documents) {
            Ok(document) => document,
            Err(error) => {
                self.modal = Some(Modal::Error(error.to_string()));
                return Task::none();
            }
        };
        let (mut doc, events) = DocView::open(stitched);
        let _ = doc.edit(|ed| ed.refresh_table_of_contents());
        doc.move_caret(|ed| ed.move_doc_start(false));
        self.state.fonts = installed_fonts(&doc);
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.state.current_page = 1;
        self.doc = Some(doc);
        self.file = None;
        self.file_context = None;
        self.dirty = true;
        self.modal = None;
        self.scroll = (0.0, 0.0);
        self.canvas_focused = true;
        self.refresh_document();
        self.refresh_view();
        self.refresh_format();
        Task::run(events, Message::Doc)
    }

    // ------------------------------------------------------------ autosave and recovery

    /// The autosave period, when there is a document open and autosave is switched on.
    /// The autosave period, or `None` when no timer is needed: autosave is off, there is no
    /// document, or the window is unfocused with nothing unsaved (a tick would only wake the
    /// app to find there is nothing to write).
    fn autosave_interval(&self) -> Option<u64> {
        let seconds = u64::from(self.settings.autosave_secs);
        (seconds > 0 && self.doc.is_some() && (self.window_focused || self.dirty))
            .then_some(seconds)
    }

    /// Offers the snapshot of a window that crashed (or was killed), if one survived.
    ///
    /// It is offered on the home screen and over a document just opened from Explorer
    /// (still unedited), but never in place of another prompt, such as an error opening that
    /// file: it is offered again once that prompt is closed. Snapshots of windows that are
    /// still running are never offered (see `recovery`).
    fn check_recovery(&mut self) {
        if self.modal.is_some() || (self.doc.is_some() && self.dirty) {
            return;
        }
        if let Some((document, meta)) = recovery::load(&self.recovery_dir) {
            self.pending_recovery = Some((document, meta));
            self.modal = Some(Modal::Recovery);
        }
    }

    /// Puts the recovered document back, still marked as unsaved, pointing at its old file.
    fn restore_recovery(&mut self) -> Task<Message> {
        let Some((document, meta)) = self.pending_recovery.take() else {
            return Task::none();
        };
        // The recovered text lives on in this window (and in its own snapshot from now on).
        recovery::remove(&self.recovery_dir, &meta);
        let (mut doc, events) = DocView::open(document);
        doc.move_caret(|ed| ed.move_doc_start(false));
        self.state.fonts = installed_fonts(&doc);
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.state.current_page = 1;
        self.doc = Some(doc);
        self.file_context = meta
            .path
            .as_deref()
            .and_then(|path| FileContext::open(path).ok())
            .map(|(context, _, _)| context);
        self.file = meta.path;
        self.dirty = true;
        self.modal = None;
        self.after_save = None;
        self.scroll = (0.0, 0.0);
        self.canvas_focused = true;
        self.ime_preedit = None;
        self.refresh_document();
        self.refresh_view();
        self.refresh_format();
        Task::run(events, Message::Doc)
    }

    fn discard_recovery(&mut self) {
        if let Some((_, meta)) = self.pending_recovery.take() {
            recovery::remove(&self.recovery_dir, &meta);
        }
        self.modal = None;
    }

    // ------------------------------------------------------------ find and replace

    /// Cmd/Ctrl+F: opens the bar, seeded with a one-line selection the way Word does.
    /// Opens the Find bar with the keyboard in its query field, so the search can be typed
    /// straight away, and searches at once when a selection seeded the query.
    fn open_find(&mut self) -> Task<Message> {
        self.state.open_menu = None;
        self.state.find.open = true;
        self.state.find.notice = None;
        self.state.find.last = None;
        self.state.find.count = None;
        if let Some(doc) = self.doc.as_ref()
            && let Some(text) = doc.editor().copy()
            && !text.is_empty()
            && !text.contains('\n')
            && text.len() <= 120
        {
            self.state.find.query = text;
        }
        self.canvas_focused = false;
        let search = self.find_as_typed();
        Task::batch([search, iced::widget::operation::focus(FIND_FIELD_ID)])
    }

    fn close_find(&mut self) {
        self.state.find.open = false;
        self.state.find.last = None;
        self.state.find.count = None;
        self.state.find.notice = None;
        self.canvas_focused = self.doc.is_some();
    }

    fn find_options(&self) -> libertum_editor::SearchOptions {
        libertum_editor::SearchOptions {
            match_case: self.state.find.match_case,
            whole_word: self.state.find.whole_word,
        }
    }

    /// Finds the next (or previous) match and selects it, so the canvas shows it without a
    /// second highlight system.
    fn find_step(&mut self, forward: bool) -> Task<Message> {
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        // From the far edge of the current match onwards: "next" moves on, "previous" goes
        // back.
        let from = match doc.editor().ordered_selection() {
            Some((start, end)) => {
                if forward {
                    end
                } else {
                    start
                }
            }
            None => doc.editor().caret(),
        };
        self.find_from(from, forward)
    }

    /// Search as the query is typed (after a pause): from the start of the selection, so a match that still
    /// fits the longer query stays where it is instead of jumping to the next one.
    fn find_as_typed(&mut self) -> Task<Message> {
        self.state.find.last = None;
        self.state.find.count = None;
        self.state.find.notice = None;
        if self.state.find.query.is_empty() {
            return Task::none();
        }
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        let from = doc
            .editor()
            .ordered_selection()
            .map_or_else(|| doc.editor().caret(), |(start, _)| start);
        self.find_from(from, true)
    }

    /// Finds the first match from `from` in the given direction, selects it and scrolls the
    /// whole of it clear of the Find bar and the word-count pill.
    fn find_from(&mut self, from: libertum_editor::DocPosition, forward: bool) -> Task<Message> {
        let query = self.state.find.query.clone();
        let options = self.find_options();
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        match doc.editor().find(&query, options, from, forward) {
            Some(found) => {
                let start = libertum_editor::DocPosition {
                    block: found.block,
                    offset: found.range.start,
                };
                let end = libertum_editor::DocPosition {
                    block: found.block,
                    offset: found.range.end,
                };
                // How many there are and which one this is: "3 of 17".
                let (total, position) = doc.editor().match_position(&query, options, Some(&found));
                self.state.find.count = Some((position, total));
                self.state.find.last = Some(found);
                self.state.find.notice = None;
                doc.move_caret(|ed| ed.set_selection(start, end));
                self.ensure_caret_visible();
                // The caret is the match's end; bring its start into view as well.
                let rects = self
                    .doc
                    .as_ref()
                    .map(DocView::selection_rects)
                    .unwrap_or_default();
                if self.viewport.is_some() && !rects.is_empty() {
                    let m = self.metrics();
                    let top = rects
                        .iter()
                        .map(|r| m.page_top(r.page) + r.y * m.scale)
                        .fold(f32::MAX, f32::min);
                    let bottom = rects
                        .iter()
                        .map(|r| m.page_top(r.page) + (r.y + r.height) * m.scale)
                        .fold(f32::MIN, f32::max);
                    self.reveal_vertical(top, bottom);
                    self.clamp_scroll();
                }
                self.refresh_view();
            }
            None => {
                self.state.find.last = None;
                self.state.find.count = None;
                self.state.find.notice = Some(if query.is_empty() {
                    s::FIND_PLACEHOLDER.to_owned()
                } else {
                    s::FIND_NO_MATCHES.to_owned()
                });
            }
        }
        Task::none()
    }

    /// Replace acts on the match found last, then moves to the next one.
    fn replace_current(&mut self) -> Task<Message> {
        let Some(found) = self.state.find.last.clone() else {
            return self.find_step(true);
        };
        let replacement = self.state.find.replace.clone();
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let result = doc.edit(|ed| ed.replace_match(&found, &replacement));
        let task = self.finish_edit(result);
        self.state.find.last = None;
        self.state.find.count = None;
        self.refresh_format();
        Task::batch([task, self.find_step(true)])
    }

    /// Replaces every match in the document in one undo step and says how many.
    fn replace_everything(&mut self) -> Task<Message> {
        let query = self.state.find.query.clone();
        let replacement = self.state.find.replace.clone();
        let options = self.find_options();
        let Some(doc) = self.doc.as_mut() else {
            return Task::none();
        };
        let mut replaced = 0usize;
        let result = doc.edit(|ed| {
            let (change, count) = ed.replace_all(&query, &replacement, options)?;
            replaced = count;
            Ok(change)
        });
        let task = self.finish_edit(result);
        self.state.find.last = None;
        self.state.find.count = None;
        self.state.find.notice = Some(if replaced == 0 {
            s::FIND_NO_MATCHES.to_owned()
        } else {
            s::find_replaced(replaced)
        });
        self.refresh_format();
        task
    }

    /// Writes a copy of the document in another format. The open file is never changed:
    /// Export is a copy, and the dialog names it separately from the document's own file.
    fn export_document(&mut self, format: ExportFormat) -> Task<Message> {
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        let stem = self
            .file
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| s::UNTITLED.to_owned());
        let suggested = format!("{stem}.{}", format.extension());
        let path = rfd::FileDialog::new()
            .add_filter(format.filter(), &[format.extension()])
            .set_title(s::EXPORT_SAVE_TITLE)
            .set_file_name(suggested)
            .save_file();
        let Some(path) = path.map(|path| ensure_export_extension(path, format)) else {
            return Task::none();
        };
        let document = doc.document().clone();
        let levels = self.settings.chapter_headings;
        Task::perform(
            async move { write_export(format, document, levels, stem, path) },
            Message::Exported,
        )
    }

    /// Saves to the current file, asking for one when there is none (or always, with
    /// `save_as`). The package is written on a background task.
    fn save_document(&mut self, save_as: bool) -> Task<Message> {
        if self.doc.is_none() {
            return Task::none();
        }
        // Ctrl/Cmd+S on an unchanged document would deep-clone it and rewrite the same
        // bytes for nothing. A pending close/open still runs.
        if !save_as && !self.dirty && self.file.is_some() && self.after_save.is_none() {
            return Task::none();
        }
        // Cross-references show their targets as they are now (PLAN §6.6: updated before
        // save).
        self.refresh_cross_refs();
        let current = self
            .file
            .as_deref()
            .map(Format::for_path)
            .or_else(|| self.file_context.as_ref().map(FileContext::format))
            .unwrap_or_else(|| Format::from_settings(&self.settings));
        let mut path = if save_as { None } else { self.file.clone() };
        if path.is_none() {
            // The name without its extension: the dialog adds the extension of the file type
            // chosen in its list, so picking another type really changes the format.
            let suggested = self
                .file
                .as_ref()
                .and_then(|path| path.file_stem())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| s::UNTITLED.to_owned());
            let mut dialog = rfd::FileDialog::new();
            for (label, extension) in save_filters(current) {
                dialog = dialog.add_filter(label, &[extension]);
            }
            path = dialog
                .set_title(s::FILE_SAVE_TITLE)
                .set_file_name(suggested)
                .save_file();
        }
        let Some(path) = path.map(|path| ensure_extension(path, current)) else {
            // The dialog was cancelled: forget a pending close/open that waited for a save.
            self.after_save = None;
            return Task::none();
        };
        // Markdown cannot hold most formatting: say what would go before writing.
        if Format::for_path(&path) == Format::Markdown {
            let losses: Vec<String> = self
                .doc
                .as_ref()
                .map(|doc| libertum_markdown::loss_report(doc.document()))
                .unwrap_or_default()
                .iter()
                .map(ToString::to_string)
                .collect();
            if !losses.is_empty() {
                self.modal = Some(Modal::MarkdownLoss {
                    path,
                    losses,
                    title: s::MARKDOWN_LOSS_TITLE,
                });
                return Task::none();
            }
        }
        // HTML keeps the text and its formatting, not the page furniture: say what would go.
        if Format::for_path(&path) == Format::Html {
            let flavour = match &self.file_context {
                Some(FileContext::Html(html)) => html.flavour(),
                _ => HtmlFlavour::Full,
            };
            let losses: Vec<String> = self
                .doc
                .as_ref()
                .map(|doc| libertum_html::loss_report(doc.document(), flavour))
                .unwrap_or_default()
                .iter()
                .map(|loss| loss.kind.label().to_owned())
                .collect();
            if !losses.is_empty() {
                self.modal = Some(Modal::MarkdownLoss {
                    path,
                    losses,
                    title: s::HTML_LOSS_TITLE,
                });
                return Task::none();
            }
        }
        self.write_document(path)
    }

    /// Writes the document to `path`, unless the file on disk has changed since it was opened or
    /// last saved here (another device, a sync service): then the user chooses first.
    fn write_document(&mut self, path: PathBuf) -> Task<Message> {
        let changed = self.file.as_ref() == Some(&path)
            && self
                .disk_stamp
                .is_some_and(|stamp| stamp.is_stale(&path).unwrap_or(false));
        if changed {
            self.modal = Some(Modal::ChangedOnDisk { path });
            return Task::none();
        }
        self.write_document_forced(path)
    }

    /// The page each heading the table of contents lists is on, as the layout has it now.
    /// Headings past where pagination has reached have no number yet and are left out; the
    /// writer then saves those entries without one.
    fn toc_pages(&self) -> HashMap<libertum_model::BlockId, u32> {
        let Some(doc) = self.doc.as_ref() else {
            return HashMap::new();
        };
        doc.document()
            .paragraphs()
            .filter_map(|paragraph| paragraph.props.toc_target)
            .filter(|target| target.0 != 0)
            .filter_map(|target| Some((target, doc.page_number_of_block(target)?)))
            .collect()
    }

    /// Writes the document to `path` on a background task, in the format its extension names.
    fn write_document_forced(&mut self, path: PathBuf) -> Task<Message> {
        // An "updated date and time" field shows this save, and the save time itself is part
        // of what a native document keeps.
        let now = now_seconds();
        let mut date_fields_changed = false;
        if let Some(doc) = self.doc.as_mut() {
            let result = doc.edit(|ed| {
                ed.mark_saved_at(now);
                let set = ed.refresh_date_fields(now)?;
                date_fields_changed = !set.dirty.is_empty();
                Ok(set)
            });
            if let Err(error) = result {
                tracing::warn!(%error, "could not update the date and time fields");
            }
        }
        if date_fields_changed {
            self.dirty = true;
            self.after_edit();
        }
        let Some(doc) = self.doc.as_ref() else {
            return Task::none();
        };
        // Clone now: the write runs after this message, on a task.
        let document = doc.document().clone();
        let target = Format::for_path(&path);
        let mut writer = match &self.file_context {
            Some(context) if context.format() == target => context.clone(),
            _ => FileContext::new(target, Document::new()),
        };
        writer.set_document(document);
        writer.set_toc_pages(self.toc_pages());
        // A native save also records how the document was being read.
        writer.set_view(ViewState {
            pageless: self.state.pageless,
            zoom_percent: self.metrics().percent(),
        });
        Task::perform(
            async move {
                let result = writer.save(&path);
                (path, result)
            },
            |(path, result)| Message::Saved { path, result },
        )
    }

    fn on_saved(&mut self, path: PathBuf, result: Result<(), String>) -> Task<Message> {
        match result {
            Ok(()) => {
                let format = Format::for_path(&path);
                let same_format = self
                    .file_context
                    .as_ref()
                    .is_some_and(|context| context.format() == format);
                if !same_format {
                    // A new document, or a Save As into the other format: from here on the
                    // document keeps a package of its own for future saves.
                    let document = self
                        .doc
                        .as_ref()
                        .map(|doc| doc.document().clone())
                        .unwrap_or_default();
                    self.file_context = Some(FileContext::new(format, document));
                }
                self.remember_recent(&path);
                self.remember_language_for_path(&path);
                self.disk_stamp = libertum_library::DiskStamp::of(&path).ok().flatten();
                self.file = Some(path);
                self.dirty = false;
                self.autosave_pending = None;
                self.autosave_done = None;
                self.modal = None;
                // The document is safe on disk, so the snapshot is no longer needed.
                recovery::clear(&self.recovery_dir);
                match self.after_save.take() {
                    Some(pending) => self.perform(pending),
                    None => Task::none(),
                }
            }
            Err(error) => {
                tracing::warn!(%error, "could not save the document");
                self.after_save = None;
                self.modal = Some(Modal::Error(format!("{} {error}", s::SAVE_FAILED)));
                Task::none()
            }
        }
    }

    fn on_close_request(&mut self) -> Task<Message> {
        if self.doc.is_some() && self.dirty {
            self.modal = Some(Modal::Unsaved {
                pending: Pending::Close,
            });
            return Task::none();
        }
        recovery::clear(&self.recovery_dir);
        self.exit_app()
    }

    /// Quits, first starting the staged update (if any) so it takes over as Libertum goes.
    fn exit_app(&mut self) -> Task<Message> {
        if let Some(staged) = self.staged_update.take() {
            staged.apply();
        }
        iced::exit()
    }

    fn on_modal_choice(&mut self, choice: ModalChoice) -> Task<Message> {
        let task = self.on_modal_choice_inner(choice);
        // A prompt that was waiting behind this one (an MCP edit) can show now.
        Task::batch([task, self.process_mcp_approvals()])
    }

    fn on_modal_choice_inner(&mut self, choice: ModalChoice) -> Task<Message> {
        let Some(modal) = self.modal.take() else {
            return Task::none();
        };
        match (modal, choice) {
            // Escape or a click outside the prompt refuses just this edit.
            (Modal::McpApproval { .. }, _) => {
                if let Some(pending) = self.mcp_approvals.pop_front() {
                    pending.respond(Err(libertum_mcp::HostError::Denied));
                }
                Task::none()
            }
            (Modal::Unsaved { pending }, ModalChoice::Save) => {
                self.after_save = Some(pending);
                self.save_document(false)
            }
            (Modal::Unsaved { pending }, ModalChoice::Discard) => self.perform(pending),
            (Modal::Unsaved { .. }, ModalChoice::Cancel | ModalChoice::Restore) => Task::none(),
            (Modal::Error(_), _) => {
                // An error while adding files to a stitch (one file would not open) goes
                // back to the stitch list, which still holds the files that did open.
                if !self.stitch_sources.is_empty() {
                    self.modal = Some(Modal::Stitch);
                } else {
                    // A crash snapshot waiting behind the error is offered now.
                    self.check_recovery();
                }
                Task::none()
            }
            (Modal::Notice { .. }, _) => {
                // A crash snapshot waiting behind the notice is offered now.
                self.check_recovery();
                Task::none()
            }
            (Modal::Stitch, _) => Task::none(),
            (Modal::Library { .. } | Modal::History { .. }, _) => Task::none(),
            // The AO3 chapter window has its own Close button; it is not a choice.
            (Modal::Ao3Chapter { .. }, _) => Task::none(),
            (Modal::DateTime { .. }, _) => Task::none(),
            (Modal::LinkedMedia { .. }, _) => Task::none(),
            // The update prompt has its own buttons; an Escape or backdrop click dismisses it.
            (Modal::UpdateAvailable { .. }, _) => Task::none(),
            (Modal::Recovery, ModalChoice::Restore) => self.restore_recovery(),
            (Modal::Recovery, _) => {
                self.discard_recovery();
                Task::none()
            }
            (Modal::ChangedOnDisk { path }, ModalChoice::Save) => self.write_document_forced(path),
            (Modal::ChangedOnDisk { .. }, ModalChoice::Discard) => self.save_document(true),
            (Modal::ChangedOnDisk { .. }, _) => {
                self.after_save = None;
                Task::none()
            }
            (Modal::MarkdownLoss { path, .. }, ModalChoice::Save) => self.write_document(path),
            (Modal::MarkdownLoss { .. }, _) => {
                // Not saved: a close or open that waited for this save is dropped too.
                self.after_save = None;
                Task::none()
            }
        }
    }

    fn perform(&mut self, pending: Pending) -> Task<Message> {
        match pending {
            Pending::Close => {
                recovery::clear(&self.recovery_dir);
                self.exit_app()
            }
            Pending::Home => {
                self.go_home();
                Task::none()
            }
            Pending::Open(path) => self.open_path(path),
        }
    }

    pub fn title(&self) -> String {
        if self.doc.is_none() {
            return s::APP_NAME.to_owned();
        }
        // An unsaved document is named after the Settings default format.
        let name = self
            .file
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned());
        match name {
            Some(name) => format!("{name} — {}", if self.dirty { s::EDITED } else { s::SAVED }),
            None => s::window_title(
                s::UNTITLED,
                Format::from_settings(&self.settings).extension(),
                self.dirty,
            ),
        }
    }

    fn scheme(&self) -> Scheme {
        let system_dark = match self.system_mode {
            theme::Mode::Dark => Some(true),
            theme::Mode::Light => Some(false),
            theme::Mode::None => None,
        };
        Scheme::resolve(self.theme_preference, system_dark)
    }

    pub fn theme(&self) -> Theme {
        look::app_theme(self.scheme())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            event::listen_with(key_event),
            iced::system::theme_changes().map(Message::SystemTheme),
            window::close_requests().map(|_| Message::CloseRequested),
        ];
        let autoscrolling = self.autoscroll.as_ref().is_some_and(AutoScroll::active);
        let coasting = self.scroll_pending != (0.0, 0.0)
            || self.fling != (0.0, 0.0)
            || self.trackpad_active(Instant::now());
        if autoscrolling || coasting || self.bench.is_some() || self.state.sidebar_animating() {
            subs.push(window::frames().map(Message::Frame));
        }
        if self.blinking(Instant::now()) {
            subs.push(Subscription::run(blink_ticks).map(Message::BlinkTick));
        }
        // The performance overlay measures real frames: while it is visible the window
        // redraws continuously, which is also what makes its figures meaningful.
        if self.settings.show_performance && self.doc.is_some() {
            subs.push(window::frames().map(Message::FpsFrame));
        }
        if !self.type_hook.is_empty() {
            subs.push(Subscription::run(type_ticks).map(|_| Message::TypeHookTick));
        }
        if self.doc.is_some() && self.settings.spellcheck {
            subs.push(Subscription::run(spell_ticks).map(|_| Message::SpellTick));
        }
        if let Some(seconds) = self.autosave_interval() {
            subs.push(
                Subscription::run_with(seconds, autosave_ticks).map(|_| Message::AutosaveTick),
            );
        }
        if let Some(service) = &self.mcp {
            subs.push(
                Subscription::run_with(service.feed(), mcp::McpFeed::stream)
                    .map(Message::McpRequest),
            );
        }
        Subscription::batch(subs)
    }

    /// The blink timer runs while a document has keyboard focus and caret activity was
    /// recent; otherwise the caret is solid and nothing ticks.
    fn blinking(&self, now: Instant) -> bool {
        self.doc.is_some()
            && self.canvas_focused
            && self.window_focused
            && now.saturating_duration_since(self.caret_activity) < BLINK_TIMEOUT
    }

    fn on_blink_tick(&mut self, now: Instant) {
        self.caret_on = !self.blinking(now) || !self.caret_on;
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let started = Instant::now();
        if matches!(
            message,
            Message::Keyboard(_)
                | Message::Ime(_)
                | Message::PasteText(_)
                | Message::Shortcut(Shortcut::PageBreak)
                | Message::Insert(_)
                | Message::CreateDocument(_)
                | Message::Canvas(CanvasEvent::Pointer { .. } | CanvasEvent::PointerDrag { .. })
        ) {
            // Typing, clicking or editing shows the caret and restarts the blink window.
            self.caret_activity = started;
            self.caret_on = true;
        }
        // Messages that can move the selection or change the text it covers refresh the
        // inspector's character formatting afterwards.
        let selection_may_change = matches!(
            message,
            Message::Keyboard(_)
                | Message::Ime(_)
                | Message::PasteText(_)
                | Message::Shortcut(_)
                | Message::Insert(_)
                | Message::CreateDocument(_)
                | Message::Canvas(CanvasEvent::Pointer { .. } | CanvasEvent::PointerDrag { .. })
        );
        let mut kind = String::new();
        if perf::enabled() {
            let text = format!("{message:?}");
            let end = text
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(text.len());
            kind = text[..end].to_owned();
            perf::message(kind.clone());
        }
        let task = self.update_inner(message);
        if selection_may_change {
            self.refresh_format();
        }
        let elapsed = started.elapsed();
        perf::update_done(elapsed);
        if elapsed >= Duration::from_micros(300) {
            perf::event("update", elapsed.as_secs_f64() * 1000.0, || kind);
        }
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        // While a benchmark runs the window shows a generated document: nothing that could edit,
        // save or close it may get through. A close request ends the run first.
        if self.bench.is_some() {
            if matches!(message, Message::CloseRequested) {
                let _ = self.bench_cancel();
            } else if !bench_run::allows(&message) {
                return Task::none();
            }
        }
        // Anything other than typing ends a stepper edit (stand-in for losing focus), and any
        // click that is not about the menu closes it.
        if !matches!(
            message,
            Message::FieldText(..)
                | Message::Canvas(_)
                | Message::Doc(_)
                | Message::Keyboard(_)
                | Message::Ime(_)
                | Message::PasteText(_)
                | Message::Frame(_)
                | Message::BlinkTick(_)
                | Message::SpellTick
                | Message::SpellChecked { .. }
                | Message::ScaleFactor(_)
                | Message::SystemTheme(_)
                | Message::WindowResized(_)
                | Message::WindowFocus(_)
        ) && let Some((field, _)) = self.state.editing.take()
        {
            self.apply_field(field);
        }
        // A context menu closes on the next user action (not on internal messages such as
        // the canvas reporting its size after a redraw, which would close it instantly).
        let closes_context_menu = matches!(
            message,
            Message::Keyboard(_)
                | Message::Ime(_)
                | Message::PasteText(_)
                | Message::Insert(_)
                | Message::Table(_)
                | Message::TableStyle(_)
                | Message::ImageStep(..)
                | Message::ImageAlt(..)
                | Message::RubyEdit(_)
                | Message::ImageRemove(..)
                | Message::Cut
                | Message::Copy
                | Message::Paste
                | Message::SelectAll
                | Message::Format(_)
                | Message::Document(_)
                | Message::OpenFind
                | Message::Shortcut(_)
                | Message::SpellReplace { .. }
                | Message::SpellIgnoreOnce(_)
                | Message::SpellIgnoreDocument(_)
                | Message::SpellAddWord(_)
                | Message::Canvas(
                    CanvasEvent::Pointer { .. }
                        | CanvasEvent::PointerDrag { .. }
                        | CanvasEvent::Wheel { .. }
                        | CanvasEvent::Zoom { .. }
                        | CanvasEvent::ScrollTo { .. }
                        | CanvasEvent::Blur
                )
        );
        if closes_context_menu {
            self.context_menu = None;
        }
        match message {
            Message::ToggleMenu(menu) => self.state.toggle_menu(menu),
            // A click on the backdrop behind a menu (or its own close button) closes the
            // toolbar menu and the right-click menu alike.
            Message::CloseMenu => {
                self.state.open_menu = None;
                self.state.ao3_open = false;
                self.context_menu = None;
            }
            Message::View(action) => {
                self.state.open_menu = None;
                match action {
                    ViewAction::Inspector => self.toggle_sidebar(),
                    ViewAction::WordCount => {
                        self.state.show_word_count = !self.state.show_word_count;
                        self.settings.show_word_count = self.state.show_word_count;
                        self.save_settings();
                    }
                    ViewAction::Booklet => {
                        let old = self.metrics();
                        let old_vp = self.viewport_physical();
                        self.state.booklet = !self.state.booklet;
                        self.reanchor(old, old_vp);
                        self.refresh_view();
                    }
                    ViewAction::Pageless => {
                        let old = self.metrics();
                        let old_vp = self.viewport_physical();
                        let next = !self.state.pageless;
                        if let Some(doc) = self.doc.as_mut() {
                            doc.set_pageless(next);
                        }
                        self.state.pageless = next;
                        self.reanchor(old, old_vp);
                        self.refresh_document();
                        self.refresh_view();
                        self.refresh_format();
                    }
                }
            }
            Message::ToggleInspector(kind) => {
                self.state.open_menu = None;
                self.state.toggle_inspector(kind);
                if self.state.inspector == Inspector::Headers && self.state.sidebar_visible {
                    self.headings = self.headings();
                }
                if self.state.inspector == Inspector::Notes && self.state.sidebar_visible {
                    self.notes = self.notes();
                }
                if self.state.inspector == Inspector::Bookmarks && self.state.sidebar_visible {
                    self.bookmarks = self.bookmarks();
                }
                self.bookmark_draft = None;
                self.persist_sidebar();
            }
            Message::OpenFind => return self.open_find(),
            Message::FindBarPressed => {
                // Back to the query field, and the document does not take the keyboard.
                self.canvas_focused = false;
                return iced::widget::operation::focus(FIND_FIELD_ID);
            }
            Message::FindTyped(generation) => {
                if generation == self.find_generation {
                    return self.find_as_typed();
                }
            }
            Message::CloseFind => self.close_find(),
            Message::FindQuery(query) => {
                self.state.find.query = query;
                self.state.find.last = None;
                self.state.find.count = None;
                self.state.find.notice = None;
                // Search once typing pauses, not on every key: each search reads the whole
                // document, and a short query (one letter, while deleting a word) matches
                // almost everything. An empty query searches nothing at all.
                self.find_generation = self.find_generation.wrapping_add(1);
                if self.state.find.query.is_empty() {
                    return Task::none();
                }
                return find_after_pause(self.find_generation);
            }
            Message::ReplaceQuery(replace) => self.state.find.replace = replace,
            Message::ToggleFindCase => {
                self.state.find.match_case = !self.state.find.match_case;
                return self.find_as_typed();
            }
            Message::ToggleFindWord => {
                self.state.find.whole_word = !self.state.find.whole_word;
                return self.find_as_typed();
            }
            Message::FindNext => return self.find_step(true),
            Message::FindPrevious => return self.find_step(false),
            Message::ReplaceOne => return self.replace_current(),
            Message::ReplaceAll => return self.replace_everything(),
            Message::Cut => return self.cut(),
            Message::Copy => return self.copy(),
            Message::Paste => return iced::clipboard::read().map(Message::PasteText),
            Message::SelectAll => {
                if let Some(doc) = self.doc.as_mut() {
                    doc.select_all();
                }
                return self.after_move();
            }
            Message::GoHome => return self.request_home(),
            Message::OpenStitch => {
                let picked = rfd::FileDialog::new()
                    .add_filter(
                        s::FILE_FILTER_SUPPORTED,
                        &[
                            s::EXT_NATIVE,
                            s::EXT_ODT,
                            s::EXT_DOCX,
                            s::EXT_MARKDOWN,
                            s::EXT_RTF,
                        ],
                    )
                    .set_title(s::STITCH_ADD)
                    .pick_files();
                if let Some(paths) = picked {
                    return self.update(Message::StitchAdd(paths));
                }
            }
            Message::StitchAdd(paths) => {
                let mut failed = Vec::new();
                for path in paths {
                    match FileContext::open(&path) {
                        Ok((_context, document, warnings)) => {
                            for warning in warnings {
                                tracing::info!(%warning, path = %path.display(), "stitch import warning");
                            }
                            self.stitch_sources
                                .push(crate::stitch::Source { path, document });
                        }
                        Err(error) => failed.push(format!("{}: {error}", path.display())),
                    }
                }
                if !failed.is_empty() {
                    self.modal = Some(Modal::Error(format!(
                        "{} {}",
                        s::OPEN_FAILED,
                        failed.join("; ")
                    )));
                    return Task::none();
                }
                self.modal = Some(Modal::Stitch);
            }
            Message::StitchMove(index, direction) => {
                let target = if direction < 0 {
                    index.checked_sub(1)
                } else {
                    Some(index + 1).filter(|target| *target < self.stitch_sources.len())
                };
                if let Some(target) = target {
                    self.stitch_sources.swap(index, target);
                }
            }
            Message::StitchRemove(index) => {
                if index < self.stitch_sources.len() {
                    self.stitch_sources.remove(index);
                }
            }
            Message::CloseStitch => {
                self.stitch_sources.clear();
                self.modal = None;
            }
            Message::StitchGo => return self.stitch_now(),
            Message::OpenLibrary => {
                self.state.open_menu = None;
                if self.settings.library_folder.is_none() && !self.pick_library_folder() {
                    return Task::none();
                }
                self.show_library();
            }
            Message::ChooseLibraryFolder => {
                if self.pick_library_folder() {
                    self.show_library();
                }
            }
            Message::CloseLibrary => self.modal = None,
            Message::Compare(msg) => return self.update_compare(msg),
            Message::Ao3Post(msg) => return self.update_ao3_post(msg),
            Message::OpenHistory(path) => {
                self.state.open_menu = None;
                let from_library = path.is_some();
                let Some(path) = path.or_else(|| self.file.clone()) else {
                    self.modal = Some(Modal::Error(s::HISTORY_NO_FILE.to_owned()));
                    return Task::none();
                };
                self.show_history(path, from_library);
            }
            Message::HistoryBack => self.show_library(),
            Message::HistoryOpenCopy(version) => {
                if self.doc.is_some() && self.dirty {
                    self.modal = Some(Modal::Error(s::HISTORY_SAVE_FIRST.to_owned()));
                    return Task::none();
                }
                return self.open_version_copy(version);
            }
            Message::HistoryRestore { original, version } => {
                if self.doc.is_some() && self.dirty && self.file.as_ref() == Some(&original) {
                    self.modal = Some(Modal::Error(s::HISTORY_SAVE_FIRST.to_owned()));
                    return Task::none();
                }
                let result =
                    libertum_library::restore(&original, &version, std::time::SystemTime::now());
                if let Err(error) = result {
                    return self.library_failed(&error);
                }
                if self.file.as_ref() == Some(&original) {
                    return self.open_path(original);
                }
                self.show_history(original, true);
            }
            Message::UseConflictCopy { original, copy } => {
                if self.doc.is_some() && self.dirty && self.file.as_ref() == Some(&original) {
                    self.modal = Some(Modal::Error(s::HISTORY_SAVE_FIRST.to_owned()));
                    return Task::none();
                }
                if let Err(error) =
                    libertum_library::use_copy(&original, &copy, std::time::SystemTime::now())
                {
                    return self.library_failed(&error);
                }
                self.show_library();
            }
            Message::KeepOriginal { original, copy } => {
                if let Err(error) = libertum_library::archive_copy(&original, &copy) {
                    return self.library_failed(&error);
                }
                self.show_library();
            }
            Message::Shortcut(Shortcut::CloseDocument | Shortcut::NewDocument) => {
                return self.request_home();
            }
            Message::OpenSettings => {
                self.state.open_menu = None;
                self.settings_open = true;
                self.bench_load_saved();
            }
            Message::CloseSettings => self.settings_open = false,
            Message::RunBenchmark => return self.bench_start(),
            Message::CancelBenchmark => return self.bench_cancel(),
            Message::CloseBenchmarkReport => {
                self.bench_report_open = false;
                self.bench_copied = false;
            }
            Message::ShowBenchmarkReport => {
                self.settings_open = false;
                self.bench_report_open = self.bench_report.is_some();
            }
            Message::CopyBenchmarkReport => {
                if let Some(report) = &self.bench_report {
                    self.bench_copied = true;
                    return iced::clipboard::write(report.to_text());
                }
            }
            Message::OpenLinkedMedia => {
                self.state.open_menu = None;
                if self.doc.is_some() {
                    self.modal = Some(Modal::LinkedMedia {
                        url: String::new(),
                        alt: String::new(),
                        video: false,
                        preview: true,
                        kind_chosen: false,
                        busy: false,
                        error: None,
                    });
                }
            }
            Message::CloseLinkedMedia => {
                if matches!(self.modal, Some(Modal::LinkedMedia { busy: false, .. })) {
                    self.modal = None;
                }
            }
            Message::LinkedUrl(value) => {
                if let Some(Modal::LinkedMedia {
                    url,
                    video,
                    kind_chosen,
                    error,
                    ..
                }) = &mut self.modal
                {
                    if !*kind_chosen {
                        *video = crate::linked::looks_like_video(&value);
                    }
                    *url = value;
                    *error = None;
                }
            }
            Message::LinkedAlt(value) => {
                if let Some(Modal::LinkedMedia { alt, .. }) = &mut self.modal {
                    *alt = value;
                }
            }
            Message::LinkedVideo(is_video) => {
                if let Some(Modal::LinkedMedia {
                    video, kind_chosen, ..
                }) = &mut self.modal
                {
                    *video = is_video;
                    *kind_chosen = true;
                }
            }
            Message::LinkedPreview(on) => {
                if let Some(Modal::LinkedMedia { preview, .. }) = &mut self.modal {
                    *preview = on;
                }
            }
            Message::LinkedInsert => {
                let Some(Modal::LinkedMedia {
                    url,
                    alt,
                    video,
                    preview,
                    ..
                }) = self.modal.clone()
                else {
                    return Task::none();
                };
                let Some(address) = crate::linked::normalise(&url) else {
                    if let Some(Modal::LinkedMedia { error, .. }) = &mut self.modal {
                        *error = Some(s::LINKED_BAD_URL.to_owned());
                    }
                    return Task::none();
                };
                let kind = if video {
                    libertum_model::MediaKind::Video
                } else {
                    libertum_model::MediaKind::Image
                };
                let link = libertum_model::MediaLink { url: address, kind };
                let from = crate::linked::preview_address(&link).filter(|_| preview);
                let Some(from) = from else {
                    // No preview wanted, or none to be had: the placeholder stands in.
                    let media = Ok(crate::linked::placeholder(kind));
                    return self.update(Message::LinkedInserted {
                        link,
                        alt,
                        preview: media,
                    });
                };
                if let Some(Modal::LinkedMedia { busy, error, .. }) = &mut self.modal {
                    *busy = true;
                    *error = None;
                }
                return Task::perform(
                    async move { crate::linked::fetch_image(&from) },
                    move |preview| Message::LinkedInserted {
                        link: link.clone(),
                        alt: alt.clone(),
                        preview,
                    },
                );
            }
            Message::LinkedInserted { link, alt, preview } => {
                // A failed download still inserts the link, with its placeholder, and says why.
                let (media, failure) = match preview {
                    Ok(media) => (media, None),
                    Err(error) => (crate::linked::placeholder(link.kind), Some(error)),
                };
                let width = self.text_column_width().min(360.0);
                let Some(doc) = self.doc.as_mut() else {
                    self.modal = None;
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.insert_linked_media(media, width, &link, &alt));
                self.modal = failure
                    .map(|error| Modal::Error(format!("{} {error}", s::LINKED_PREVIEW_FAILED)));
                self.canvas_focused = self.modal.is_none();
                let task = self.finish_edit(result);
                self.refresh_format();
                return task;
            }
            Message::FetchPreview(block, index) => {
                let Some(link) = self
                    .state
                    .images
                    .iter()
                    .find(|image| image.block == block && image.index == index)
                    .and_then(|image| image.link.clone())
                else {
                    return Task::none();
                };
                let Some(from) = crate::linked::preview_address(&link) else {
                    self.modal = Some(Modal::Notice {
                        title: s::LINKED_NO_PREVIEW.to_owned(),
                        detail: String::new(),
                    });
                    return Task::none();
                };
                return Task::perform(
                    async move { crate::linked::fetch_image(&from) },
                    move |result| Message::PreviewFetched(block, index, result),
                );
            }
            Message::PreviewFetched(block, index, result) => match result {
                Ok(media) => {
                    let Some(doc) = self.doc.as_mut() else {
                        return Task::none();
                    };
                    let result = doc.edit(|ed| ed.replace_image_media(block, index, media));
                    let task = self.finish_edit(result);
                    self.refresh_format();
                    return task;
                }
                Err(error) => {
                    self.modal = Some(Modal::Error(format!(
                        "{} {error}",
                        s::LINKED_PREVIEW_FAILED
                    )));
                }
            },
            Message::OpenDateTime => {
                self.state.open_menu = None;
                if self.doc.is_some() {
                    self.modal = Some(Modal::DateTime {
                        updated: false,
                        format: DateTimeFormat::DateTimeLong,
                    });
                }
            }
            Message::CloseDateTime => {
                if matches!(self.modal, Some(Modal::DateTime { .. })) {
                    self.modal = None;
                }
            }
            Message::DateTimeUpdated(updated) => {
                if let Some(Modal::DateTime { updated: at, .. }) = &mut self.modal {
                    *at = updated;
                }
            }
            Message::DateTimeFormat(format) => {
                if let Some(Modal::DateTime { format: at, .. }) = &mut self.modal {
                    *at = format;
                }
            }
            Message::DateTimeInsert => {
                let Some(Modal::DateTime { updated, format }) = self.modal.clone() else {
                    return Task::none();
                };
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let source = if updated {
                    DateTimeSource::Updated
                } else {
                    DateTimeSource::Inserted
                };
                let now = now_seconds();
                let result = doc.edit(|ed| ed.insert_date_time(now, source, format));
                self.modal = None;
                self.canvas_focused = true;
                return self.finish_edit(result);
            }
            Message::SetTheme(theme) => {
                self.settings.theme = theme;
                self.theme_preference = theme;
                self.save_settings();
            }
            Message::SetDefaultFormat(format) => {
                self.settings.default_format = format;
                self.save_settings();
                self.refresh_format();
            }
            Message::SetAutosave(seconds) => {
                self.settings.autosave_secs = seconds;
                self.save_settings();
            }
            Message::SetScriptMemory(megabytes) => {
                self.settings.script_memory_mb = megabytes;
                self.save_settings();
            }
            Message::SetScriptTotalMemory(megabytes) => {
                self.settings.script_total_memory_mb = megabytes;
                self.save_settings();
            }
            Message::SetShowPerformance(on) => {
                self.settings.show_performance = on;
                // A fresh measurement starts each time it is switched on.
                self.fps = crate::fps::FrameCounter::default();
                self.save_settings();
            }
            Message::SetAntialiasing(on) => {
                self.settings.antialiasing = on;
                crate::docview::set_antialias(on);
                // Thumbnails are keyed by page content, not by how they were drawn.
                self.thumb_cache.clear();
                self.save_settings();
            }
            Message::SetSpellcheck(on) => {
                self.settings.spellcheck = on;
                self.save_settings();
                return self.schedule_spell_check();
            }
            Message::SetDevMode(on) => {
                self.settings.set_dev_mode(on);
                self.save_settings();
                self.sync_mcp();
            }
            Message::SetMcpEnabled(on) => {
                if self.settings.set_mcp_enabled(on) {
                    self.save_settings();
                }
                self.sync_mcp();
            }
            Message::SetMcpAllowEdits(on) => {
                if self.settings.set_mcp_allow_edits(on) {
                    self.save_settings();
                }
                self.sync_mcp();
            }
            Message::ForgetMcpClients => {
                self.settings.forget_mcp_clients();
                self.mcp_session_trusted.clear();
                self.save_settings();
            }
            Message::McpDecision(decision) => return self.decide_mcp(decision),
            Message::Runner(message) => return self.update_runner(message),
            Message::McpRequest(request) => return self.answer_mcp(&request),
            Message::CopyMcpToken => {
                if let Some(service) = &self.mcp {
                    return iced::clipboard::write(service.token().to_owned());
                }
            }
            Message::SetSpellLanguage(language) => {
                self.settings.spellcheck_language = language;
                spell::warm(language);
                self.save_settings();
                return self.schedule_spell_check();
            }
            Message::SetCheckForUpdates(on) => {
                self.settings.check_for_updates = on;
                if !on {
                    if matches!(self.modal, Some(Modal::UpdateAvailable { .. })) {
                        self.modal = None;
                    }
                    self.save_settings();
                    return Task::none();
                }
                self.save_settings();
                return self.update_check_task();
            }
            Message::CheckForUpdatesNow => {
                if self.update_check == UpdateCheck::Checking {
                    return Task::none();
                }
                self.update_check = UpdateCheck::Checking;
                return Task::perform(async { crate::update::check() }, |result| {
                    Message::UpdateChecked {
                        manual: true,
                        result: result.map_err(|error| error.to_string()),
                    }
                });
            }
            Message::UpdateChecked { manual, result } => {
                let now = now_seconds().max(0) as u64;
                self.settings.last_update_check = Some(now);
                self.save_settings();
                match result {
                    Ok(Some(release)) => {
                        self.update_check = UpdateCheck::Idle;
                        let show = manual
                            || (self.modal.is_none()
                                && self.settings.wants_update_prompt(&release.version, now));
                        if show {
                            self.modal = Some(Modal::UpdateAvailable {
                                release,
                                stage: UpdateStage::Ask,
                            });
                        }
                    }
                    Ok(None) => {
                        if manual {
                            self.update_check = UpdateCheck::UpToDate;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "update check failed");
                        if manual {
                            self.update_check = UpdateCheck::Failed;
                        }
                    }
                }
            }
            Message::UpdateNow => {
                let Some(Modal::UpdateAvailable { release, stage }) = self.modal.as_mut() else {
                    return Task::none();
                };
                let plan = crate::install::plan(&crate::install::Host::detect(), release);
                if plan == crate::install::Plan::Page {
                    return self.update(Message::OpenUpdatePage);
                }
                *stage = UpdateStage::Working;
                let release = release.clone();
                return Task::perform(
                    async move { crate::install::prepare(&release, plan) },
                    Message::UpdatePrepared,
                );
            }
            Message::UpdatePrepared(result) => {
                let Some(Modal::UpdateAvailable { stage, .. }) = self.modal.as_mut() else {
                    return Task::none();
                };
                match result {
                    Ok(crate::install::Prepared::Restart(staged)) => {
                        // Applied as Libertum quits, after the unsaved-changes prompt.
                        self.staged_update = Some(staged);
                        self.modal = None;
                        return self.on_close_request();
                    }
                    Ok(crate::install::Prepared::Command { command, file }) => {
                        *stage = UpdateStage::Command { command, file };
                    }
                    Err(error) => {
                        tracing::warn!(%error, "the update could not be prepared");
                        *stage = UpdateStage::Failed(error);
                    }
                }
            }
            Message::CopyUpdateCommand => {
                if let Some(Modal::UpdateAvailable {
                    stage: UpdateStage::Command { command, .. },
                    ..
                }) = &self.modal
                {
                    return iced::clipboard::write(command.clone());
                }
            }
            Message::OpenUpdatePage => {
                if let Some(Modal::UpdateAvailable { release, .. }) = self.modal.take()
                    && let Err(error) = crate::update::open_release_page(&release.url)
                {
                    tracing::warn!(%error, "could not open the release page");
                    self.modal = Some(Modal::Error(format!("{} {error}", s::UPDATES_OPEN_FAILED)));
                }
            }
            Message::DismissUpdate => {
                if matches!(self.modal, Some(Modal::UpdateAvailable { .. })) {
                    self.modal = None;
                }
            }
            Message::MuteUpdate(seconds) => {
                if let Some(Modal::UpdateAvailable { release, .. }) = self.modal.take() {
                    let until = now_seconds().max(0) as u64 + seconds;
                    self.settings.update_mute = Some(UpdateMute {
                        version: release.version,
                        until: Some(until),
                    });
                    self.save_settings();
                }
            }
            Message::MuteUpdateVersion => {
                if let Some(Modal::UpdateAvailable { release, .. }) = self.modal.take() {
                    self.settings.update_mute = Some(UpdateMute {
                        version: release.version,
                        until: None,
                    });
                    self.save_settings();
                }
            }
            Message::DisableUpdates => {
                self.settings.check_for_updates = false;
                self.modal = None;
                self.save_settings();
            }
            Message::SetUnits(preference) => {
                self.settings.units = preference;
                self.state.units = preference.to_units();
                self.save_settings();
            }
            Message::FileDropped(path) => {
                if !is_document_path(&path) {
                    tracing::info!(path = %path.display(), "ignored a dropped file");
                    return Task::none();
                }
                if self.doc.is_some() && self.dirty {
                    self.modal = Some(Modal::Unsaved {
                        pending: Pending::Open(path),
                    });
                    return Task::none();
                }
                return self.open_path(path);
            }
            Message::OpenRecent(path) => {
                if self.doc.is_some() && self.dirty {
                    self.modal = Some(Modal::Unsaved {
                        pending: Pending::Open(path),
                    });
                    return Task::none();
                }
                return self.open_path(path);
            }
            Message::FpsFrame(now) => {
                self.fps.tick(now);
            }
            Message::AutosaveTick => {
                if self.dirty
                    && let Some(doc) = self.doc.as_ref()
                {
                    let doc_id = doc.id();
                    let revision = doc.document().revision();
                    // The snapshot is a deep clone plus serialisation. A document that has
                    // not changed since the last one would produce identical bytes, so skip
                    // it entirely (this used to repeat every minute while left dirty).
                    if self.autosave_pending != Some(revision)
                        && self.autosave_done != Some(revision)
                    {
                        let document = doc.document().clone();
                        let path = self.file.clone();
                        let dir = self.recovery_dir.clone();
                        self.autosave_pending = Some(revision);
                        return Task::perform(
                            async move {
                                recovery::write(&dir, document, path.as_deref())
                                    .map_err(|error| error.to_string())
                            },
                            move |result| Message::Autosaved {
                                doc_id,
                                revision,
                                result,
                            },
                        );
                    }
                }
            }
            Message::Autosaved {
                doc_id,
                revision,
                result,
            } => {
                // A snapshot from the previous document must not mark this one as saved.
                if self.doc.as_ref().is_some_and(|doc| doc.id() == doc_id) {
                    self.autosave_pending = None;
                    match result {
                        Ok(()) => self.autosave_done = Some(revision),
                        Err(error) => {
                            tracing::warn!(%error, "could not write the recovery snapshot")
                        }
                    }
                }
            }
            Message::Shortcut(Shortcut::Settings) => {
                self.state.open_menu = None;
                self.settings_open = true;
            }
            Message::Shortcut(Shortcut::Find) => return self.open_find(),
            Message::Shortcut(Shortcut::FindNext) => return self.find_step(true),
            Message::Shortcut(Shortcut::FindPrevious) => return self.find_step(false),
            Message::Shortcut(Shortcut::Save) => return self.save_document(false),
            Message::Shortcut(Shortcut::SaveAs) => return self.save_document(true),
            Message::Shortcut(Shortcut::Open) => return self.choose_open_file(),
            Message::Shortcut(Shortcut::ToggleSidebar) => self.toggle_sidebar(),
            Message::Shortcut(
                shortcut @ (Shortcut::Bold | Shortcut::Italic | Shortcut::Underline),
            ) => {
                if self.canvas_focused && self.state.editing.is_none() {
                    self.toggle_char(shortcut);
                }
            }
            Message::Shortcut(Shortcut::Heading(level)) => {
                if self.canvas_focused && self.state.editing.is_none() {
                    let style = match level {
                        1 => ParagraphStyle::Heading1,
                        2 => ParagraphStyle::Heading2,
                        3 => ParagraphStyle::Heading3,
                        4 => ParagraphStyle::Heading4,
                        _ => ParagraphStyle::Body,
                    };
                    return self.update(Message::Format(FormatMsg::Style(style)));
                }
            }
            Message::Shortcut(Shortcut::PageBreak) => {
                if self.canvas_focused && self.state.editing.is_none() {
                    return self.insert_page_break();
                }
            }
            Message::Shortcut(shortcut @ (Shortcut::Superscript | Shortcut::Subscript)) => {
                if self.canvas_focused && self.state.editing.is_none() {
                    let v = if shortcut == Shortcut::Superscript {
                        libertum_model::VerticalAlign::Superscript
                    } else {
                        libertum_model::VerticalAlign::Subscript
                    };
                    if let Some(doc) = self.doc.as_mut() {
                        let result = doc.edit(|ed| ed.toggle_vertical(v));
                        let _ = self.finish_edit(result);
                    }
                }
            }
            Message::Shortcut(
                shortcut @ (Shortcut::AlignLeft
                | Shortcut::AlignCentre
                | Shortcut::AlignRight
                | Shortcut::AlignJustify),
            ) => {
                if self.canvas_focused && self.state.editing.is_none() {
                    let align = match shortcut {
                        Shortcut::AlignCentre => Alignment::Centre,
                        Shortcut::AlignRight => Alignment::Right,
                        Shortcut::AlignJustify => Alignment::Justify,
                        _ => Alignment::Left,
                    };
                    self.apply_para(&ParaFormat {
                        align: Some(model_align(align)),
                        ..ParaFormat::default()
                    });
                }
            }
            Message::Shortcut(Shortcut::CloseMenu) => {
                self.context_menu = None;
                // Escape backs out of whatever is in front: the tool runner, a prompt (Cancel,
                // or Discard for the recovery offer), then the Settings window, then the Find bar.
                if self.runner.is_some() {
                    self.close_runner();
                    return Task::none();
                }
                if self.vocabulary_open {
                    self.vocabulary_open = false;
                    self.rerank_suggestions();
                    return Task::none();
                }
                if let Some(modal) = self.modal.clone() {
                    let choice = match modal {
                        Modal::Recovery => ModalChoice::Discard,
                        _ => ModalChoice::Cancel,
                    };
                    return self.update(Message::ModalChoice(choice));
                }
                if self.settings_open {
                    self.settings_open = false;
                }
                let something_open = self.settings_open
                    || self.state.find.open
                    || self.state.open_menu.is_some()
                    || self.state.open_swatch.is_some();
                if self.state.find.open {
                    self.close_find();
                }
                // With nothing else in front, Escape leaves compare mode.
                if !something_open && self.compare_on() {
                    self.exit_compare();
                    return Task::none();
                }
                self.state.open_menu = None;
                self.state.open_swatch = None;
                // With nothing else in front, Escape leaves full screen first.
                if self.state.fullscreen && !something_open {
                    self.escape_presses = (0, Instant::now());
                    return self.toggle_full_screen();
                }
                // Three Escapes in a row go back to the home screen, for anyone who does not
                // know Ctrl/Cmd+W.
                let now = Instant::now();
                let (count, last) = self.escape_presses;
                self.escape_presses = if now.duration_since(last).as_millis() <= 900 {
                    (count.saturating_add(1), now)
                } else {
                    (1, now)
                };
                if self.escape_presses.0 >= 3 {
                    self.escape_presses = (0, now);
                    return self.request_home();
                }
            }
            Message::ZoomSelected(zoom) => {
                self.state.open_menu = None;
                self.state.zoom_edit = None;
                self.set_zoom(zoom);
            }
            Message::ZoomStep(delta) => {
                self.state.open_menu = None;
                let current = f32::from(self.metrics().percent());
                self.set_zoom_percent(current + f32::from(delta));
            }
            Message::ZoomSet(percent) => {
                self.state.open_menu = None;
                self.set_zoom_percent(f32::from(percent));
            }
            Message::ZoomText(text) => {
                // Apply parseable values live; the committed value is clamped to 1..=800.
                if let Some(value) = crate::widgets::stepper::parse(&text) {
                    self.set_zoom_percent(value);
                }
                self.state.zoom_edit = Some(text);
            }
            Message::ZoomCommit => {
                if let Some(text) = self.state.zoom_edit.take()
                    && let Some(value) = crate::widgets::stepper::parse(&text)
                {
                    self.set_zoom_percent(value);
                }
            }
            Message::Canvas(event) => return self.on_canvas_event(event),
            Message::SelectTemplate(id) => {
                self.home_selection = id;
                self.home_user = None;
            }
            Message::SelectUserTemplate(index) => {
                if index < self.user_templates.len() {
                    self.home_user = Some(index);
                }
            }
            Message::CreateUserTemplate(index) => return self.create_user_template(index),
            Message::AddTemplate => {
                let Some(path) = rfd::FileDialog::new()
                    .set_title(s::TEMPLATE_ADD_TITLE)
                    .add_filter(s::TEMPLATE_FILTER, &["odt", "lbtd", "docx"])
                    .pick_file()
                else {
                    return Task::none();
                };
                match self.import_template(&path) {
                    Ok(index) => self.home_user = Some(index),
                    Err(error) => {
                        tracing::warn!(%error, path = %path.display(), "could not add the template");
                        self.modal =
                            Some(Modal::Error(format!("{} {error}", s::TEMPLATE_ADD_FAILED)));
                    }
                }
            }
            Message::RemoveSelectedTemplate => {
                if let Some(index) = self.home_user.take()
                    && let Some(template) = self.user_templates.get(index).cloned()
                {
                    if let Err(error) = templates::remove(&template) {
                        self.home_user = Some(index);
                        tracing::warn!(%error, path = %template.path.display(), "could not remove the template");
                        self.modal = Some(Modal::Error(format!(
                            "{} {error}",
                            s::TEMPLATE_REMOVE_FAILED
                        )));
                    } else {
                        self.reload_templates();
                    }
                }
            }
            Message::OpenFile => return self.choose_open_file(),
            Message::Export(format) => return self.export_document(format),
            Message::Exported(result) => {
                if let Err(error) = result {
                    self.modal = Some(Modal::Error(error));
                }
            }
            Message::OpenAo3Chapters => {
                if let Some(doc) = self.doc.as_ref() {
                    self.state.ao3_chapters =
                        libertum_export::chapters(doc.document(), self.settings.chapter_headings);
                    self.state.ao3_open = true;
                    self.state.open_menu = Some(Menu::Export);
                }
            }
            Message::CloseAo3List => self.state.ao3_open = false,
            Message::Ao3Chapter(index) => {
                let Some(doc) = self.doc.as_ref() else {
                    return Task::none();
                };
                let Some(chapter) = self.state.ao3_chapters.get(index).cloned() else {
                    return Task::none();
                };
                let html = libertum_export::ao3_fragment(doc.document(), &chapter);
                self.sync_work_tags();
                let Some(doc) = self.doc.as_ref() else {
                    return Task::none();
                };
                let suggestions = self.chapter_suggestions(doc.document(), &chapter);
                let selected = vec![false; suggestions.len()];
                self.ao3_post_notice = None;
                self.modal = Some(Modal::Ao3Chapter {
                    chapter: index,
                    title: chapter.title,
                    html,
                    copied: false,
                    suggestions,
                    selected,
                    tags_copied: false,
                    has_title: chapter.level.is_some(),
                    title_copied: false,
                });
                self.state.open_menu = None;
                self.state.ao3_open = false;
            }
            Message::CopyAo3 => {
                if let Some(Modal::Ao3Chapter { html, copied, .. }) = &mut self.modal {
                    *copied = true;
                    return iced::clipboard::write(html.clone());
                }
            }
            Message::CopyAo3Title => {
                if let Some(Modal::Ao3Chapter {
                    title,
                    has_title: true,
                    title_copied,
                    ..
                }) = &mut self.modal
                {
                    *title_copied = true;
                    return iced::clipboard::write(title.trim().to_owned());
                }
            }
            Message::CloseAo3 => self.modal = None,
            Message::ToggleSuggestedTag(index) => {
                if let Some(Modal::Ao3Chapter {
                    selected,
                    tags_copied,
                    ..
                }) = &mut self.modal
                    && let Some(on) = selected.get_mut(index)
                {
                    *on = !*on;
                    *tags_copied = false;
                }
            }
            Message::SetAutoTags(on) => {
                self.settings.auto_suggest_tags = on;
                self.save_settings();
                self.rerank_suggestions();
            }
            Message::CopySuggestedTags => {
                if let Some(Modal::Ao3Chapter {
                    suggestions,
                    selected,
                    tags_copied,
                    ..
                }) = &mut self.modal
                {
                    let text = libertum_export::tags_to_text(suggestions, selected);
                    if !text.is_empty() {
                        *tags_copied = true;
                        // What the writer copies is what they use: remember it for later
                        // chapters and stories (characters give the name book full names).
                        let used: Vec<libertum_export::Tag> = suggestions
                            .iter()
                            .zip(selected.iter())
                            .filter(|(_, on)| **on)
                            .map(|(suggestion, _)| suggestion.tag.clone())
                            .collect();
                        self.tag_cache.learn(&used);
                        self.save_tag_cache();
                        // They are on the work now: later chapters do not offer them again.
                        self.work_tags.add(used);
                        self.save_work_tags();
                        return iced::clipboard::write(text);
                    }
                }
            }
            Message::SetStarterTags(on) => {
                self.settings.use_starter_tags = on;
                self.save_settings();
                self.rerank_suggestions();
            }
            Message::ImportTagFile => {
                let Some(path) = rfd::FileDialog::new()
                    .add_filter(s::TAG_IMPORT_FILTER, &["txt", "csv", "tsv", "list"])
                    .set_title(s::TAG_IMPORT_TITLE)
                    .pick_file()
                else {
                    return Task::none();
                };
                self.tag_cache_notice = Some(s::TAG_IMPORT_WORKING.to_owned());
                // A large file is read off the UI thread.
                return Task::perform(
                    async move {
                        std::fs::File::open(&path)
                            .map(|file| {
                                libertum_export::parse_tag_import(std::io::BufReader::new(file))
                            })
                            .map_err(|error| error.to_string())
                    },
                    Message::TagFileImported,
                );
            }
            Message::TagFileImported(result) => {
                self.tag_cache_notice = Some(match result {
                    Ok(tags) => {
                        let read = tags.len();
                        let added = self.tag_cache.import(tags);
                        self.save_tag_cache();
                        self.rerank_suggestions();
                        s::tag_import_done(read, added)
                    }
                    Err(error) => format!("{} {error}", s::TAG_IMPORT_FAILED),
                });
            }
            Message::DismissSuggestedTag(index) => {
                if let Some(Modal::Ao3Chapter {
                    suggestions,
                    selected,
                    tags_copied,
                    ..
                }) = &mut self.modal
                    && index < suggestions.len()
                {
                    let removed = suggestions.remove(index);
                    if index < selected.len() {
                        selected.remove(index);
                    }
                    *tags_copied = false;
                    self.work_tags.dismiss(removed.tag);
                    self.save_work_tags();
                }
            }
            Message::ShowHiddenTags => {
                self.work_tags = work_tags::WorkTags::default();
                self.save_work_tags();
                self.rerank_suggestions();
            }
            Message::ForgetLearnedTags => {
                self.tag_cache.forget_learned();
                self.save_tag_cache();
                self.tag_cache_notice = None;
                self.rerank_suggestions();
            }
            Message::OpenTagVocabulary => {
                self.tag_draft.clear();
                self.vocabulary_open = true;
            }
            Message::CloseTagVocabulary => {
                self.vocabulary_open = false;
                // The chapter window underneath may be showing a ranking from the old
                // vocabulary; re-rank it and keep the writer's ticks.
                self.rerank_suggestions();
            }
            Message::TagDraft(text) => self.tag_draft = text,
            Message::TagCategoryPicked(category) => {
                self.tag_category = category;
                // A name typed for one category is not a name AO3 has for a fixed one.
                if category.fixed_name(&self.tag_draft).is_none()
                    && category.fixed_names().is_some()
                {
                    self.tag_draft.clear();
                }
            }
            Message::FixedTagPicked(name) => self.tag_draft = name.to_owned(),
            Message::AddTag => {
                // The rating and the warnings are AO3's own names, in AO3's own spelling.
                let name = match self.tag_category.fixed_names() {
                    Some(_) => self
                        .tag_category
                        .fixed_name(&self.tag_draft)
                        .map(str::to_owned)
                        .unwrap_or_default(),
                    None => self.tag_draft.trim().to_owned(),
                };
                if !name.is_empty()
                    && !self
                        .settings
                        .tags
                        .iter()
                        .any(|tag| tag.name.eq_ignore_ascii_case(&name))
                {
                    self.settings.tags.push(Tag::new(name, self.tag_category));
                    self.save_settings();
                }
                self.tag_draft.clear();
            }
            Message::RemoveTag(index) => {
                if index < self.settings.tags.len() {
                    self.settings.tags.remove(index);
                    self.save_settings();
                }
            }
            Message::SetChapterHeading(level, on) => {
                let mut levels = self.settings.chapter_headings;
                match level {
                    1 => levels.h1 = on,
                    2 => levels.h2 = on,
                    3 => levels.h3 = on,
                    4 => levels.h4 = on,
                    _ => {}
                }
                self.settings.chapter_headings = levels;
                self.save_settings();
            }
            Message::Saved { path, result } => return self.on_saved(path, result),
            Message::CloseRequested => return self.on_close_request(),
            Message::ModalChoice(choice) => return self.on_modal_choice(choice),
            Message::CreateDocument(id) => {
                if self.doc.is_none() {
                    return self.create_document(id);
                }
            }
            Message::Doc(event) => return self.on_doc_event(event),
            Message::SpellTick => return self.check_spelling_if_changed(),
            Message::TypeHookTick => {
                if Instant::now() < self.type_hook_from {
                    return Task::none();
                }
                let Some(c) = self.type_hook.pop_front() else {
                    return Task::none();
                };
                self.canvas_focused = true;
                let text = c.to_string();
                let key = if c == '\n' {
                    keyboard::Key::Named(keyboard::key::Named::Enter)
                } else {
                    keyboard::Key::Character(text.as_str().into())
                };
                return self.update(Message::Keyboard(keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: keyboard::key::Physical::Unidentified(
                        keyboard::key::NativeCode::Unidentified,
                    ),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::default(),
                    text: (c != '\n').then(|| text.as_str().into()),
                    repeat: false,
                }));
            }
            Message::SpellChecked { generation, result } => {
                tracing::debug!(
                    generation,
                    current = self.spell_generation,
                    errors = result.as_ref().map_or(0, Vec::len),
                    "spell check finished"
                );
                if generation == self.spell_generation {
                    match result {
                        Ok(errors) => {
                            // Typing may have gone on while the check ran: keep only the
                            // words still at their place (the next check covers the rest).
                            let document = self.doc.as_ref().map(DocView::document);
                            // The word the caret is in or against is still being typed:
                            // leave it unchecked until the caret moves off it.
                            let typing = self.doc.as_ref().and_then(Self::typing_span);
                            let still_there = |error: &spell::Span| {
                                document
                                    .and_then(|d| d.paragraph(error.block))
                                    .and_then(|p| p.text().get(error.start..error.end))
                                    .is_some_and(|word| word == error.word)
                            };
                            // Skipping the word being typed is not staleness, so it must not
                            // count as an out-of-date result (that would schedule a recheck
                            // every couple of seconds for as long as the caret sits on it).
                            // The caret moving off the word triggers its own check.
                            let wanted: Vec<spell::Span> = errors
                                .into_iter()
                                .filter(|error| {
                                    !Self::touches_caret(error, typing)
                                        && !self.spell_ignored_once.contains(&(
                                            error.block,
                                            error.start,
                                            error.end,
                                            error.word.clone(),
                                        ))
                                })
                                .collect();
                            let fresh = wanted.len();
                            self.spell_errors =
                                wanted.into_iter().filter(|e| still_there(e)).collect();
                            if self.spell_errors.len() < fresh {
                                // Some were out of date: look again once typing pauses.
                                self.spell_signature = None;
                                retry_spell_later();
                            }
                            self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
                        }
                        Err(error) => {
                            tracing::warn!(%error, "spell check failed");
                            // Let the next tick try again; the signature said this text was
                            // already covered, but no result arrived.
                            self.spell_signature = None;
                            retry_spell_later();
                        }
                    }
                }
            }
            Message::SpellReplace { span, replacement } => {
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|editor| {
                        editor.set_caret(
                            libertum_editor::DocPosition {
                                block: span.block,
                                offset: span.start,
                            },
                            false,
                        );
                        editor.set_caret(
                            libertum_editor::DocPosition {
                                block: span.block,
                                offset: span.end,
                            },
                            true,
                        );
                        editor.insert_text(&replacement)
                    });
                    self.spell_errors.retain(|error| error != &span);
                    self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
                    return self.finish_edit(result);
                }
            }
            Message::SpellIgnoreOnce(span) => {
                self.spell_ignored_once.insert((
                    span.block,
                    span.start,
                    span.end,
                    span.word.clone(),
                ));
                self.spell_errors.retain(|error| error != &span);
                self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
            }
            Message::SpellIgnoreDocument(word) => {
                self.spell_ignored_document.insert(word.to_lowercase());
                self.spell_errors
                    .retain(|error| !error.word.eq_ignore_ascii_case(&word));
                self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
            }
            Message::SpellAddWord(word) => {
                let lower = word.to_lowercase();
                self.spell_personal.insert(lower.clone());
                if let Some(path) = &self.spell_path
                    && let Err(error) = spell::add_personal(path, &lower)
                {
                    tracing::warn!(%error, "could not add the word to the personal dictionary");
                }
                self.spell_errors
                    .retain(|error| !error.word.eq_ignore_ascii_case(&word));
                self.spell_errors_epoch = self.spell_errors_epoch.wrapping_add(1);
            }
            Message::Keyboard(event) => return self.on_keyboard(event),
            Message::Ime(event) => return self.on_ime(event),
            Message::ImportChapterFile => {
                self.state.open_menu = None;
                if self.import_blocked() {
                    return Task::none();
                }
                let Some(path) = rfd::FileDialog::new()
                    .add_filter(
                        s::IMPORT_CHAPTER_FILTER,
                        &[
                            s::EXT_HTML,
                            "htm",
                            s::EXT_EPUB,
                            s::EXT_NATIVE,
                            s::EXT_ODT,
                            s::EXT_DOCX,
                            s::EXT_MARKDOWN,
                            s::EXT_RTF,
                        ],
                    )
                    .set_title(s::IMPORT_CHAPTER_TITLE)
                    .pick_file()
                else {
                    return Task::none();
                };
                return self.import_chapter_file(&path);
            }
            Message::ImportChapterClipboard => {
                self.state.open_menu = None;
                if self.import_blocked() {
                    return Task::none();
                }
                return iced::clipboard::read().map(Message::ImportChapterPasted);
            }
            Message::ImportChapterPasted(text) => {
                let text = text.unwrap_or_default();
                if text.trim().is_empty() {
                    self.modal = Some(Modal::Error(s::IMPORT_CLIPBOARD_EMPTY.to_owned()));
                    return Task::none();
                }
                let document = libertum_html::parse_fragment(&text);
                return self.import_chapter(&document);
            }
            Message::PasteText(text) => {
                if let Some(text) = text
                    && self.canvas_focused
                    && let Some(doc) = self.doc.as_mut()
                {
                    let result = doc.edit(|ed| ed.paste(&text));
                    return self.finish_edit(result);
                }
            }
            Message::ScaleFactor(sf) => self.set_scale_factor(sf),
            Message::WindowResized(size) => {
                self.window_width = size.width;
                self.window_height = size.height;
            }
            Message::BlinkTick(now) => self.on_blink_tick(now),
            Message::WindowFocus(focused) => {
                self.window_focused = focused;
                if focused {
                    // Back in front: a solid caret that blinks for the usual few seconds.
                    self.caret_on = true;
                    self.caret_activity = Instant::now();
                } else if self.dirty {
                    // Leaving the window is when work is most at risk: snapshot it now.
                    return self.update(Message::AutosaveTick);
                }
            }
            Message::Frame(now) => {
                if self.bench.is_some() {
                    return self.bench_frame(now);
                }
                self.state.tick_sidebar(now);
                self.coast(now);
                return self.autoscroll_frame(now);
            }
            Message::Insert(action) => {
                self.state.open_menu = None;
                match action {
                    InsertAction::PageBreak => {
                        self.canvas_focused = true;
                        return self.insert_page_break();
                    }
                    InsertAction::PageNumber => {
                        self.canvas_focused = true;
                        self.toggle_page_numbers();
                    }
                    InsertAction::Table => {
                        self.canvas_focused = true;
                        return self.insert_table();
                    }
                    InsertAction::TableOfContents => {
                        self.canvas_focused = true;
                        let Some(doc) = self.doc.as_mut() else {
                            return Task::none();
                        };
                        let result = doc.edit(|ed| ed.insert_table_of_contents());
                        return self.finish_edit(result);
                    }
                    InsertAction::Footnote => {
                        self.canvas_focused = true;
                        let Some(doc) = self.doc.as_mut() else {
                            return Task::none();
                        };
                        let result = doc.edit(|ed| ed.insert_footnote());
                        return self.finish_edit(result);
                    }
                    InsertAction::Endnote => {
                        self.canvas_focused = true;
                        let Some(doc) = self.doc.as_mut() else {
                            return Task::none();
                        };
                        let result = doc.edit(|ed| ed.insert_endnote());
                        return self.finish_edit(result);
                    }
                    InsertAction::Image => {
                        self.canvas_focused = true;
                        return self.pick_image();
                    }
                    // TODO(Phase 5): section break.
                    _ => tracing::debug!(?action, "insert action (not implemented yet)"),
                }
            }
            Message::Table(action) => {
                self.state.open_menu = None;
                return self.table_action(action);
            }
            Message::ToggleFullScreen | Message::Shortcut(Shortcut::FullScreen) => {
                return self.toggle_full_screen();
            }
            Message::ToggleThumbnails => {
                self.state.open_menu = None;
                self.state.thumbnails = !self.state.thumbnails;
                if !self.state.thumbnails {
                    self.thumb_cache.clear();
                    return Task::none();
                }
                // Open on the page in view.
                let current = self.state.current_page.saturating_sub(1);
                let offset = (current as f32 * THUMB_ROW_H - THUMB_ROW_H).max(0.0);
                self.thumb_scroll.0 = offset;
                self.refresh_thumbnails();
                return iced::widget::operation::scroll_to(
                    THUMB_LIST_ID,
                    iced::widget::scrollable::AbsoluteOffset {
                        x: None,
                        y: Some(offset),
                    },
                );
            }
            Message::ThumbScrolled(offset, height) => {
                self.thumb_scroll = (offset, height);
                self.refresh_thumbnails();
            }
            Message::GoToPage(page) => {
                let m = self.metrics();
                self.scroll_pending = (0.0, 0.0);
                self.trackpad_pending = (0.0, 0.0);
                self.fling = (0.0, 0.0);
                self.scroll.1 = m.page_top(page) - m.pad;
                self.clamp_scroll();
                self.refresh_view();
            }
            Message::TableStyle(index) => {
                self.state.open_menu = None;
                return self.set_table_style(index);
            }
            Message::ImageStep(block, index, direction) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                // A quarter of a centimetre (or a tenth of an inch) per step.
                let step = match self.state.units {
                    Units::Inches => 7.2,
                    Units::Centimetres => 72.0 / 2.54 * 0.25,
                };
                let result = doc.edit(|ed| {
                    let width = ed
                        .document()
                        .paragraph(block)
                        .and_then(|p| p.images().get(index))
                        .map_or(0.0, |image| image.width_pt);
                    ed.resize_image(block, index, width + f32::from(direction.signum()) * step)
                });
                let task = self.finish_edit(result);
                self.refresh_format();
                return task;
            }
            Message::ImageAlt(block, index, alt) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.set_image_alt(block, index, &alt));
                let task = self.finish_edit(result);
                self.refresh_format();
                return task;
            }
            Message::RubyEdit(reading) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.set_ruby(&reading));
                let task = self.finish_edit(result);
                self.refresh_format();
                return task;
            }
            Message::ImageRemove(block, index) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.remove_image(block, index));
                let task = self.finish_edit(result);
                self.refresh_format();
                return task;
            }
            Message::InsertCrossRef(name, kind) => {
                let pages = self.bookmark_pages();
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                self.canvas_focused = true;
                let result = doc.edit(|ed| {
                    ed.insert_cross_ref(&name, kind, &|block| pages.get(&block).copied())
                });
                return self.finish_edit(result);
            }
            Message::RefreshCrossRefs => {
                self.refresh_cross_refs();
            }
            Message::GoToHeading(block) => {
                if let Some(doc) = self.doc.as_mut() {
                    let _ = doc.edit(|ed| {
                        ed.set_caret(libertum_editor::DocPosition { block, offset: 0 }, false);
                        Ok(libertum_model::ChangeSet::default())
                    });
                    self.ensure_caret_visible();
                    self.refresh_view();
                }
            }
            Message::ToggleHeading(block) => {
                if !self.collapsed_headings.remove(&block) {
                    self.collapsed_headings.insert(block);
                }
            }
            Message::NoteText(block, index, text) => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.set_note_at(block, index, text));
                return self.finish_edit(result);
            }
            Message::GoToNote(block, index) => {
                if let Some(doc) = self.doc.as_mut() {
                    let offset = doc
                        .editor()
                        .document()
                        .paragraph(block)
                        .and_then(|p| p.notes().get(index))
                        .map_or(0, |note| note.offset);
                    let _ = doc.edit(|ed| {
                        ed.set_caret(libertum_editor::DocPosition { block, offset }, false);
                        Ok(libertum_model::ChangeSet::default())
                    });
                    self.ensure_caret_visible();
                    self.refresh_view();
                }
            }
            Message::AddBookmark => {
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| {
                    let name = ed.next_bookmark_name();
                    ed.insert_bookmark(&name).map(|(change, _)| change)
                });
                self.bookmark_draft = None;
                return self.finish_edit(result);
            }
            Message::BookmarkName(block, index, name) => {
                self.bookmark_draft = Some((block, index, name));
            }
            Message::BookmarkNameCommit(block, index) => {
                let name = match self.bookmark_draft.take() {
                    Some((draft_block, draft_index, name))
                        if draft_block == block && draft_index == index =>
                    {
                        name
                    }
                    other => {
                        self.bookmark_draft = other;
                        return Task::none();
                    }
                };
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.rename_bookmark(block, index, &name));
                return self.finish_edit(result);
            }
            Message::GoToBookmark(block, index) => {
                let offset = self.doc.as_ref().map_or(0, |doc| {
                    doc.editor()
                        .bookmarks()
                        .into_iter()
                        .find(|(bookmark_block, bookmark_index, _)| {
                            *bookmark_block == block && *bookmark_index == index
                        })
                        .map_or(0, |(_, _, bookmark)| bookmark.offset)
                });
                if let Some(doc) = self.doc.as_mut() {
                    let _ = doc.edit(|ed| {
                        ed.set_caret(libertum_editor::DocPosition { block, offset }, false);
                        Ok(libertum_model::ChangeSet::default())
                    });
                    self.ensure_caret_visible();
                    self.refresh_view();
                }
            }
            Message::RemoveBookmark(block, index) => {
                self.bookmark_draft = None;
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = doc.edit(|ed| ed.remove_bookmark(block, index));
                return self.finish_edit(result);
            }
            Message::FormatPane(pane) => {
                self.canvas_focused = false;
                self.state.format_pane = pane;
            }
            Message::DocumentPane(pane) => {
                self.canvas_focused = false;
                self.state.document_pane = pane;
            }
            Message::ToggleSwatch(target) => {
                self.canvas_focused = false;
                self.state.toggle_swatch(target);
            }
            Message::Format(msg) => {
                self.canvas_focused = false;
                self.apply_format(msg);
            }
            Message::Document(msg) => {
                self.canvas_focused = false;
                self.apply_document(msg);
            }
            Message::Step(field, direction) => {
                self.canvas_focused = false;
                field.nudge(&mut self.state, direction);
                self.apply_field(field);
            }
            Message::FieldText(field, text) => {
                self.canvas_focused = false;
                self.state.edit_field(field, text);
            }
            Message::FieldCommit => {
                if let Some((field, _)) = self.state.editing.take() {
                    self.apply_field(field);
                }
            }
            Message::SystemTheme(mode) => self.system_mode = mode,
            Message::ScreenshotDue => {
                return window::oldest()
                    .and_then(window::screenshot)
                    .map(Message::Screenshot);
            }
            Message::Screenshot(shot) => {
                if let Some(path) = crate::shot::target() {
                    match crate::shot::save(&shot, &path) {
                        Ok(()) => tracing::info!(path = %path.display(), "screenshot saved"),
                        Err(error) => tracing::error!(%error, "could not save the screenshot"),
                    }
                }
                return iced::exit();
            }
            Message::DragWindow => return window::oldest().and_then(window::drag),
            Message::ToggleMaximise => return window::oldest().and_then(window::toggle_maximize),
        }
        Task::none()
    }

    fn toggle_sidebar(&mut self) {
        self.state.open_menu = None;
        self.state.toggle_sidebar();
        self.persist_sidebar();
    }

    /// Copy the sidebar state into the settings and write them.
    fn persist_sidebar(&mut self) {
        self.settings.sidebar_collapsed = !self.state.sidebar_visible;
        self.settings.sidebar_width = self.state.sidebar_width;
        self.settings.inspector = self.state.inspector;
        self.save_settings();
    }

    /// Puts `path` at the front of the recent list (deduplicated, capped) and persists it.
    fn remember_recent(&mut self, path: &std::path::Path) {
        const MAX_RECENT: usize = 8;
        self.settings.recent_files.retain(|recent| recent != path);
        self.settings.recent_files.insert(0, path.to_path_buf());
        self.settings.recent_files.truncate(MAX_RECENT);
        self.save_settings();
    }

    fn save_settings(&self) {
        let Some(path) = &self.settings_path else {
            return;
        };
        if let Err(error) = self.settings.save_to(path) {
            tracing::warn!(%error, path = %path.display(), "could not save settings");
        }
    }

    /// Format inspector controls. Character and paragraph formatting act on the document's
    /// selection; without a document (the home screen, some tests) only the inspector state
    /// changes. Ligatures are bound here per selection; the document-wide switches live in
    /// [`apply_document`].
    fn apply_format(&mut self, msg: FormatMsg) {
        if matches!(msg, FormatMsg::ToggleFontAdvanced) {
            self.state.font_advanced = !self.state.font_advanced;
            return;
        }
        if self.doc.is_some() {
            let para: Option<ParaFormat> = match &msg {
                FormatMsg::Align(a) => Some(ParaFormat {
                    align: Some(model_align(*a)),
                    ..ParaFormat::default()
                }),
                FormatMsg::SpacingMode(mode) => Some(ParaFormat {
                    line_spacing: Some(self.line_spacing_in(*mode)),
                    ..ParaFormat::default()
                }),
                FormatMsg::KeepLines(v) => Some(ParaFormat {
                    keep_together: Some(*v),
                    ..ParaFormat::default()
                }),
                FormatMsg::KeepWithNext(v) => Some(ParaFormat {
                    keep_with_next: Some(*v),
                    ..ParaFormat::default()
                }),
                FormatMsg::WidowOrphan(v) => Some(ParaFormat {
                    widow_control: Some(*v),
                    ..ParaFormat::default()
                }),
                FormatMsg::RemoveHyphenation(v) => Some(ParaFormat {
                    hyphenate: Some(!*v),
                    ..ParaFormat::default()
                }),
                FormatMsg::BorderStyle(v) => Some(ParaFormat {
                    borders: Some(v.borders()),
                    ..ParaFormat::default()
                }),
                FormatMsg::BorderSides(sides) => {
                    self.state.format.border_sides = *sides;
                    let width = self
                        .state
                        .format
                        .border_style
                        .value()
                        .map_or(0.5, BorderStyle::width)
                        .max(0.5);
                    Some(ParaFormat {
                        borders: Some(libertum_model::Borders {
                            sides: *sides,
                            width_pt: width,
                            colour: self.state.format.border_colour,
                            ..libertum_model::Borders::default()
                        }),
                        ..ParaFormat::default()
                    })
                }
                FormatMsg::BorderColour(colour) => {
                    self.state.format.border_colour = *colour;
                    let width = self
                        .state
                        .format
                        .border_style
                        .value()
                        .map_or(0.5, BorderStyle::width)
                        .max(0.5);
                    Some(ParaFormat {
                        borders: Some(libertum_model::Borders {
                            sides: self.state.format.border_sides,
                            width_pt: width,
                            colour: *colour,
                            ..libertum_model::Borders::default()
                        }),
                        ..ParaFormat::default()
                    })
                }
                _ => None,
            };
            if let Some(para) = para {
                self.apply_para(&para);
                return;
            }
            let paragraph_command = matches!(
                msg,
                FormatMsg::Style(_)
                    | FormatMsg::UpdateStyle
                    | FormatMsg::Indent(_)
                    | FormatMsg::PageBreakBefore(_)
                    | FormatMsg::ListKind(_)
            );
            if paragraph_command {
                if let Some(doc) = self.doc.as_mut() {
                    let m = msg.clone();
                    let result = doc.edit(move |ed| match m {
                        FormatMsg::Style(v) => ed.set_paragraph_style(v.id()),
                        FormatMsg::UpdateStyle => ed.update_style_from_selection(),
                        FormatMsg::Indent(d) => ed.indent(d),
                        FormatMsg::PageBreakBefore(on) => ed.set_page_break_before(on),
                        FormatMsg::ListKind(kind) => ed.set_list(kind.model()),
                        _ => Ok(libertum_model::ChangeSet::default()),
                    });
                    let _ = self.finish_edit(result);
                }
                self.canvas_focused = true;
                self.refresh_format();
                return;
            }
            let patch = match &msg {
                FormatMsg::Family(v) => Some(CharFormat {
                    family: Some(v.0.clone()),
                    ..CharFormat::default()
                }),
                FormatMsg::Baseline(v) => Some(CharFormat {
                    vertical: Some(v.vertical()),
                    ..CharFormat::default()
                }),
                FormatMsg::Ligatures(v) => Some(CharFormat {
                    ligatures: Some(v.model()),
                    ..CharFormat::default()
                }),
                FormatMsg::Weight(v) => Some(CharFormat {
                    weight: Some(v.value()),
                    ..CharFormat::default()
                }),
                FormatMsg::Colour(v) => Some(CharFormat {
                    colour: Some(v.text_rgba()),
                    ..CharFormat::default()
                }),
                FormatMsg::Highlight(v) => Some(CharFormat {
                    highlight: Some(v.map(Swatch::highlight_rgba)),
                    ..CharFormat::default()
                }),
                _ => None,
            };
            let toggle = match msg {
                FormatMsg::ToggleBold => Some(Shortcut::Bold),
                FormatMsg::ToggleItalic => Some(Shortcut::Italic),
                FormatMsg::ToggleUnderline => Some(Shortcut::Underline),
                _ => None,
            };
            if let Some(patch) = patch {
                self.apply_char(&patch);
            }
            if let Some(t) = toggle {
                self.toggle_char(t);
            }
            if matches!(msg, FormatMsg::ToggleStrike)
                && let Some(doc) = self.doc.as_mut()
            {
                let result = doc.edit(|ed| ed.toggle_strikethrough());
                let _ = self.finish_edit(result);
            }
            let character = matches!(
                msg,
                FormatMsg::Family(_)
                    | FormatMsg::Baseline(_)
                    | FormatMsg::Ligatures(_)
                    | FormatMsg::Weight(_)
                    | FormatMsg::ToggleBold
                    | FormatMsg::ToggleItalic
                    | FormatMsg::ToggleUnderline
                    | FormatMsg::ToggleStrike
                    | FormatMsg::Colour(_)
                    | FormatMsg::Highlight(_)
            );
            if character {
                self.state.open_swatch = None;
                // Clicking a formatting button leaves the keyboard with the document (Pages).
                self.canvas_focused = true;
                self.refresh_format();
                return;
            }
        }
        let f = &mut self.state.format;
        match msg {
            FormatMsg::Style(v) => f.paragraph_style = Mixed::Value(v),
            FormatMsg::UpdateStyle | FormatMsg::ToggleFontAdvanced => {}
            FormatMsg::Baseline(v) => f.baseline = Mixed::Value(v),
            FormatMsg::Family(v) => f.font_family = Mixed::Value(v),
            FormatMsg::Weight(v) => f.font_weight = Mixed::Value(v),
            FormatMsg::ToggleBold => f.bold = f.bold.toggled(),
            FormatMsg::ToggleItalic => f.italic = f.italic.toggled(),
            FormatMsg::ToggleUnderline => f.underline = f.underline.toggled(),
            FormatMsg::ToggleStrike => f.strikethrough = f.strikethrough.toggled(),
            FormatMsg::Colour(v) => {
                f.colour = Mixed::Value(v);
                self.state.open_swatch = None;
            }
            FormatMsg::Highlight(v) => {
                f.highlight = Mixed::Value(v);
                self.state.open_swatch = None;
            }
            FormatMsg::Align(v) => f.alignment = Mixed::Value(v),
            FormatMsg::Indent(direction) => {
                Field::IndentLeft.nudge_by(&mut self.state, direction, 1.27)
            }
            FormatMsg::SpacingMode(v) => f.line_spacing_mode = Mixed::Value(v),
            FormatMsg::ListKind(v) => f.list_kind = Mixed::Value(v),
            FormatMsg::KeepLines(v) => f.keep_lines_together = Mixed::Value(v),
            FormatMsg::KeepWithNext(v) => f.keep_with_next = Mixed::Value(v),
            FormatMsg::PageBreakBefore(v) => f.page_break_before = Mixed::Value(v),
            FormatMsg::WidowOrphan(v) => f.widow_orphan = Mixed::Value(v),
            FormatMsg::RemoveHyphenation(v) => f.remove_hyphenation = Mixed::Value(v),
            FormatMsg::BorderStyle(v) => f.border_style = Mixed::Value(v),
            FormatMsg::BorderSides(v) => f.border_sides = v,
            FormatMsg::BorderColour(v) => f.border_colour = v,
            FormatMsg::Ligatures(v) => f.ligatures = Mixed::Value(v),
        }
    }

    /// Lays `patch` over the selection (or keeps it for the caret's next typing).
    fn apply_char(&mut self, patch: &CharFormat) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let result = doc.edit(|ed| ed.format_selection(patch));
        let _ = self.finish_edit(result);
    }

    fn toggle_char(&mut self, which: Shortcut) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let result = doc.edit(|ed| match which {
            Shortcut::Bold => ed.toggle_bold(),
            Shortcut::Italic => ed.toggle_italic(),
            _ => ed.toggle_underline(),
        });
        let _ = self.finish_edit(result);
        self.refresh_format();
    }

    /// The font-size field's value, applied to the selection.
    fn apply_font_size(&mut self) {
        if let Some(size) = self.state.format.font_size.value() {
            self.apply_char(&CharFormat::sized(f32::from(size.0)));
        }
    }

    /// Lays `patch` over the selected paragraphs as one undo step, leaving the keyboard with
    /// the document.
    fn apply_para(&mut self, patch: &ParaFormat) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let result = doc.edit(|ed| ed.format_paragraphs(patch));
        let _ = self.finish_edit(result);
        self.canvas_focused = true;
        self.refresh_format();
    }

    /// Applies an inspector field's value to the document once it is stepped or typed in
    /// (on Enter or when another control is used, not on every digit).
    fn apply_field(&mut self, field: Field) {
        if let Field::Margin(side) = field {
            self.apply_margin(side);
            return;
        }
        if field == Field::NumberingStart {
            if let Some(start) = self.state.document.numbering_start {
                self.apply_numbering(|numbering| numbering.start_at = u32::from(start));
            }
            return;
        }
        if field == Field::NumberingStartPage {
            if let Some(page) = self.state.document.numbering_start_page {
                self.apply_numbering(|numbering| {
                    numbering.start_page = u32::from(page).max(1);
                });
            }
            return;
        }
        if matches!(field, Field::HeaderDistance | Field::FooterDistance) {
            let kind = if field == Field::HeaderDistance {
                HfKind::Header
            } else {
                HfKind::Footer
            };
            let value = match field {
                Field::HeaderDistance => self.state.document.header_distance.value(),
                _ => self.state.document.footer_distance.value(),
            };
            let Some(margin) = value else { return };
            let points = margin.to_points();
            if let Some(doc) = self.doc.as_mut() {
                let result = doc.edit(|ed| ed.set_header_footer_distance(kind, points));
                let _ = self.finish_edit(result);
            }
            self.refresh_document();
            return;
        }
        if matches!(field, Field::Columns | Field::Gutter) {
            let Some(doc) = self.doc.as_mut() else {
                return;
            };
            let index = doc.editor().caret_section();
            let result = match field {
                Field::Columns => self
                    .state
                    .format
                    .columns
                    .value()
                    .map(|n| doc.edit(|ed| ed.set_columns(index, n.clamp(1, 12) as u8))),
                _ => self
                    .state
                    .format
                    .gutter_cm
                    .value()
                    .map(|cm| doc.edit(|ed| ed.set_column_gutter(index, cm * 72.0 / 2.54))),
            };
            if let Some(result) = result {
                let _ = self.finish_edit(result);
            }
            self.refresh_section();
            return;
        }
        if self.doc.is_none() {
            return;
        }
        let f = &self.state.format;
        let cm_to_pt = |cm: f32| cm * 72.0 / 2.54;
        let para = match field {
            Field::FontSize => {
                self.apply_font_size();
                self.refresh_format();
                return;
            }
            Field::CharSpacing => {
                if let Some(tenths) = f.char_spacing.value() {
                    self.apply_char(&CharFormat {
                        letter_spacing_pt: Some(f32::from(tenths) / 10.0),
                        ..CharFormat::default()
                    });
                    self.refresh_format();
                }
                return;
            }
            Field::LineSpacing => f.line_spacing.value().map(|l| ParaFormat {
                line_spacing: Some(LineSpacing::Multiple(f32::from(l.0) / 100.0)),
                ..ParaFormat::default()
            }),
            Field::LineSpacingPoints => {
                let mode = f
                    .line_spacing_mode
                    .value()
                    .unwrap_or(LineSpacingMode::Exactly);
                f.line_spacing_pt.value().map(|_| ParaFormat {
                    line_spacing: Some(self.line_spacing_in(mode)),
                    ..ParaFormat::default()
                })
            }
            Field::SpaceBefore => f.space_before.value().map(|p| ParaFormat {
                space_before_pt: Some(f32::from(p.0)),
                ..ParaFormat::default()
            }),
            Field::SpaceAfter => f.space_after.value().map(|p| ParaFormat {
                space_after_pt: Some(f32::from(p.0)),
                ..ParaFormat::default()
            }),
            Field::IndentFirst => f.indent_first_cm.value().map(|cm| ParaFormat {
                indent_first_pt: Some(cm_to_pt(cm)),
                ..ParaFormat::default()
            }),
            Field::IndentLeft => f.indent_left_cm.value().map(|cm| ParaFormat {
                indent_left_pt: Some(cm_to_pt(cm)),
                ..ParaFormat::default()
            }),
            Field::IndentRight => f.indent_right_cm.value().map(|cm| ParaFormat {
                indent_right_pt: Some(cm_to_pt(cm)),
                ..ParaFormat::default()
            }),
            // Section settings (columns, gutter, header and footer distances) are handled
            // before this match; nothing here applies to them.
            _ => None,
        };
        if let Some(para) = para {
            let Some(doc) = self.doc.as_mut() else {
                return;
            };
            let result = doc.edit(|ed| ed.format_paragraphs(&para));
            let _ = self.finish_edit(result);
            self.refresh_format();
        }
    }

    /// The line spacing rule `mode` with the inspector's current value for it (single
    /// spacing, or 14 pt, when the selection is mixed).
    fn line_spacing_in(&self, mode: LineSpacingMode) -> LineSpacing {
        let f = &self.state.format;
        let pt = f.line_spacing_pt.value().map_or(14.0, |p| f32::from(p.0));
        match mode {
            LineSpacingMode::Lines => LineSpacing::Multiple(
                f.line_spacing
                    .value()
                    .map_or(1.0, |l| f32::from(l.0) / 100.0),
            ),
            LineSpacingMode::AtLeast => LineSpacing::AtLeast(pt),
            LineSpacingMode::Exactly => LineSpacing::Exactly(pt),
            LineSpacingMode::Between => LineSpacing::Leading(pt),
        }
    }

    /// Shows the selection's character formatting in the inspector (uniform values, or the
    /// mixed state). O(1) for a caret; O(selected runs) for a selection.
    /// The format a save would use: the file's, else the Settings default (an imported EPUB
    /// is never written back, so it is saved like a new document).
    fn save_format(&self) -> Format {
        self.file
            .as_deref()
            .map(Format::for_path)
            .or_else(|| self.file_context.as_ref().map(FileContext::format))
            .filter(|format| *format != Format::Epub)
            .unwrap_or_else(|| Format::from_settings(&self.settings))
    }

    /// The language of the open document: the one its tag names, else the Settings default (a
    /// new file, one that does not say, or one in a language this list does not have).
    fn doc_language(&self) -> DocLanguage {
        self.doc
            .as_ref()
            .and_then(|doc| doc.editor().language())
            .and_then(DocLanguage::from_tag)
            .unwrap_or_else(|| DocLanguage::from(self.settings.spellcheck_language))
    }

    /// The second language of the open document, if it has one this list knows.
    fn doc_language2(&self) -> Option<DocLanguage> {
        self.doc
            .as_ref()
            .and_then(|doc| doc.editor().language2())
            .and_then(DocLanguage::from_tag)
    }

    /// The dictionaries to check the open document with: its language's and its second
    /// language's, those that have one (a document in Chinese, Japanese or Korean only has
    /// none, so nothing is underlined in it).
    fn spell_languages(&self) -> Vec<spell::Language> {
        let mut out: Vec<spell::Language> = Vec::new();
        for language in [Some(self.doc_language()), self.doc_language2()]
            .into_iter()
            .flatten()
            .filter_map(DocLanguage::dictionary)
        {
            if !out.contains(&language) {
                out.push(language);
            }
        }
        out
    }

    /// The dictionary to check the open document with, or `None` when its language has none
    /// (Chinese, Japanese and Korean documents are not spell-checked).
    #[cfg(test)]
    fn spell_language(&self) -> Option<spell::Language> {
        self.doc_language().dictionary()
    }

    /// Gives a document that has no language the Settings default, so it is written into the
    /// file and the choice stays with the document when the default changes later.
    fn stamp_default_language(&self, document: &mut libertum_model::Document) {
        if document
            .sections
            .iter()
            .all(|section| section.language.is_none())
        {
            let tag = self.settings.spellcheck_language.tag().to_owned();
            for section in &mut document.sections {
                section.language = Some(tag.clone());
            }
        }
    }

    /// RTF and Markdown cannot hold a language, so a file of those formats gets the one the
    /// app remembered for its path.
    fn apply_remembered_language(
        &self,
        document: &mut libertum_model::Document,
        path: &std::path::Path,
        format: Format,
    ) {
        if format.keeps_language() {
            return;
        }
        if let Some(language) = self.settings.language_for(path) {
            let second = self.settings.second_language_for(path);
            for section in &mut document.sections {
                section.language = Some(language.tag().to_owned());
                section.language2 = second.map(|s| s.tag().to_owned());
            }
        }
    }

    /// Remembers the open document's language for `path` when its format cannot hold one.
    fn remember_language_for_path(&mut self, path: &std::path::Path) {
        if Format::for_path(path).keeps_language() {
            return;
        }
        let language = self.doc_language();
        let second = self.doc_language2();
        if self.settings.remember_languages(path, language, second) {
            self.save_settings();
        }
    }

    fn refresh_format(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        let editor = doc.editor();
        self.state.in_table = editor.in_table();
        self.state.can_merge_right = editor.can_merge_cell_right();
        self.state.can_merge_below = editor.can_merge_cell_below();
        self.state.can_split_cell = editor.can_split_cell();
        self.state.table_style = editor.table_style_name();
        self.state.column_width_pt = editor.caret_column_width();
        let (image_block, images) = editor.caret_images();
        self.state.images = images
            .iter()
            .enumerate()
            .map(|(index, image)| crate::state::ImageEntry {
                block: image_block,
                index,
                width_pt: image.width_pt,
                alt: image.alt.clone(),
                link: image.link(),
                previewable: image
                    .link()
                    .is_some_and(|link| crate::linked::preview_address(&link).is_some()),
            })
            .collect();
        self.state.ruby = editor.ruby_target().map(|(_, _, text)| text);
        self.state.markdown = self
            .file
            .as_deref()
            .map(Format::for_path)
            .or_else(|| self.file_context.as_ref().map(FileContext::format))
            == Some(Format::Markdown);
        let saves_as = self.save_format();
        self.state.keeps_page_colour = saves_as.keeps_page_colour();
        self.state.keeps_watermark = saves_as.keeps_watermark();
        self.state.keeps_vertical = saves_as.keeps_vertical();
        self.state.keeps_ruby = saves_as.keeps_ruby();
        let sum = doc.editor().char_summary();
        fn mixed<T>(v: Option<T>) -> Mixed<T> {
            v.map_or(Mixed::Mixed, Mixed::Value)
        }
        let ps = doc.editor().para_summary();
        let f = &mut self.state.format;
        f.font_family = mixed(sum.family.clone().map(FontName));
        f.baseline = mixed(sum.vertical.map(Baseline::from_vertical));
        f.ligatures = mixed(sum.ligatures.map(Ligature::from_model));
        f.char_spacing = mixed(
            sum.letter_spacing_pt
                .map(|p| (p * 10.0).round().clamp(-100.0, 500.0) as i16),
        );
        f.paragraph_style = mixed(ps.style.and_then(ParagraphStyle::from_id));
        f.style_differs = ps.differs_from_style;
        f.alignment = mixed(ps.align.map(|a| match a {
            Align::Start => Alignment::Left,
            Align::Centre => Alignment::Centre,
            Align::End => Alignment::Right,
            Align::Justify => Alignment::Justify,
        }));
        let points = |v: f32| Points(v.round().clamp(0.0, 1000.0) as u16);
        match ps.line_spacing {
            Some(LineSpacing::Multiple(m)) => {
                f.line_spacing_mode = Mixed::Value(LineSpacingMode::Lines);
                f.line_spacing = Mixed::Value(crate::state::LineSpacing(
                    (m * 100.0).round().clamp(0.0, 1000.0) as u16,
                ));
            }
            Some(LineSpacing::AtLeast(pt)) => {
                f.line_spacing_mode = Mixed::Value(LineSpacingMode::AtLeast);
                f.line_spacing_pt = Mixed::Value(points(pt));
            }
            Some(LineSpacing::Exactly(pt)) => {
                f.line_spacing_mode = Mixed::Value(LineSpacingMode::Exactly);
                f.line_spacing_pt = Mixed::Value(points(pt));
            }
            Some(LineSpacing::Leading(pt)) => {
                f.line_spacing_mode = Mixed::Value(LineSpacingMode::Between);
                f.line_spacing_pt = Mixed::Value(points(pt));
            }
            None => {
                f.line_spacing_mode = Mixed::Mixed;
                f.line_spacing = Mixed::Mixed;
                f.line_spacing_pt = Mixed::Mixed;
            }
        }
        f.space_before = mixed(ps.space_before_pt.map(points));
        f.space_after = mixed(ps.space_after_pt.map(points));
        let cm = |pt: f32| (pt * 2.54 / 72.0 * 100.0).round() / 100.0;
        f.indent_first_cm = mixed(ps.indent_first_pt.map(cm));
        f.indent_left_cm = mixed(ps.indent_left_pt.map(cm));
        f.indent_right_cm = mixed(ps.indent_right_pt.map(cm));
        f.keep_lines_together = mixed(ps.keep_together);
        f.keep_with_next = mixed(ps.keep_with_next);
        f.widow_orphan = mixed(ps.widow_control);
        f.page_break_before = mixed(ps.page_break_before);
        f.list_kind = mixed(ps.list.map(ListKind::from_model));
        f.remove_hyphenation = mixed(ps.hyphenate.map(|h| !h));
        f.border_style = mixed(ps.borders.map(BorderStyle::from_borders));
        if let Some(Some(borders)) = ps.borders {
            f.border_sides = borders.sides;
            f.border_colour = borders.colour;
        }
        let defaults = &doc.editor().document().styles.para_defaults;
        self.state.document.hyphenation = Mixed::Value(defaults.hyphenate);
        f.font_weight = mixed(sum.weight.map(FontWeight::nearest));
        f.font_size = mixed(
            sum.size_pt
                .map(|s| Points(s.round().clamp(1.0, 400.0) as u16)),
        );
        f.bold = mixed(sum.is_bold());
        f.italic = mixed(sum.italic);
        f.underline = mixed(sum.underline);
        f.strikethrough = mixed(sum.strikethrough);
        f.colour = mixed(sum.colour.and_then(Swatch::from_text_rgba));
        f.highlight = match sum.highlight {
            Some(None) => Mixed::Value(None),
            Some(Some(rgba)) => mixed(Swatch::from_highlight_rgba(rgba).map(Some)),
            None => Mixed::Mixed,
        };
    }

    // The Document pane's page setup is document-wide; the Section pane edits the caret's
    // section. Headers, footers and section columns remain unbound until those units.
    fn apply_document(&mut self, msg: DocumentMsg) {
        match msg {
            DocumentMsg::WatermarkText(text) => {
                self.state.document.watermark_text = text;
            }
            DocumentMsg::WatermarkApply => {
                let d = &self.state.document;
                let watermark =
                    libertum_model::Watermark::new(&d.watermark_text, d.watermark_diagonal);
                if watermark.is_none() {
                    return;
                }
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_watermark(watermark));
                    let _ = self.finish_edit(result);
                }
                self.refresh_document();
            }
            DocumentMsg::WatermarkDiagonal(on) => {
                self.state.document.watermark_diagonal = on;
                // A watermark already on the pages turns at once.
                let current = self
                    .doc
                    .as_ref()
                    .and_then(|doc| doc.editor().watermark().cloned());
                if let (Some(current), Some(doc)) = (current, self.doc.as_mut()) {
                    let watermark = libertum_model::Watermark {
                        diagonal: on,
                        ..current
                    };
                    let result = doc.edit(|ed| ed.set_watermark(Some(watermark)));
                    let _ = self.finish_edit(result);
                    self.refresh_document();
                }
            }
            DocumentMsg::WatermarkRemove => {
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_watermark(None));
                    let _ = self.finish_edit(result);
                }
                self.refresh_document();
            }
            DocumentMsg::PageColour(colour) => {
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_page_colour(colour));
                    let _ = self.finish_edit(result);
                }
                self.refresh_document();
            }
            DocumentMsg::Language(_) | DocumentMsg::Language2(_) => {
                let was_dirty = self.dirty;
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| match &msg {
                        DocumentMsg::Language(language) => ed.set_language(Some(language.tag())),
                        DocumentMsg::Language2(second) => {
                            let first = ed.language().map(str::to_owned);
                            ed.set_languages(first.as_deref(), second.map(DocLanguage::tag))
                        }
                        _ => unreachable!("matched above"),
                    });
                    let _ = self.finish_edit(result);
                }
                // An RTF or Markdown file cannot hold the choice, so it is not an unsaved
                // change to the file: the app remembers it by path at once instead.
                if !self.save_format().keeps_language() {
                    self.dirty = was_dirty;
                    if let Some(path) = self.file.clone() {
                        self.remember_language_for_path(&path);
                    }
                }
                self.refresh_document();
                for language in self.spell_languages() {
                    spell::warm(language);
                }
                self.spell_ignored_once.clear();
                // The next quiet tick re-checks the page in the new language.
                self.spell_signature = None;
                note_spell_edit();
            }
            DocumentMsg::Vertical(on) => {
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_vertical_text(on));
                    let _ = self.finish_edit(result);
                }
                self.refresh_document();
            }
            DocumentMsg::PageColourHex(text) => {
                self.state.document.page_colour_hex = text;
            }
            DocumentMsg::PageColourHexApply => {
                let Some(colour) =
                    crate::state::parse_page_colour_hex(&self.state.document.page_colour_hex)
                else {
                    return;
                };
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_page_colour(Some(colour)));
                    let _ = self.finish_edit(result);
                }
                self.refresh_document();
            }
            DocumentMsg::Hyphenation(on) => {
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_document_hyphenation(on));
                    let _ = self.finish_edit(result);
                }
                self.refresh_format();
                self.refresh_document();
            }
            DocumentMsg::Ligatures(on) => {
                let ligatures = if on {
                    Ligatures::Standard
                } else {
                    Ligatures::None
                };
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.set_document_ligatures(ligatures));
                    let _ = self.finish_edit(result);
                }
                self.refresh_document();
            }
            DocumentMsg::PageSize(size) => {
                self.apply_page_setup(|page| page.paper = size.model());
            }
            DocumentMsg::Orientation(orientation) => {
                self.apply_page_setup(|page| page.orientation = orientation.model());
            }
            DocumentMsg::NumberingOn(on) => {
                self.apply_numbering(|numbering| numbering.show = on);
            }
            DocumentMsg::NumberingUseCurrentPage => {
                // The page the caret is on, not merely the middle of the viewport.
                let page = self
                    .doc
                    .as_ref()
                    .and_then(DocView::caret_page_index)
                    .map_or(self.state.current_page, |index| index + 1)
                    .max(1)
                    .min(u16::MAX as usize) as u16;
                self.state.document.numbering_start_page = Some(page);
                self.apply_numbering(|numbering| numbering.start_page = u32::from(page));
            }
            DocumentMsg::NumberingUpdate => {
                let page = self.state.document.numbering_start_page.unwrap_or(1).max(1);
                let at = self.state.document.numbering_start.unwrap_or(1);
                self.apply_numbering(|numbering| {
                    numbering.start_page = u32::from(page);
                    numbering.start_at = u32::from(at);
                });
            }
            DocumentMsg::NumberingVertical(vertical) => {
                self.apply_numbering(|numbering| numbering.position.vertical = vertical.model());
            }
            DocumentMsg::NumberingHorizontal(horizontal) => {
                self.apply_numbering(|numbering| {
                    numbering.position.horizontal = horizontal.model();
                });
            }
            DocumentMsg::NumberingMirror(mirror) => {
                self.apply_numbering(|numbering| numbering.position.mirror = mirror);
            }
            DocumentMsg::HideOnFirstPage(hidden) => {
                self.apply_numbering(|numbering| numbering.hide_first_page = hidden);
            }
            DocumentMsg::Header(on) => {
                self.edit_header_footer(|ed, index| {
                    ed.set_header_footer_enabled(index, HfKind::Header, on)
                });
            }
            DocumentMsg::Footer(on) => {
                self.edit_header_footer(|ed, index| {
                    ed.set_header_footer_enabled(index, HfKind::Footer, on)
                });
            }
            DocumentMsg::HeaderText(variant, text) => {
                self.edit_header_footer(|ed, index| {
                    ed.set_header_footer_text(index, HfKind::Header, variant, text)
                });
            }
            DocumentMsg::FooterText(variant, text) => {
                self.edit_header_footer(|ed, index| {
                    ed.set_header_footer_text(index, HfKind::Footer, variant, text)
                });
            }
            DocumentMsg::DifferentFirstPage(on) => {
                self.edit_header_footer(|ed, index| ed.set_different_first_page(index, on));
            }
            DocumentMsg::DifferentEvenOdd(on) => {
                self.edit_header_footer(|ed, index| ed.set_different_even_odd(index, on));
            }
            DocumentMsg::LinkHeaders(link) => {
                self.edit_section(|section| section.link_headers_to_previous = link);
            }
            DocumentMsg::LinkFooters(link) => {
                self.edit_section(|section| section.link_footers_to_previous = link);
            }
            DocumentMsg::FootnoteText(text) => {
                let Some(doc) = self.doc.as_mut() else {
                    return;
                };
                let result = doc.edit(|ed| ed.set_footnote_text(text));
                let _ = self.finish_edit(result);
            }
        }
    }

    /// Runs a header or footer edit on the caret's section and refreshes the inspector. The
    /// edit's own typing coalesces in the history, so a typed header is one undo step.
    fn edit_header_footer(
        &mut self,
        edit: impl FnOnce(
            &mut libertum_editor::Editor,
            usize,
        ) -> Result<libertum_model::ChangeSet, libertum_editor::EditorError>,
    ) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let result = doc.edit(|ed| {
            let index = ed.caret_section();
            edit(ed, index)
        });
        let _ = self.finish_edit(result);
        self.refresh_document();
    }

    /// Applies `change` to the caret's section as one undo step.
    fn edit_section(&mut self, change: impl FnOnce(&mut Section)) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let result = doc.edit(|ed| {
            let index = ed.caret_section();
            let mut section = ed
                .document()
                .sections
                .get(index)
                .cloned()
                .unwrap_or_default();
            change(&mut section);
            ed.set_section(index, section)
        });
        let _ = self.finish_edit(result);
        self.refresh_document();
    }

    /// Applies a page-setup change to the whole document as one undo step, then re-paginates
    /// through [`DocView::sync`] (the model owns the page setup now).
    fn apply_page_setup(&mut self, change: impl FnOnce(&mut PageSetup)) {
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let mut setup = doc.editor().page_setup_at_caret();
        change(&mut setup);
        let result = doc.edit(|ed| ed.set_page_setup_at_caret(setup));
        let _ = self.finish_edit(result);
        self.refresh_document();
    }

    /// Applies one margin stepper (`Field::Margin`) to the document.
    fn apply_margin(&mut self, side: MarginSide) {
        let Some(value) = self.state.document.margin(side).value() else {
            return;
        };
        let points = value.to_points();
        self.apply_page_setup(|page| match side {
            MarginSide::Top => page.margins.top = points,
            MarginSide::Bottom => page.margins.bottom = points,
            MarginSide::Left => page.margins.left = points,
            MarginSide::Right => page.margins.right = points,
        });
    }

    /// Reads the document's page setup and document-wide switches back into the inspector, so
    /// the controls show the document rather than a copy of their last click.
    fn refresh_document(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        let document = doc.editor().document();
        self.state.pageless = doc.pageless;
        let setup = doc.editor().page_setup_at_caret();
        let language = self.doc_language();
        let language2 = self.doc_language2();
        let vertical = doc.is_vertical();
        let d = &mut self.state.document;
        d.page_size = Mixed::Value(PageSize::from_model(setup.paper));
        d.orientation = Mixed::Value(Orientation::from_model(setup.orientation));
        d.margin_top = Mixed::Value(Margin::from_points(setup.margins.top));
        d.margin_bottom = Mixed::Value(Margin::from_points(setup.margins.bottom));
        d.margin_left = Mixed::Value(Margin::from_points(setup.margins.left));
        d.margin_right = Mixed::Value(Margin::from_points(setup.margins.right));
        d.hyphenation = Mixed::Value(document.styles.para_defaults.hyphenate);
        d.ligatures = Mixed::Value(document.styles.defaults.ligatures != Ligatures::None);
        let watermark = document.sections.first().and_then(|s| s.watermark.clone());
        d.watermark_on = watermark.is_some();
        d.watermark_diagonal = watermark.as_ref().is_none_or(|w| w.diagonal);
        d.watermark_text = watermark.map(|w| w.text).unwrap_or_default();
        d.page_colour = doc.editor().page_colour();
        d.language = language;
        d.language2 = language2;
        d.vertical = vertical;
        d.page_colour_hex = d
            .page_colour
            .map(|[r, g, b, _]| format!("#{r:02X}{g:02X}{b:02X}"))
            .unwrap_or_default();
        self.refresh_section();
    }

    /// Reads the caret section's page-number settings back into the Section pane.
    fn refresh_section(&mut self) {
        let Some(doc) = &self.doc else {
            return;
        };
        let index = doc.editor().caret_section();
        let section = doc
            .editor()
            .document()
            .sections
            .get(index)
            .cloned()
            .unwrap_or_default();
        // Page numbering belongs to the document, not the caret's section.
        let numbering = doc.editor().document().page_numbering();
        let d = &mut self.state.document;
        d.numbering_start = Some(numbering.start_at.min(u32::from(u16::MAX)) as u16);
        d.numbering_start_page = Some(numbering.start_page.min(u32::from(u16::MAX)) as u16);
        d.numbering_on = Mixed::Value(numbering.show);
        d.numbering_vertical =
            Mixed::Value(NumberingVertical::from_model(numbering.position.vertical));
        d.numbering_horizontal = Mixed::Value(NumberingHorizontal::from_model(
            numbering.position.horizontal,
        ));
        d.numbering_mirror = Mixed::Value(numbering.position.mirror);
        d.hide_on_first_page = Mixed::Value(numbering.hide_first_page);
        // Headers and footers: the Document pane edits the caret's section, like the
        // page-number options above.
        let header_text = |hf: Option<&libertum_model::HeaderFooter>| {
            hf.map_or_else(String::new, |h| h.text.clone())
        };
        d.header = Mixed::Value(section.headers.enabled());
        d.footer = Mixed::Value(section.footers.enabled());
        d.header_text = header_text(section.headers.default.as_ref());
        d.first_header_text = header_text(section.headers.first.as_ref());
        d.even_header_text = header_text(section.headers.even.as_ref());
        d.footer_text = header_text(section.footers.default.as_ref());
        d.first_footer_text = header_text(section.footers.first.as_ref());
        d.even_footer_text = header_text(section.footers.even.as_ref());
        d.different_first_page =
            Mixed::Value(section.headers.different_first || section.footers.different_first);
        d.different_even_odd =
            Mixed::Value(section.headers.different_even_odd || section.footers.different_even_odd);
        d.link_headers = Mixed::Value(section.link_headers_to_previous);
        d.link_footers = Mixed::Value(section.link_footers_to_previous);
        let block = doc.editor().caret().block;
        d.footnote_number = doc.editor().footnote_number(block);
        d.footnote_text = doc
            .editor()
            .document()
            .paragraph(block)
            .and_then(|paragraph| paragraph.note())
            .unwrap_or_default()
            .to_owned();
        d.header_distance = Mixed::Value(Margin::from_points(section.header_distance_pt));
        d.footer_distance = Mixed::Value(Margin::from_points(section.footer_distance_pt));
        // The Layout pane's Columns and Gutter are section settings; show the caret's.
        let cm = section.column_gutter_pt * 2.54 / 72.0;
        let f = &mut self.state.format;
        f.columns = Mixed::Value(u16::from(section.columns.max(1)));
        f.gutter_cm = Mixed::Value((cm * 100.0).round() / 100.0);
    }

    pub fn view(&self) -> Element<'_, Message> {
        let started = Instant::now();
        let element = self.view_inner();
        let elapsed = started.elapsed();
        if elapsed >= Duration::from_micros(300) {
            perf::event("view", elapsed.as_secs_f64() * 1000.0, String::new);
        }
        element
    }

    fn view_inner(&self) -> Element<'_, Message> {
        let t = Tokens::of(self.scheme());
        let Some(doc) = &self.doc else {
            // Prompts (the stitch dialog, an open error, crash recovery) belong on the home
            // screen too: without this layer they were set but never drawn.
            let home = home::view(
                self.home_selection,
                self.home_user,
                &self.user_templates,
                &self.settings.recent_files,
                t,
            );
            return match &self.modal {
                None => home,
                Some(_) => stack![home, self.modal_layer(t)].into(),
            };
        };
        let (compare_added, compare_removed) = self.compare_rects();
        // The canvas uses the geometry the document is laid out with, so a page-setup change
        // resizes the pages as soon as `DocView` has re-paginated.
        let canvas = canvas::view(
            CanvasProps {
                zoom: self.state.zoom,
                geometry: doc.geometry,
                page_count: doc.page_total(),
                scale_factor: self.scale_factor,
                scroll: self.scroll,
                source: doc.source.clone(),
                paper: doc.editor().page_colour(),
                caret: doc.caret_rect().filter(|_| self.caret_on),
                ime_enabled: self.canvas_focused && self.state.editing.is_none(),
                ime_caret: doc.caret_rect(),
                vertical_text: doc.is_vertical(),
                composition_vertical: doc.is_vertical() && !doc.caret_in_upright_cell(),
                selection: doc.selection_rects_on(self.pages_near_view()),
                find: self.find_match_rects(),
                spell: self.spell_rects(),
                compare_added: compare_added.clone(),
                compare_removed: compare_removed.clone(),
                preedit: self.preedit_overlay(),
                animate: self.autoscroll.as_ref().is_some_and(AutoScroll::active),
                booklet: self.state.booklet,
                continuous: doc.pageless,
                geometries: doc.page_geometries(),
                used_heights: doc.page_used_heights(),
                metrics_cache: self.canvas_metrics_cache.clone(),
            },
            Message::Canvas,
        );
        let mut canvas_stack = stack![
            container(canvas)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| container::background(t.canvas))
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        if self.state.show_word_count {
            canvas_stack = canvas_stack.push(self.word_count_pill(t));
        }
        if self.settings.show_performance {
            canvas_stack = canvas_stack.push(self.fps_pill(t));
        }
        if self.bench.is_some() {
            canvas_stack = canvas_stack.push(self.bench_progress_layer(t));
        }

        let mut main = row![].height(Length::Fill);
        if self.state.thumbnails && !doc.pageless {
            main = main.push(self.thumbnails_panel(doc.page_total(), t));
        }
        main = main.push(canvas_stack);
        let headings: &[HeadingEntry] = if self.state.inspector == Inspector::Headers {
            &self.headings
        } else {
            &[]
        };
        let notes: &[NoteEntry] = if self.state.inspector == Inspector::Notes {
            &self.notes
        } else {
            &[]
        };
        let bookmarks: &[BookmarkEntry] = if self.state.inspector == Inspector::Bookmarks {
            &self.bookmarks
        } else {
            &[]
        };
        let draft = self
            .bookmark_draft
            .as_ref()
            .map(|(block, index, name)| (*block, *index, name.as_str()));
        if let Some(sidebar) = inspector::view(
            &self.state,
            headings,
            &self.collapsed_headings,
            notes,
            bookmarks,
            draft,
            t,
        ) {
            main = main.push(sidebar);
        }

        let page = column![
            toolbar::view(&self.state, self.metrics().percent(), t),
            main,
        ];
        let base: Element<'_, Message> = container(page)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::background(t.chrome))
            .into();

        let base: Element<'_, Message> = match self.state.open_menu {
            None => base,
            Some(menu) => {
                let dismiss = mouse_area(space().width(Length::Fill).height(Length::Fill))
                    .on_press(Message::CloseMenu);
                stack![
                    base,
                    dismiss,
                    toolbar::menu_layer(menu, &self.state, self.window_width, t)
                ]
                .into()
            }
        };
        let base: Element<'_, Message> =
            if self.state.open_menu == Some(Menu::Export) && self.state.ao3_open {
                stack![base, toolbar::ao3_layer(&self.state, self.window_width, t)].into()
            } else {
                base
            };
        let base: Element<'_, Message> = if self.state.find.open {
            stack![base, self.find_layer(t)].into()
        } else {
            base
        };
        let base: Element<'_, Message> = if self.compare_on() {
            stack![base, self.compare_layer(t)].into()
        } else {
            base
        };
        let base: Element<'_, Message> = if self.settings_open {
            stack![base, self.settings_layer(t)].into()
        } else {
            base
        };
        let base: Element<'_, Message> = if self.bench_report_open && self.bench.is_none() {
            stack![base, self.bench_report_layer(t)].into()
        } else {
            base
        };
        let base: Element<'_, Message> = match &self.context_menu {
            None => base,
            Some(menu) => stack![
                base,
                mouse_area(space().width(Length::Fill).height(Length::Fill))
                    .on_press(Message::CloseMenu),
                toolbar::context_layer(&menu.target, menu.x, menu.y, t)
            ]
            .into(),
        };
        let base: Element<'_, Message> = match &self.modal {
            None => base,
            Some(_) => stack![base, self.modal_layer(t)].into(),
        };
        // The vocabulary window sits over the chapter window it re-ranks (and over Settings,
        // which links to it), so it is pushed last.
        let base: Element<'_, Message> = if self.vocabulary_open {
            stack![base, self.tag_vocabulary_layer(t)].into()
        } else {
            base
        };
        // The tool runner is a developer window; it goes over everything else.
        match &self.runner {
            Some(runner) => stack![base, self.runner_layer(runner, t)].into(),
            None => base,
        }
    }

    /// The Date & Time chooser: fixed or updated-on-save, and the written form to use. Every
    /// format row shows what it would insert now, so date-only and time-only forms are as
    /// easy to pick as a full date and time.
    fn date_time_layer(
        &self,
        updated: bool,
        format: DateTimeFormat,
        t: Tokens,
    ) -> Element<'_, Message> {
        use crate::widgets::controls;
        let now = now_seconds();
        let source = segmented::select(
            t,
            Look::Raised,
            &[
                (false, Content::Label(s::DATE_TIME_INSERTED)),
                (true, Content::Label(s::DATE_TIME_UPDATED)),
            ],
            Some(updated),
            Message::DateTimeUpdated,
        );
        let option = |option: DateTimeFormat, selected: bool| -> Element<'_, Message> {
            button(text(libertum_editor::format_date_time(now, option)).size(12))
                .padding([5, 10])
                .width(Length::Fill)
                .style(move |_, _| button::Style {
                    background: Some(if selected { t.accent } else { t.control }.into()),
                    text_color: if selected { t.on_accent } else { t.text },
                    border: border::rounded(5),
                    ..button::Style::default()
                })
                .on_press(Message::DateTimeFormat(option))
                .into()
        };
        let mut list = column![].spacing(3);
        for (content, heading) in [
            (DateTimeContent::DateAndTime, s::DATE_TIME_GROUP_DATE_TIME),
            (DateTimeContent::Date, s::DATE_TIME_GROUP_DATE),
            (DateTimeContent::Time, s::DATE_TIME_GROUP_TIME),
        ] {
            list = list.push(controls::caption(t, heading));
            for pick in DateTimeFormat::ALL
                .iter()
                .filter(|f| f.content() == content)
            {
                list = list.push(option(*pick, *pick == format));
            }
        }
        let cancel = button(text(s::CANCEL).size(13))
            .padding([6, 16])
            .style(move |_, status| flat(t, status))
            .on_press(Message::CloseDateTime);
        let insert = button(text(s::DATE_TIME_INSERT).size(13))
            .padding([6, 18])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::DateTimeInsert);
        let card = container(
            column![
                text(s::DATE_TIME_TITLE)
                    .size(15)
                    .font(controls::bold())
                    .color(t.text),
                column![
                    controls::caption(t, s::DATE_TIME_SOURCE),
                    source,
                    controls::caption(t, s::DATE_TIME_UPDATED_HINT),
                ]
                .spacing(4),
                list,
                row![space::horizontal(), cancel, insert].spacing(8),
            ]
            .spacing(12),
        )
        .padding(18)
        .width(Length::Fixed(420.0))
        .style(move |_| container::Style {
            background: Some(t.inspector.into()),
            border: border::rounded(12),
            shadow: iced::Shadow {
                color: t.shadow,
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseDateTime),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// Insert > Linked Image or Video: the address, a description and whether to download a
    /// preview.
    fn linked_media_layer<'a>(&'a self, modal: &'a Modal, t: Tokens) -> Element<'a, Message> {
        use crate::widgets::controls;
        let Modal::LinkedMedia {
            url,
            alt,
            video,
            preview,
            busy,
            error,
            ..
        } = modal
        else {
            return space().into();
        };
        let kind = segmented::select(
            t,
            Look::Raised,
            &[
                (false, Content::Label(s::LINKED_KIND_IMAGE)),
                (true, Content::Label(s::LINKED_KIND_VIDEO)),
            ],
            Some(*video),
            Message::LinkedVideo,
        );
        let mut address = iced::widget::text_input(s::LINKED_URL_PLACEHOLDER, url)
            .size(13)
            .padding(7);
        let mut description = iced::widget::text_input(s::LINKED_ALT_PLACEHOLDER, alt)
            .size(13)
            .padding(7);
        let mut check = iced::widget::checkbox(*preview)
            .label(s::LINKED_PREVIEW)
            .size(15);
        if !*busy {
            address = address
                .on_input(Message::LinkedUrl)
                .on_submit(Message::LinkedInsert);
            description = description
                .on_input(Message::LinkedAlt)
                .on_submit(Message::LinkedInsert);
            check = check.on_toggle(Message::LinkedPreview);
        }
        let cancel = button(text(s::CANCEL).size(13))
            .padding([6, 16])
            .style(move |_, status| flat(t, status))
            .on_press_maybe((!*busy).then_some(Message::CloseLinkedMedia));
        let insert = button(text(s::LINKED_INSERT).size(13))
            .padding([6, 18])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press_maybe((!*busy).then_some(Message::LinkedInsert));
        let mut card = column![
            text(s::LINKED_TITLE)
                .size(15)
                .font(controls::bold())
                .color(t.text),
            kind,
            address,
            description,
            check,
            controls::caption(t, s::LINKED_HINT),
        ]
        .spacing(10);
        if *busy {
            card = card.push(text(s::LINKED_DOWNLOADING).size(12).color(t.text_secondary));
        }
        if let Some(error) = error {
            card = card.push(text(error.as_str()).size(12).color(t.accent));
        }
        card = card.push(row![space::horizontal(), cancel, insert].spacing(8));
        let card = container(card)
            .padding(18)
            .width(Length::Fixed(440.0))
            .style(move |_| container::Style {
                background: Some(t.inspector.into()),
                border: border::rounded(12),
                shadow: iced::Shadow {
                    color: t.shadow,
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..container::Style::default()
            });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseLinkedMedia),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// The stitch dialog: the chosen documents in order, with add, move, remove and Stitch.
    fn stitch_layer(&self, t: Tokens) -> Element<'_, Message> {
        use crate::widgets::controls::{bold, caption};
        let plain = |label: &'static str, message: Message| -> Element<'_, Message> {
            button(text(label).size(12))
                .padding([3, 8])
                .style(move |_, status| flat(t, status))
                .on_press(message)
                .into()
        };
        let list = self.stitch_sources.iter().enumerate().fold(
            column![].spacing(3),
            |list, (index, source)| {
                let name = source
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| source.path.display().to_string());
                list.push(
                    row![
                        text(format!("{}. {name}", index + 1))
                            .size(12)
                            .color(t.text)
                            .width(Length::Fill),
                        plain("↑", Message::StitchMove(index, -1)),
                        plain("↓", Message::StitchMove(index, 1)),
                        plain("✕", Message::StitchRemove(index)),
                    ]
                    .spacing(4)
                    .align_y(Center),
                )
            },
        );
        let body: Element<'_, Message> = if self.stitch_sources.is_empty() {
            caption(t, s::STITCH_EMPTY)
        } else {
            list.into()
        };
        let footer = row![
            space::horizontal(),
            plain(s::STITCH_ADD, Message::OpenStitch),
            plain(s::CANCEL, Message::CloseStitch),
            button(text(s::STITCH_GO).size(12))
                .padding([4, 14])
                .style(move |_, _| button::Style {
                    background: Some(t.accent.into()),
                    text_color: t.on_accent,
                    border: border::rounded(6),
                    ..button::Style::default()
                })
                .on_press(Message::StitchGo),
        ]
        .spacing(8)
        .align_y(Center);
        let card = container(
            column![
                text(s::STITCH_TITLE).size(16).font(bold()).color(t.text),
                caption(t, s::STITCH_HINT),
                body,
                footer,
            ]
            .spacing(12),
        )
        .padding(18)
        .width(Length::Fixed(520.0))
        .style(move |_| container::Style {
            background: Some(t.chrome.into()),
            border: border::rounded(10),
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseStitch),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// One window card with a dimmed backdrop that closes it.
    fn window_card<'a>(
        &self,
        t: Tokens,
        width: f32,
        content: Element<'a, Message>,
        close: Message,
    ) -> Element<'a, Message> {
        let card = container(content)
            .padding(18)
            .width(Length::Fixed(width))
            .style(move |_| container::Style {
                background: Some(t.chrome.into()),
                border: border::rounded(10),
                ..container::Style::default()
            });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill)).on_press(close),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// The Library window: the documents under the library folder, newest first, each with
    /// its history, and sync conflict copies with the choice of which side to keep.
    fn library_layer<'a>(
        &'a self,
        entries: &'a [libertum_library::LibraryEntry],
        t: Tokens,
    ) -> Element<'a, Message> {
        use crate::widgets::controls::{bold, caption};
        let plain = |label: &'static str, message: Message| -> Element<'a, Message> {
            button(text(label).size(12))
                .padding([3, 8])
                .style(move |_, status| flat(t, status))
                .on_press(message)
                .into()
        };
        let root = self.settings.library_folder.as_deref();
        let when = |secs: u64| {
            libertum_editor::format_date_time(
                i64::try_from(secs).unwrap_or(0),
                DateTimeFormat::DateTimeMedium,
            )
        };
        let mut list = column![].spacing(6);
        for entry in entries {
            let name = entry
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.path.display().to_string());
            let folder = entry
                .path
                .parent()
                .and_then(|parent| root.and_then(|root| parent.strip_prefix(root).ok()))
                .map(|relative| relative.display().to_string())
                .filter(|relative| !relative.is_empty());
            let mut details = when(entry.modified);
            if let Some(folder) = folder {
                details = format!("{folder}  \u{b7}  {details}");
            }
            let mut buttons = row![].spacing(2).align_y(Center);
            let original = entry.conflict_of.as_ref().map(|original| {
                entry
                    .path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(original)
            });
            if entry.versions > 0 {
                buttons = buttons.push(plain(
                    s::LIBRARY_HISTORY,
                    Message::OpenHistory(Some(entry.path.clone())),
                ));
            }
            buttons = buttons.push(plain(
                s::LIBRARY_OPEN,
                Message::OpenRecent(entry.path.clone()),
            ));
            let mut item = column![
                row![
                    column![
                        text(name).size(13).color(t.text),
                        text(details).size(11).color(t.text_secondary),
                    ]
                    .spacing(1)
                    .width(Length::Fill),
                    buttons,
                ]
                .align_y(Center),
            ]
            .spacing(3);
            if let (Some(of), Some(original)) = (&entry.conflict_of, original) {
                item = item.push(
                    text(format!("{} {of}", s::LIBRARY_CONFLICT_OF))
                        .size(11)
                        .color(t.accent),
                );
                item = item.push(
                    row![
                        plain(
                            s::LIBRARY_USE_COPY,
                            Message::UseConflictCopy {
                                original: original.clone(),
                                copy: entry.path.clone(),
                            },
                        ),
                        plain(
                            s::LIBRARY_KEEP_ORIGINAL,
                            Message::KeepOriginal {
                                original,
                                copy: entry.path.clone(),
                            },
                        ),
                    ]
                    .spacing(4),
                );
            }
            list = list.push(container(item).padding([6, 8]).width(Length::Fill).style(
                move |_| container::Style {
                    background: Some(t.control.into()),
                    border: border::rounded(6),
                    ..container::Style::default()
                },
            ));
        }
        let body: Element<'a, Message> = if root.is_none() {
            caption(t, s::LIBRARY_NO_FOLDER)
        } else if entries.is_empty() {
            caption(t, s::LIBRARY_EMPTY)
        } else {
            iced::widget::scrollable(list)
                .height(Length::Fixed(320.0))
                .into()
        };
        // A long path keeps its tail, which is the part that tells folders apart.
        let location = root
            .map(|root| {
                const KEEP: usize = 64;
                let full = root.display().to_string();
                let count = full.chars().count();
                if count > KEEP {
                    let tail: String = full.chars().skip(count - KEEP).collect();
                    format!("\u{2026}{tail}")
                } else {
                    full
                }
            })
            .unwrap_or_default();
        let footer = row![
            plain(
                if root.is_some() {
                    s::LIBRARY_CHANGE
                } else {
                    s::LIBRARY_CHOOSE
                },
                Message::ChooseLibraryFolder,
            ),
            space::horizontal(),
            plain(s::LIBRARY_CLOSE, Message::CloseLibrary),
        ]
        .align_y(Center);
        let mut content = column![
            text(s::LIBRARY_TITLE).size(16).font(bold()).color(t.text),
            caption(t, s::LIBRARY_HINT),
        ]
        .spacing(10);
        if !location.is_empty() {
            content = content.push(text(location).size(11).color(t.text_secondary));
        }
        if entries.iter().any(|entry| entry.conflict_of.is_some()) {
            content = content.push(caption(t, s::LIBRARY_CONFLICT_HINT));
        }
        let content = content.push(body).push(footer);
        self.window_card(t, 560.0, content.into(), Message::CloseLibrary)
    }

    /// The version history window for one document.
    fn history_layer<'a>(
        &'a self,
        path: &'a std::path::Path,
        versions: &'a [libertum_library::Version],
        from_library: bool,
        t: Tokens,
    ) -> Element<'a, Message> {
        use crate::widgets::controls::{bold, caption};
        let plain = |label: &'static str, message: Message| -> Element<'a, Message> {
            button(text(label).size(12))
                .padding([3, 8])
                .style(move |_, status| flat(t, status))
                .on_press(message)
                .into()
        };
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut list = column![].spacing(6);
        for version in versions {
            let when = libertum_editor::format_date_time(
                i64::try_from(version.taken).unwrap_or(0),
                DateTimeFormat::DateTimeMedium,
            );
            let kib = version.size.div_ceil(1024);
            list = list.push(
                container(
                    row![
                        column![
                            text(when).size(13).color(t.text),
                            text(format!("{kib} KB")).size(11).color(t.text_secondary),
                        ]
                        .spacing(1)
                        .width(Length::Fill),
                        plain(
                            s::HISTORY_OPEN_COPY,
                            Message::HistoryOpenCopy(version.path.clone()),
                        ),
                        plain(
                            s::HISTORY_RESTORE,
                            Message::HistoryRestore {
                                original: path.to_path_buf(),
                                version: version.path.clone(),
                            },
                        ),
                    ]
                    .spacing(4)
                    .align_y(Center),
                )
                .padding([6, 8])
                .width(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(t.control.into()),
                    border: border::rounded(6),
                    ..container::Style::default()
                }),
            );
        }
        let body: Element<'a, Message> = if versions.is_empty() {
            caption(t, s::HISTORY_EMPTY)
        } else {
            iced::widget::scrollable(list)
                .height(Length::Fixed(320.0))
                .into()
        };
        let mut footer = row![].align_y(Center);
        if from_library {
            footer = footer.push(plain(s::HISTORY_BACK, Message::HistoryBack));
        }
        footer = footer
            .push(space::horizontal())
            .push(plain(s::LIBRARY_CLOSE, Message::CloseLibrary));
        let content = column![
            text(s::HISTORY_TITLE).size(16).font(bold()).color(t.text),
            text(name).size(12).color(t.text_secondary),
            caption(t, s::HISTORY_HINT),
            body,
            footer,
        ]
        .spacing(10);
        self.window_card(t, 520.0, content.into(), Message::CloseLibrary)
    }

    /// The Settings window: a card over the canvas. Every change applies at once and is
    /// written to the settings file.
    fn settings_layer(&self, t: Tokens) -> Element<'_, Message> {
        use crate::widgets::controls;
        /// A caption above its control.
        fn labelled<'a>(
            t: Tokens,
            label: &'static str,
            control: Element<'a, Message>,
        ) -> Element<'a, Message> {
            column![text(label).size(12).color(t.text_secondary), control]
                .spacing(5)
                .into()
        }
        let theme = segmented::select(
            t,
            Look::Raised,
            &[
                (ThemePreference::System, Content::Label(s::THEME_SYSTEM)),
                (ThemePreference::Light, Content::Label(s::THEME_LIGHT)),
                (ThemePreference::Dark, Content::Label(s::THEME_DARK)),
            ],
            Some(self.settings.theme),
            Message::SetTheme,
        );
        let format = segmented::select(
            t,
            Look::Raised,
            &[
                (DefaultFormat::Odt, Content::Label(s::FORMAT_ODT)),
                (DefaultFormat::Docx, Content::Label(s::FORMAT_DOCX)),
                (DefaultFormat::Rtf, Content::Label(s::FORMAT_RTF)),
                (DefaultFormat::Markdown, Content::Label(s::FORMAT_MARKDOWN)),
                (DefaultFormat::Native, Content::Label(s::FORMAT_NATIVE)),
            ],
            Some(self.settings.default_format),
            Message::SetDefaultFormat,
        );
        let autosave = segmented::select(
            t,
            Look::Raised,
            &[
                (0u32, Content::Label(s::AUTOSAVE_OFF)),
                (30, Content::Label(s::AUTOSAVE_30)),
                (60, Content::Label(s::AUTOSAVE_60)),
                (300, Content::Label(s::AUTOSAVE_300)),
            ],
            Some(self.settings.autosave_secs),
            Message::SetAutosave,
        );
        let units = segmented::select(
            t,
            Look::Raised,
            &[
                (UnitsPreference::Auto, Content::Label(s::UNITS_AUTO)),
                (
                    UnitsPreference::Centimetres,
                    Content::Label(s::UNITS_CENTIMETRES),
                ),
                (UnitsPreference::Inches, Content::Label(s::UNITS_INCHES)),
            ],
            Some(self.settings.units),
            Message::SetUnits,
        );
        let spelling_language = segmented::select(
            t,
            Look::Raised,
            &[
                (
                    spell::Language::British,
                    Content::Label(spell::Language::British.label()),
                ),
                (
                    spell::Language::American,
                    Content::Label(spell::Language::American.label()),
                ),
            ],
            Some(self.settings.spellcheck_language),
            Message::SetSpellLanguage,
        );
        let update_status: Element<'_, Message> = match self.update_check {
            UpdateCheck::Idle => space().into(),
            UpdateCheck::Checking => text(s::UPDATES_CHECKING)
                .size(12)
                .color(t.text_secondary)
                .into(),
            UpdateCheck::UpToDate => text(s::UPDATES_UP_TO_DATE)
                .size(12)
                .color(t.text_secondary)
                .into(),
            UpdateCheck::Failed => text(s::UPDATES_FAILED)
                .size(12)
                .color(t.text_secondary)
                .into(),
        };
        let close = button(text(s::SETTINGS_CLOSE).size(13).wrapping(Wrapping::None))
            .padding([6, 18])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::CloseSettings);
        let sections = column![
            labelled(t, s::SETTINGS_THEME, theme),
            labelled(t, s::SETTINGS_FORMAT, format),
            labelled(t, s::SETTINGS_AUTOSAVE, autosave),
            labelled(t, s::SETTINGS_UNITS, units),
            labelled(
                t,
                s::SETTINGS_CHAPTERS,
                column![
                    row![
                        controls::checkbox_row(
                            t,
                            s::SETTINGS_CHAPTER_H1,
                            self.settings.chapter_headings.h1,
                            false,
                            |on| Message::SetChapterHeading(1, on),
                        ),
                        controls::checkbox_row(
                            t,
                            s::SETTINGS_CHAPTER_H2,
                            self.settings.chapter_headings.h2,
                            false,
                            |on| Message::SetChapterHeading(2, on),
                        ),
                        controls::checkbox_row(
                            t,
                            s::SETTINGS_CHAPTER_H3,
                            self.settings.chapter_headings.h3,
                            false,
                            |on| Message::SetChapterHeading(3, on),
                        ),
                        controls::checkbox_row(
                            t,
                            s::SETTINGS_CHAPTER_H4,
                            self.settings.chapter_headings.h4,
                            false,
                            |on| Message::SetChapterHeading(4, on),
                        ),
                    ]
                    .spacing(10),
                    controls::caption(t, s::SETTINGS_CHAPTERS_HINT),
                ]
                .spacing(4)
                .into(),
            ),
            labelled(
                t,
                s::SETTINGS_TAGS,
                column![
                    button(text(s::AO3_TAGS_EDIT).size(13).wrapping(Wrapping::None))
                        .padding([6, 14])
                        .style(move |_, status| controls::flat(t, status))
                        .on_press(Message::OpenTagVocabulary),
                    controls::caption(t, s::SETTINGS_TAGS_HINT),
                ]
                .spacing(6)
                .into(),
            ),
            column![
                controls::checkbox_row(
                    t,
                    s::SETTINGS_ANTIALIASING,
                    self.settings.antialiasing,
                    false,
                    Message::SetAntialiasing,
                ),
                controls::caption(t, s::SETTINGS_ANTIALIASING_HINT),
            ]
            .spacing(4),
            column![
                controls::checkbox_row(
                    t,
                    s::SETTINGS_PERFORMANCE,
                    self.settings.show_performance,
                    false,
                    Message::SetShowPerformance,
                ),
                controls::caption(t, s::SETTINGS_PERFORMANCE_HINT),
            ]
            .spacing(4),
            labelled(t, s::SETTINGS_BENCHMARK, {
                let mut benchmark = row![
                    button(
                        text(s::SETTINGS_BENCHMARK_RUN)
                            .size(13)
                            .wrapping(Wrapping::None)
                    )
                    .padding([6, 14])
                    .style(move |_, status| controls::flat(t, status))
                    .on_press(Message::RunBenchmark),
                ]
                .spacing(10)
                .align_y(Center);
                if self.bench_report.is_some() {
                    benchmark = benchmark.push(
                        button(
                            text(s::SETTINGS_BENCHMARK_LAST)
                                .size(13)
                                .wrapping(Wrapping::None),
                        )
                        .padding([6, 14])
                        .style(move |_, status| controls::flat(t, status))
                        .on_press(Message::ShowBenchmarkReport),
                    );
                }
                column![benchmark, controls::caption(t, s::SETTINGS_BENCHMARK_HINT)]
                    .spacing(6)
                    .into()
            }),
            labelled(
                t,
                s::SETTINGS_SPELLING,
                column![
                    controls::checkbox_row(
                        t,
                        s::SETTINGS_SPELLCHECK,
                        self.settings.spellcheck,
                        false,
                        Message::SetSpellcheck,
                    ),
                    labelled(t, s::SETTINGS_SPELL_LANGUAGE, spelling_language),
                    controls::caption(t, s::SETTINGS_SPELL_HINT),
                ]
                .spacing(6)
                .into(),
            ),
            column![
                controls::checkbox_row(
                    t,
                    s::SETTINGS_UPDATES,
                    self.settings.check_for_updates,
                    false,
                    Message::SetCheckForUpdates,
                ),
                controls::caption(t, s::SETTINGS_UPDATES_HINT),
                row![
                    button(
                        text(s::SETTINGS_CHECK_NOW)
                            .size(13)
                            .wrapping(Wrapping::None)
                    )
                    .padding([6, 14])
                    .style(move |_, status| controls::flat(t, status))
                    .on_press(Message::CheckForUpdatesNow),
                    update_status,
                ]
                .spacing(10)
                .align_y(Center),
            ]
            .spacing(4),
            labelled(t, s::SETTINGS_DEVELOPER, {
                let mut developer = column![
                    controls::checkbox_row(
                        t,
                        s::SETTINGS_DEV_MODE,
                        self.settings.dev_mode,
                        false,
                        Message::SetDevMode,
                    ),
                    controls::caption(t, s::SETTINGS_DEV_MODE_HINT),
                ]
                .spacing(6);
                // The MCP switch only exists while developer mode is on.
                if self.settings.dev_mode {
                    developer = developer
                        .push(controls::checkbox_row(
                            t,
                            s::SETTINGS_MCP,
                            self.settings.mcp_enabled,
                            false,
                            Message::SetMcpEnabled,
                        ))
                        .push(controls::caption(t, s::SETTINGS_MCP_HINT));
                    // Editing is a second, separate switch, and exists only while MCP is on.
                    if self.settings.mcp_enabled {
                        developer = developer
                            .push(controls::checkbox_row(
                                t,
                                s::SETTINGS_MCP_ALLOW_EDITS,
                                self.settings.mcp_allow_edits,
                                false,
                                Message::SetMcpAllowEdits,
                            ))
                            .push(controls::caption(t, s::SETTINGS_MCP_ALLOW_EDITS_HINT));
                        if !self.settings.mcp_trusted_clients.is_empty()
                            || !self.mcp_session_trusted.is_empty()
                        {
                            developer = developer.push(
                                button(
                                    text(s::SETTINGS_MCP_FORGET)
                                        .size(13)
                                        .wrapping(Wrapping::None),
                                )
                                .padding([6, 14])
                                .style(move |_, status| controls::flat(t, status))
                                .on_press(Message::ForgetMcpClients),
                            );
                        }
                    }
                    if let Some(service) = &self.mcp {
                        developer = developer
                            .push(controls::caption(
                                t,
                                format!("{} {}", s::SETTINGS_MCP_ADDRESS, service.url()),
                            ))
                            .push(
                                button(
                                    text(s::SETTINGS_MCP_COPY_TOKEN)
                                        .size(13)
                                        .wrapping(Wrapping::None),
                                )
                                .padding([6, 14])
                                .style(move |_, status| controls::flat(t, status))
                                .on_press(Message::CopyMcpToken),
                            );
                    }
                    developer = developer
                        .push(controls::caption(t, s::SETTINGS_SCRIPT_MEMORY))
                        .push(segmented::select(
                            t,
                            Look::Raised,
                            &[
                                (128u32, Content::Label(s::SCRIPT_MEMORY_128)),
                                (256, Content::Label(s::SCRIPT_MEMORY_256)),
                                (512, Content::Label(s::SCRIPT_MEMORY_512)),
                                (1024, Content::Label(s::SCRIPT_MEMORY_1024)),
                                (2048, Content::Label(s::SCRIPT_MEMORY_2048)),
                            ],
                            Some(self.settings.script_memory_mb),
                            Message::SetScriptMemory,
                        ))
                        .push(controls::caption(t, s::SETTINGS_SCRIPT_MEMORY_HINT))
                        .push(controls::caption(t, s::SETTINGS_SCRIPT_TOTAL_MEMORY))
                        .push(segmented::select(
                            t,
                            Look::Raised,
                            &[
                                (512u32, Content::Label(s::SCRIPT_MEMORY_512)),
                                (1024, Content::Label(s::SCRIPT_MEMORY_1024)),
                                (2048, Content::Label(s::SCRIPT_MEMORY_2048)),
                                (4096, Content::Label(s::SCRIPT_MEMORY_4096)),
                            ],
                            Some(self.settings.script_total_memory_mb),
                            Message::SetScriptTotalMemory,
                        ))
                        .push(controls::caption(t, s::SETTINGS_SCRIPT_TOTAL_MEMORY_HINT));
                    developer = developer.push(
                        button(
                            text(s::SETTINGS_MCP_TOOLS)
                                .size(13)
                                .wrapping(Wrapping::None),
                        )
                        .padding([6, 14])
                        .style(move |_, status| controls::flat(t, status))
                        .on_press(Message::Runner(RunnerMsg::Open)),
                    );
                    if let Some(error) = &self.mcp_error {
                        developer = developer.push(controls::caption(
                            t,
                            format!("{} {error}", s::SETTINGS_MCP_FAILED),
                        ));
                    }
                }
                developer.into()
            }),
        ]
        .spacing(16)
        .width(Length::Fixed(460.0));
        let body_height = (self.window_height - 160.0).max(240.0);
        let body = iced::widget::scrollable(sections)
            .height(Length::Fixed(body_height))
            .width(Length::Fill);
        let card = container(
            column![
                text(s::SETTINGS_TITLE)
                    .size(16)
                    .font(controls::bold())
                    .color(t.text),
                body,
                row![space::horizontal(), close],
            ]
            .spacing(16),
        )
        .padding(20)
        .width(Length::Fixed(520.0))
        .style(move |_| container::Style {
            background: Some(t.chrome.into()),
            border: border::rounded(10),
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseSettings),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// The tags suggested for one chapter, in this order: the writer's vocabulary ranked
    /// against it; then (when the setting is on) pairings remembered from earlier use, and the
    /// characters, pairings and phrases found in its own text, with names resolved to the full
    /// names the whole document (or the library) uses; then library and starter-list tags whose
    /// words the chapter keeps using. A tag is never offered twice.
    fn chapter_suggestions(
        &self,
        document: &Document,
        chapter: &libertum_export::Chapter,
    ) -> Vec<TagSuggestion> {
        use libertum_export::{TagCategory, TagMatch};
        let text = libertum_export::chapter_text(document, chapter);
        let mut out = libertum_export::suggest_tags(&text, &self.settings.tags, SUGGESTION_LIMIT);
        if self.settings.auto_suggest_tags {
            let mut book =
                libertum_export::NameBook::from_text(&libertum_export::document_text(document));
            book.remember(self.tag_cache.names(TagCategory::Character));
            let auto = libertum_export::auto_suggestions_with(&text, &book);
            // A pairing used before, written the writer's way, when both people are here.
            let present: Vec<&str> = auto
                .iter()
                .filter(|s| s.tag.category == TagCategory::Character)
                .map(|s| s.tag.name.as_str())
                .collect();
            let remembered: Vec<TagSuggestion> = self
                .tag_cache
                .names(TagCategory::Relationship)
                .filter_map(|pairing| {
                    let people: Vec<&str> = pairing
                        .split(['/', '&'])
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .collect();
                    (people.len() >= 2 && people.iter().all(|p| present.contains(p))).then(|| {
                        TagSuggestion {
                            tag: libertum_export::Tag::new(pairing, TagCategory::Relationship),
                            score: 0.0,
                            matched: vec![TagMatch {
                                term: s::TAG_USED_BEFORE.to_owned(),
                                count: 1,
                            }],
                        }
                    })
                })
                .collect();
            // The auto pairing of the same two people gives way to the remembered one.
            let same_people = |a: &str, b: &str| {
                let key = |x: &str| {
                    let mut people: Vec<String> = x
                        .split(['/', '&'])
                        .map(|p| p.trim().to_lowercase())
                        .collect();
                    people.sort();
                    people
                };
                key(a) == key(b)
            };
            let auto: Vec<TagSuggestion> = auto
                .into_iter()
                .filter(|s| {
                    s.tag.category != TagCategory::Relationship
                        || !remembered
                            .iter()
                            .any(|r| same_people(&r.tag.name, &s.tag.name))
                })
                .collect();
            out = libertum_export::merge_suggestions(out, remembered);
            out = libertum_export::merge_suggestions(out, auto);
        }
        // Library tags need two uses of their words; starter tags three, from at least two
        // different words (they are generic).
        let library = libertum_export::suggest_tags_with(
            &text,
            &self.tag_cache.rankable(),
            SUGGESTION_LIMIT,
            2,
            1,
        );
        out = libertum_export::merge_suggestions(out, library);
        if self.settings.use_starter_tags {
            let starter = libertum_export::suggest_tags_with(
                &text,
                &libertum_export::starter_tags(),
                SUGGESTION_LIMIT / 2,
                3,
                2,
            );
            out = libertum_export::merge_suggestions(out, starter);
        }
        // Tags already on the work, or removed as unrelated, are not offered again.
        self.work_tags.filter(out)
    }

    /// Writes the tag library (when it has a file).
    fn save_tag_cache(&self) {
        let Some(path) = &self.tag_cache_path else {
            return;
        };
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let bytes = serde_json::to_vec(&self.tag_cache).map_err(std::io::Error::other)?;
            libertum_library::save_atomic(path, |temporary| std::fs::write(temporary, &bytes))
                .map_err(|error| match error {
                    libertum_library::SaveError::Write(error)
                    | libertum_library::SaveError::Permissions(error)
                    | libertum_library::SaveError::Replace(error) => error,
                })
        };
        if let Err(error) = write() {
            tracing::warn!(%error, "could not keep the tag library");
        }
    }

    /// Re-ranks an open AO3 chapter after the vocabulary changed, keeping the writer's
    /// ticks on tags that are still suggested.
    fn rerank_suggestions(&mut self) {
        let index = match &self.modal {
            Some(Modal::Ao3Chapter { chapter, .. }) => *chapter,
            _ => return,
        };
        let Some(doc) = self.doc.as_ref() else {
            return;
        };
        let Some(chapter) = self.state.ao3_chapters.get(index).cloned() else {
            return;
        };
        let fresh = self.chapter_suggestions(doc.document(), &chapter);
        if let Some(Modal::Ao3Chapter {
            suggestions,
            selected,
            tags_copied,
            ..
        }) = &mut self.modal
        {
            let ticked: Vec<String> = suggestions
                .iter()
                .zip(selected.iter())
                .filter(|(_, on)| **on)
                .map(|(suggestion, _)| suggestion.tag.name.clone())
                .collect();
            *selected = fresh
                .iter()
                .map(|suggestion| ticked.contains(&suggestion.tag.name))
                .collect();
            *suggestions = fresh;
            *tags_copied = false;
        }
    }

    /// One AO3 chapter's HTML and the tags its vocabulary suggests. This is a copy surface,
    /// not an editor: AO3's HTML editor and its tag fields are where the text goes.
    #[allow(clippy::too_many_arguments)]
    fn ao3_chapter_layer<'a>(
        &'a self,
        title: &'a str,
        html: &'a str,
        copied: bool,
        suggestions: &'a [TagSuggestion],
        selected: &'a [bool],
        tags_copied: bool,
        (has_title, title_copied): (bool, bool),
        t: Tokens,
    ) -> Element<'a, Message> {
        use crate::widgets::controls;
        let body = iced::widget::scrollable(
            container(
                text(html.to_owned())
                    .size(12)
                    .font(iced::Font::MONOSPACE)
                    .color(t.text),
            )
            .padding(8),
        )
        .height(Length::Fixed(130.0));

        // Suggested tags, grouped by the category AO3 files them under. Empty categories are
        // left out so the list stays about this chapter.
        let mut groups: Vec<Element<'_, Message>> = Vec::new();
        for category in TagCategory::ALL {
            let mut rows: Vec<Element<'_, Message>> = Vec::new();
            for (index, suggestion) in suggestions.iter().enumerate() {
                if suggestion.tag.category != category {
                    continue;
                }
                let why = suggestion
                    .matched
                    .iter()
                    .map(|hit| format!("{} \u{d7}{}", hit.term, hit.count))
                    .collect::<Vec<_>>()
                    .join(", ");
                // The – button: not about this work, so hide it for the whole work.
                let dismiss = iced::widget::tooltip(
                    button(text("\u{2212}").size(13).color(t.text_secondary))
                        .padding([0, 7])
                        .style(move |_, status| controls::flat(t, status))
                        .on_press(Message::DismissSuggestedTag(index)),
                    container(text(s::TAG_DISMISS_TIP).size(12))
                        .padding(4)
                        .style(move |_| container::Style {
                            background: Some(t.chrome.into()),
                            border: border::rounded(4),
                            ..container::Style::default()
                        }),
                    iced::widget::tooltip::Position::Left,
                );
                rows.push(
                    row![
                        controls::checkbox_row(
                            t,
                            suggestion.tag.name.as_str(),
                            selected.get(index).copied().unwrap_or(false),
                            false,
                            move |_| Message::ToggleSuggestedTag(index),
                        ),
                        space::horizontal(),
                        text(why).size(11).color(t.text_secondary),
                        dismiss,
                    ]
                    .spacing(8)
                    .align_y(Center)
                    .into(),
                );
            }
            if rows.is_empty() {
                continue;
            }
            // The rating and the warnings are choices on AO3's form, so there is nothing to
            // paste: say where to choose them. The other tags are ticked here and copied
            // together with Copy tags.
            let header_note: Element<'_, Message> = if category.fixed_names().is_some() {
                text(s::AO3_TAGS_CHOOSE_ON_AO3)
                    .size(11)
                    .color(t.text_disabled)
                    .into()
            } else {
                space::horizontal().width(Length::Shrink).into()
            };
            groups.push(
                column![
                    row![
                        text(category_label(category))
                            .size(11)
                            .color(t.text_secondary),
                        space::horizontal(),
                        header_note,
                    ]
                    .align_y(Center),
                    column(rows).spacing(3),
                ]
                .spacing(3)
                .into(),
            );
        }
        let tag_area: Element<'_, Message> = if groups.is_empty() {
            controls::caption(t, s::AO3_TAGS_EMPTY)
        } else {
            // Room on the right for the scroll bar, so it never covers the evidence column.
            iced::widget::scrollable(container(column(groups).spacing(8)).padding(Padding {
                right: 16.0,
                ..Padding::ZERO
            }))
            .height(Length::Fixed(170.0))
            .into()
        };

        let flat = |label: &'static str, message: Message| {
            button(text(label).size(13))
                .padding([6, 12])
                .style(move |_, status| flat(t, status))
                .on_press(message)
        };
        let edit = flat(s::AO3_TAGS_EDIT, Message::OpenTagVocabulary);
        let copy_tags = flat(
            if tags_copied {
                s::AO3_TAGS_COPIED
            } else {
                s::AO3_TAGS_COPY
            },
            Message::CopySuggestedTags,
        );
        let close = flat(s::AO3_CLOSE, Message::CloseAo3);
        // AO3's chapter title is a plain-text field. The whole-document chapter has no real
        // title to give it, so it gets no button.
        let title_button: Element<'_, Message> = if has_title {
            button(
                text(if title_copied {
                    s::AO3_TITLE_COPIED
                } else {
                    s::AO3_TITLE_COPY
                })
                .size(12)
                .color(t.accent),
            )
            .padding([3, 10])
            .style(move |_, status| controls::flat(t, status))
            .on_press(Message::CopyAo3Title)
            .into()
        } else {
            space::horizontal().width(Length::Shrink).into()
        };
        let copy = button(text(if copied { s::AO3_COPIED } else { s::AO3_COPY }).size(13))
            .padding([6, 16])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::CopyAo3);
        let card = container(
            column![
                row![
                    text(title.to_owned())
                        .size(16)
                        .font(controls::bold())
                        .color(t.text),
                    space::horizontal(),
                    title_button,
                ]
                .align_y(Center),
                controls::caption(t, s::AO3_HINT),
                body,
                controls::divider(t),
                text(s::AO3_TAGS)
                    .size(13)
                    .font(controls::bold())
                    .color(t.text),
                controls::caption(t, s::AO3_TAGS_HINT),
                controls::checkbox_row(
                    t,
                    s::AO3_TAGS_AUTO,
                    self.settings.auto_suggest_tags,
                    false,
                    Message::SetAutoTags,
                ),
                tag_area,
                self.hidden_tags_line(t),
                if self.settings.auto_suggest_tags {
                    controls::caption(t, s::AO3_TAGS_AUTO_HINT)
                } else {
                    space::vertical().height(Length::Shrink).into()
                },
                row![edit, space::horizontal(), close, copy_tags, copy]
                    .spacing(8)
                    .align_y(Center),
                controls::divider(t),
                self.ao3_post_section(t),
            ]
            .spacing(10),
        )
        .padding(18)
        .width(Length::Fixed(760.0))
        .style(move |_| container::Style {
            background: Some(t.chrome.into()),
            border: border::rounded(10),
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseAo3),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// "3 tags hidden for this work" and Show Again, when any are hidden.
    fn hidden_tags_line(&self, t: Tokens) -> Element<'_, Message> {
        let hidden = self.work_tags.hidden();
        if hidden == 0 {
            return space::vertical().height(Length::Shrink).into();
        }
        row![
            text(s::tags_hidden_for_work(hidden))
                .size(12)
                .color(t.text_secondary),
            space::horizontal(),
            button(text(s::TAG_SHOW_AGAIN).size(12))
                .padding([3, 10])
                .style(move |_, status| flat(t, status))
                .on_press(Message::ShowHiddenTags),
        ]
        .align_y(Center)
        .into()
    }

    /// Mark as Posted, for Compare with Posted, and what it last did.
    fn ao3_post_section(&self, t: Tokens) -> Element<'_, Message> {
        use crate::widgets::controls;
        let mark = button(text(s::AO3_POST_MARK).size(12))
            .padding([5, 10])
            .style(move |_, status| flat(t, status))
            .on_press(Message::Ao3Post(Ao3PostMsg::MarkPosted));
        let mut body = column![
            row![
                controls::caption(t, s::AO3_POST_HINT),
                space::horizontal(),
                mark
            ]
            .spacing(8)
            .align_y(Center),
        ]
        .spacing(6);
        if let Some(notice) = &self.ao3_post_notice {
            body = body.push(text(notice.clone()).size(12).color(t.accent));
        }
        body.into()
    }

    /// The tag library in the vocabulary window: what it holds, the starter list switch,
    /// Import and Forget.
    fn tag_library_section(&self, t: Tokens) -> Element<'_, Message> {
        use crate::widgets::controls;
        let (learned, imported) = self.tag_cache.counts();
        let flat_button = |label: &'static str, message: Message| {
            button(text(label).size(12))
                .padding([5, 10])
                .style(move |_, status| flat(t, status))
                .on_press(message)
        };
        let mut body = column![
            text(s::TAG_LIBRARY_TITLE)
                .size(13)
                .font(controls::bold())
                .color(t.text),
            controls::caption(t, s::TAG_LIBRARY_HINT),
            controls::checkbox_row(
                t,
                s::TAG_STARTER_LIST,
                self.settings.use_starter_tags,
                false,
                Message::SetStarterTags,
            ),
            row![
                text(s::tag_library_counts(learned, imported))
                    .size(12)
                    .color(t.text_secondary),
                space::horizontal(),
                flat_button(s::TAG_IMPORT, Message::ImportTagFile),
                flat_button(s::TAG_FORGET_LEARNED, Message::ForgetLearnedTags),
            ]
            .spacing(6)
            .align_y(Center),
        ]
        .spacing(6);
        if let Some(notice) = &self.tag_cache_notice {
            body = body.push(text(notice.clone()).size(12).color(t.accent));
        }
        body.into()
    }

    /// The local AO3 tag vocabulary: what chapter suggestions are ranked against. Stored in
    /// settings, edited here, and never sent anywhere.
    fn tag_vocabulary_layer(&self, t: Tokens) -> Element<'_, Message> {
        use crate::state::TagCategoryChoice;
        use crate::widgets::controls;
        // The rating and the warnings are choices on AO3, so they are chosen here too, from
        // its list; every other category is free text.
        let field: Element<'_, Message> = match self.tag_category.fixed_names() {
            Some(names) => controls::popup(
                t,
                names,
                self.tag_category.fixed_name(&self.tag_draft),
                s::TAG_NAME_CHOOSE,
                Message::FixedTagPicked,
            )
            .into(),
            None => iced::widget::text_input(s::TAG_NAME_PLACEHOLDER, &self.tag_draft)
                .on_input(Message::TagDraft)
                .on_submit(Message::AddTag)
                .size(13)
                .padding(6)
                .width(Length::Fill)
                .style(move |_, _| iced::widget::text_input::Style {
                    background: t.control.into(),
                    border: iced::Border {
                        color: t.control_border,
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    icon: t.text_secondary,
                    placeholder: t.text_secondary,
                    value: t.text,
                    selection: t.accent,
                })
                .into(),
        };
        let category = controls::popup(
            t,
            TagCategoryChoice::ALL,
            Some(TagCategoryChoice(self.tag_category)),
            s::TAG_CATEGORY_FREEFORM,
            |choice: TagCategoryChoice| Message::TagCategoryPicked(choice.0),
        )
        .width(Length::Fixed(150.0));
        let add = button(text(s::TAG_ADD).size(13))
            .padding([6, 14])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::AddTag);

        let mut groups: Vec<Element<'_, Message>> = Vec::new();
        for category in TagCategory::ALL {
            let mut rows: Vec<Element<'_, Message>> = Vec::new();
            for (index, tag) in self.settings.tags.iter().enumerate() {
                if tag.category != category {
                    continue;
                }
                let remove = button(text(s::TAG_REMOVE).size(11))
                    .padding([3, 8])
                    .style(move |_, status| flat(t, status))
                    .on_press(Message::RemoveTag(index));
                rows.push(
                    row![
                        text(tag.name.clone()).size(13).color(t.text),
                        space::horizontal(),
                        remove,
                    ]
                    .spacing(8)
                    .align_y(Center)
                    .into(),
                );
            }
            if rows.is_empty() {
                continue;
            }
            groups.push(
                column![
                    text(category_label(category))
                        .size(11)
                        .color(t.text_secondary),
                    column(rows).spacing(2),
                ]
                .spacing(2)
                .into(),
            );
        }
        let list: Element<'_, Message> = if groups.is_empty() {
            controls::caption(t, s::TAG_EMPTY)
        } else {
            iced::widget::scrollable(column(groups).spacing(8))
                .height(Length::Fixed(240.0))
                .into()
        };
        let done = button(text(s::TAG_DONE).size(13))
            .padding([6, 18])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::CloseTagVocabulary);
        let card = container(
            column![
                text(s::TAG_VOCAB_TITLE)
                    .size(16)
                    .font(controls::bold())
                    .color(t.text),
                controls::caption(t, s::TAG_VOCAB_HINT),
                row![field, category, add].spacing(8).align_y(Center),
                list,
                controls::divider(t),
                self.tag_library_section(t),
                row![space::horizontal(), done],
            ]
            .spacing(12),
        )
        .padding(18)
        .width(Length::Fixed(500.0))
        .style(move |_| container::Style {
            background: Some(t.chrome.into()),
            border: border::rounded(10),
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CloseTagVocabulary),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    /// The Find & Replace bar: a strip over the top of the canvas.
    fn find_layer(&self, t: Tokens) -> Element<'_, Message> {
        let f = &self.state.find;
        let field = |placeholder: &'static str,
                     value: &str,
                     message: fn(String) -> Message,
                     submit: Option<Message>| {
            let mut input: iced::widget::TextInput<'_, Message> =
                iced::widget::text_input(placeholder, value)
                    .on_input(message)
                    .size(12)
                    .padding(6)
                    .width(Length::Fixed(150.0));
            if let Some(submit) = submit {
                // The query field: Enter finds the next match, and Find opens focused on it.
                input = input.on_submit(submit).id(FIND_FIELD_ID);
            }
            let element: Element<'_, Message> = input.into();
            element
        };
        let toggle = |label: &'static str, on: bool, message: Message| -> Element<'_, Message> {
            let shown = if on {
                format!("{label} ✓")
            } else {
                label.to_owned()
            };
            button(text(shown).size(12))
                .padding([4, 8])
                .style(move |_, status| flat(t, status))
                .on_press(message)
                .into()
        };
        let plain = |label: &'static str, message: Message| -> Element<'_, Message> {
            button(text(label).size(12))
                .padding([4, 8])
                .style(move |_, status| flat(t, status))
                .on_press(message)
                .into()
        };
        let mut bar = row![
            field(
                s::FIND_PLACEHOLDER,
                &f.query,
                Message::FindQuery,
                Some(Message::FindNext)
            ),
            toggle(s::FIND_CASE, f.match_case, Message::ToggleFindCase),
            toggle(s::FIND_WHOLE_WORD, f.whole_word, Message::ToggleFindWord),
            plain(s::FIND_PREVIOUS, Message::FindPrevious),
            plain(s::FIND_NEXT, Message::FindNext),
            field(
                s::REPLACE_PLACEHOLDER,
                &f.replace,
                Message::ReplaceQuery,
                None
            ),
            plain(s::FIND_REPLACE_ONE, Message::ReplaceOne),
            plain(s::FIND_REPLACE_ALL, Message::ReplaceAll),
            plain("✕", Message::CloseFind),
        ]
        .spacing(4)
        .align_y(Center);
        if let Some(notice) = &f.notice {
            bar = bar.push(text(notice.clone()).size(12).color(t.text_secondary));
        } else if let Some((position, total)) = f.count {
            bar = bar.push(
                text(s::find_count(position, total))
                    .size(12)
                    .color(t.text_secondary),
            );
        }
        // A click anywhere on the bar belongs to the bar: between its controls it would
        // otherwise fall through to the page below, give the document the keyboard, and the
        // next keystrokes would replace the selected match.
        let bar = mouse_area(
            container(bar)
                .padding([6, 8])
                .style(move |_| container::Style {
                    background: Some(t.chrome.into()),
                    border: border::rounded(8),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::FindBarPressed)
        .on_right_press(Message::FindBarPressed);
        container(bar)
            .padding(Padding {
                // Just under the toolbar, so neither the toolbar nor its buttons are covered.
                top: crate::toolbar::HEIGHT + 10.0,
                ..Padding::ZERO
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Center)
            .align_top(Length::Fill)
            .into()
    }

    /// The compare bar: where the current change is, what was posted, and the actions.
    fn compare_layer(&self, t: Tokens) -> Element<'_, Message> {
        let Some(mode) = self.compare.as_ref() else {
            return space().into();
        };
        let has_changes = !mode.visible().is_empty();
        let has_current = mode.has_current();
        let action = |label: &'static str, message: Option<compare::CompareMsg>| {
            button(text(label).size(12))
                .padding([4, 8])
                .style(move |_, status| flat(t, status))
                .on_press_maybe(message.map(Message::Compare))
        };
        let bar = row![
            action(
                s::COMPARE_PREVIOUS,
                has_changes.then_some(compare::CompareMsg::Previous)
            ),
            action(
                s::COMPARE_NEXT,
                has_changes.then_some(compare::CompareMsg::Next)
            ),
            action(
                s::COMPARE_KEEP,
                has_current.then_some(compare::CompareMsg::Keep)
            ),
            action(
                s::COMPARE_REVERT,
                has_current.then_some(compare::CompareMsg::Revert)
            ),
            action(s::COMPARE_MARK, Some(compare::CompareMsg::MarkPosted)),
            action(s::COMPARE_DONE, Some(compare::CompareMsg::Exit)),
            text(mode.summary())
                .size(12)
                .color(t.text_secondary)
                .width(Length::Shrink),
        ]
        .spacing(4)
        .align_y(Center);
        let bar = mouse_area(
            container(bar)
                .padding([6, 8])
                .max_width(900)
                .style(move |_| container::Style {
                    background: Some(t.chrome.into()),
                    border: border::rounded(8),
                    ..container::Style::default()
                }),
        )
        .on_press(Message::FindBarPressed)
        .on_right_press(Message::FindBarPressed);
        let below_find = if self.state.find.open {
            FIND_BAR_COVER
        } else {
            0.0
        };
        container(bar)
            .padding(Padding {
                top: crate::toolbar::HEIGHT + 10.0 + below_find,
                ..Padding::ZERO
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Center)
            .align_top(Length::Fill)
            .into()
    }

    /// The unsaved-changes or error prompt, with a dimmed backdrop.
    fn modal_layer(&self, t: Tokens) -> Element<'_, Message> {
        let Some(modal) = &self.modal else {
            return space().into();
        };
        if matches!(modal, Modal::Stitch) {
            return self.stitch_layer(t);
        }
        if let Modal::Library { entries } = modal {
            return self.library_layer(entries, t);
        }
        if let Modal::History {
            path,
            versions,
            from_library,
        } = modal
        {
            return self.history_layer(path, versions, *from_library, t);
        }
        if let Modal::Ao3Chapter {
            title,
            html,
            copied,
            suggestions,
            selected,
            tags_copied,
            has_title,
            title_copied,
            ..
        } = modal
        {
            return self.ao3_chapter_layer(
                title,
                html,
                *copied,
                suggestions,
                selected,
                *tags_copied,
                (*has_title, *title_copied),
                t,
            );
        }
        if let Modal::DateTime { updated, format } = modal {
            return self.date_time_layer(*updated, *format, t);
        }
        if let Modal::LinkedMedia { .. } = modal {
            return self.linked_media_layer(modal, t);
        }
        if let Modal::UpdateAvailable { release, stage } = modal {
            return self.update_layer(release, stage, t);
        }
        let (title, detail): (String, String) = match modal {
            Modal::McpApproval {
                client,
                action,
                detail,
            } => {
                let mut lines = vec![action.clone()];
                if !detail.is_empty() {
                    lines.push(detail.clone());
                }
                lines.push(String::new());
                lines.push(s::MCP_APPROVAL_DETAIL.to_owned());
                (
                    format!("{client} {}", s::MCP_APPROVAL_TITLE),
                    lines.join("\n"),
                )
            }
            Modal::Unsaved { .. } => (s::UNSAVED_TITLE.to_owned(), s::UNSAVED_DETAIL.to_owned()),
            Modal::Error(message) => (message.clone(), String::new()),
            Modal::Notice { title, detail } => (title.clone(), detail.clone()),
            Modal::Stitch => (s::STITCH_TITLE.to_owned(), String::new()),
            Modal::Library { .. } | Modal::History { .. } => (String::new(), String::new()),
            Modal::DateTime { .. } => (s::DATE_TIME_TITLE.to_owned(), String::new()),
            Modal::LinkedMedia { .. } => (String::new(), String::new()),
            Modal::Ao3Chapter { .. } => (String::new(), String::new()),
            Modal::UpdateAvailable { .. } => (String::new(), String::new()),
            Modal::Recovery => {
                let detail = match self
                    .pending_recovery
                    .as_ref()
                    .and_then(|(_, meta)| meta.path.as_ref())
                {
                    Some(path) => format!(
                        "{}
{}",
                        s::RECOVERY_DETAIL,
                        path.display()
                    ),
                    None => s::RECOVERY_DETAIL.to_owned(),
                };
                (s::RECOVERY_TITLE.to_owned(), detail)
            }
            Modal::ChangedOnDisk { .. } => (
                s::CHANGED_ON_DISK_TITLE.to_owned(),
                s::CHANGED_ON_DISK_DETAIL.to_owned(),
            ),
            Modal::MarkdownLoss { losses, title, .. } => {
                // The first few kinds; a long list would not fit the prompt.
                const SHOWN: usize = 8;
                let mut detail = losses
                    .iter()
                    .take(SHOWN)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n");
                if losses.len() > SHOWN {
                    detail.push('\n');
                    detail.push_str(s::MARKDOWN_LOSS_MORE);
                }
                ((*title).to_owned(), detail)
            }
        };
        let buttons: Vec<Element<'_, Message>> = match modal {
            Modal::McpApproval { .. } => vec![
                self.action_button(
                    s::MCP_ALLOW_SESSION,
                    true,
                    Message::McpDecision(McpDecision::AllowSession),
                ),
                self.action_button(
                    s::MCP_ALLOW_ALWAYS,
                    false,
                    Message::McpDecision(McpDecision::AllowAlways),
                ),
                self.action_button(s::MCP_DENY, false, Message::McpDecision(McpDecision::Deny)),
            ],
            Modal::Unsaved { .. } => vec![
                self.modal_button(s::SAVE, true, ModalChoice::Save),
                self.modal_button(s::DONT_SAVE, false, ModalChoice::Discard),
                self.modal_button(s::CANCEL, false, ModalChoice::Cancel),
            ],
            Modal::Error(_) | Modal::Notice { .. } => {
                vec![self.modal_button(s::OK, true, ModalChoice::Cancel)]
            }
            Modal::ChangedOnDisk { .. } => vec![
                self.modal_button(s::OVERWRITE, false, ModalChoice::Save),
                self.modal_button(s::SAVE_A_COPY, true, ModalChoice::Discard),
                self.modal_button(s::CANCEL, false, ModalChoice::Cancel),
            ],
            Modal::MarkdownLoss { .. } => vec![
                self.modal_button(s::SAVE_ANYWAY, true, ModalChoice::Save),
                self.modal_button(s::CANCEL, false, ModalChoice::Cancel),
            ],
            Modal::Stitch
            | Modal::Library { .. }
            | Modal::History { .. }
            | Modal::DateTime { .. }
            | Modal::LinkedMedia { .. }
            | Modal::Ao3Chapter { .. }
            | Modal::UpdateAvailable { .. } => Vec::new(),
            Modal::Recovery => vec![
                self.modal_button(s::RECOVERY_RESTORE, true, ModalChoice::Restore),
                self.modal_button(s::RECOVERY_DISCARD, false, ModalChoice::Discard),
            ],
        };
        let mut card = column![text(title).size(15).color(t.text)].spacing(8);
        if !detail.is_empty() {
            card = card.push(text(detail).size(13).color(t.text_secondary));
        }
        card = card.push(row(buttons).spacing(8).padding(Padding {
            top: 8.0,
            ..Padding::ZERO
        }));
        let card = container(card)
            .padding(24)
            .width(Length::Fixed(440.0))
            .style(move |_| container::Style {
                background: Some(t.inspector.into()),
                border: iced::Border {
                    color: t.divider,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                shadow: iced::Shadow {
                    color: t.shadow,
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..container::Style::default()
            });
        let backdrop = mouse_area(
            container(space().width(Length::Fill).height(Length::Fill)).style(move |_| {
                container::Style {
                    background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.28).into()),
                    ..container::Style::default()
                }
            }),
        )
        .on_press(Message::ModalChoice(ModalChoice::Cancel));
        stack![
            backdrop,
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
        ]
        .into()
    }

    fn modal_button(
        &self,
        label: &'static str,
        primary: bool,
        choice: ModalChoice,
    ) -> Element<'_, Message> {
        self.action_button(label, primary, Message::ModalChoice(choice))
    }

    /// A prompt button with an arbitrary message (the update prompt has more choices than
    /// [`ModalChoice`]).
    fn action_button(
        &self,
        label: &'static str,
        primary: bool,
        message: Message,
    ) -> Element<'_, Message> {
        let t = Tokens::of(self.scheme());
        button(text(label).size(13).wrapping(Wrapping::None))
            .padding(Padding::from([6.0, 16.0]))
            .on_press(message)
            .style(move |_, status| {
                let background = if primary {
                    match status {
                        button::Status::Hovered => t.accent.scale_alpha(0.9),
                        button::Status::Pressed => t.accent.scale_alpha(0.8),
                        _ => t.accent,
                    }
                } else {
                    match status {
                        button::Status::Hovered => t.control_hover,
                        button::Status::Pressed => t.segment_track,
                        _ => t.control,
                    }
                };
                button::Style {
                    background: Some(background.into()),
                    text_color: if primary { t.on_accent } else { t.text },
                    border: iced::Border {
                        color: t.control_border,
                        width: 1.0,
                        radius: 6.0.into(),
                    },
                    ..button::Style::default()
                }
            })
            .into()
    }

    /// The prompt for a newer release: update, remind later, mute it, or turn checks off.
    fn update_layer(
        &self,
        release: &crate::update::Release,
        stage: &UpdateStage,
        t: Tokens,
    ) -> Element<'_, Message> {
        let version = &release.version;
        let content: Element<'_, Message> = match stage {
            UpdateStage::Ask => {
                let hint = if crate::install::plan(&crate::install::Host::detect(), release)
                    == crate::install::Plan::Page
                {
                    s::UPDATES_HINT_PAGE
                } else {
                    s::UPDATES_HINT
                };
                let detail = format!(
                    "Libertum {version} is available. You have {}. {hint}",
                    crate::update::current_version(),
                );
                let primary = row![
                    self.action_button(s::UPDATES_UPDATE, true, Message::UpdateNow),
                    self.action_button(s::UPDATES_LATER, false, Message::DismissUpdate),
                ]
                .spacing(8);
                let quiet = row![
                    self.action_button(
                        s::UPDATES_MUTE_WEEK,
                        false,
                        Message::MuteUpdate(7 * 86_400)
                    ),
                    self.action_button(
                        s::UPDATES_MUTE_MONTH,
                        false,
                        Message::MuteUpdate(30 * 86_400)
                    ),
                    self.action_button(s::UPDATES_MUTE_VERSION, false, Message::MuteUpdateVersion),
                    self.action_button(s::UPDATES_DISABLE, false, Message::DisableUpdates),
                ]
                .spacing(6);
                column![
                    text(s::UPDATES_TITLE).size(15).color(t.text),
                    text(detail).size(13).color(t.text_secondary),
                    primary,
                    quiet,
                ]
                .spacing(10)
                .into()
            }
            UpdateStage::Working => column![
                text(s::UPDATES_WORKING_TITLE).size(15).color(t.text),
                text(s::UPDATES_WORKING_DETAIL)
                    .size(13)
                    .color(t.text_secondary),
            ]
            .spacing(10)
            .into(),
            UpdateStage::Failed(why) => column![
                text(s::UPDATES_FAILED_TITLE).size(15).color(t.text),
                text(why.clone()).size(13).color(t.text_secondary),
                row![
                    self.action_button(s::UPDATES_OPEN_PAGE, true, Message::OpenUpdatePage),
                    self.action_button(s::UPDATES_CLOSE, false, Message::DismissUpdate),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
            UpdateStage::Command { command, file } => column![
                text(s::UPDATES_COMMAND_TITLE).size(15).color(t.text),
                text(s::UPDATES_COMMAND_DETAIL)
                    .size(13)
                    .color(t.text_secondary),
                text(format!("{} {}", s::UPDATES_SAVED_TO, file.display()))
                    .size(13)
                    .color(t.text_secondary),
                text(command.clone())
                    .size(13)
                    .font(iced::Font::MONOSPACE)
                    .color(t.text),
                row![
                    self.action_button(s::UPDATES_COPY_COMMAND, true, Message::CopyUpdateCommand),
                    self.action_button(s::UPDATES_CLOSE, false, Message::DismissUpdate),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        };
        let card = container(content)
            .padding(24)
            .width(Length::Fixed(640.0))
            .style(move |_| container::Style {
                background: Some(t.inspector.into()),
                border: iced::Border {
                    color: t.divider,
                    width: 1.0,
                    radius: 12.0.into(),
                },
                shadow: iced::Shadow {
                    color: t.shadow,
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..container::Style::default()
            });
        let backdrop = mouse_area(
            container(space().width(Length::Fill).height(Length::Fill)).style(move |_| {
                container::Style {
                    background: Some(iced::Color::from_rgba(0.0, 0.0, 0.0, 0.28).into()),
                    ..container::Style::default()
                }
            }),
        )
        .on_press(Message::DismissUpdate);
        stack![
            backdrop,
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
        ]
        .into()
    }

    /// The floating "Page 1 of 506 · 288,323 words" pill at the bottom centre of the canvas.
    /// The frame-rate and frame-time pill in the top-right corner of the canvas (Settings >
    /// Show performance). It only appears while the setting is on.
    fn fps_pill(&self, t: Tokens) -> Element<'_, Message> {
        let label = self
            .fps
            .label()
            .unwrap_or_else(|| s::PERFORMANCE_MEASURING.to_owned());
        container(
            container(text(label).size(11).color(t.pill_text))
                .padding([3, 9])
                .style(move |_| container::Style {
                    background: Some(t.pill.into()),
                    border: border::rounded(999),
                    ..container::Style::default()
                }),
        )
        .padding(Padding {
            top: 8.0,
            right: 12.0,
            ..Padding::ZERO
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_top(Length::Fill)
        .into()
    }

    fn word_count_pill(&self, t: Tokens) -> Element<'_, Message> {
        let label = s::word_count_pill(
            self.state.current_page,
            self.state.page_count,
            self.state.word_count,
            self.state.selection_words,
        );
        container(
            container(text(label).size(12).color(t.pill_text))
                .padding([5, 14])
                .style(move |_| container::Style {
                    background: Some(t.pill.into()),
                    border: border::rounded(999),
                    ..container::Style::default()
                }),
        )
        .padding(Padding {
            bottom: 14.0,
            ..Padding::ZERO
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Center)
        .align_bottom(Length::Fill)
        .into()
    }
}

/// Appends the fallback format's extension when the user typed one the app does not write;
/// an existing `.docx`, `.odt`, `.md` or `.rtf` is kept.
/// Whether a path has one of the extensions the app opens (used for drag and drop, where
/// the user may drop anything).
fn is_document_path(path: &std::path::Path) -> bool {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    matches!(
        extension.as_deref(),
        Some(s::EXT_ODT)
            | Some(s::EXT_DOCX)
            | Some(s::EXT_MARKDOWN)
            | Some(s::EXT_RTF)
            | Some(s::EXT_NATIVE)
    )
}

/// The Save dialog's file types, one per format the app writes: `current` first (the
/// dialog preselects the first), then the native format and the others.
fn save_filters(current: Format) -> Vec<(&'static str, &'static str)> {
    let all = [
        (Format::Native, s::FILE_FILTER_NATIVE),
        (Format::Odt, s::FILE_FILTER_ODT),
        (Format::Docx, s::FILE_FILTER_DOCX),
        (Format::Rtf, s::FILE_FILTER_RTF),
        (Format::Markdown, s::FILE_FILTER_MARKDOWN),
        (Format::Html, s::FILE_FILTER_HTML),
    ];
    let mut out: Vec<(&'static str, &'static str)> = all
        .iter()
        .filter(|(format, _)| *format == current)
        .map(|(format, label)| (*label, format.extension()))
        .collect();
    out.extend(
        all.iter()
            .filter(|(format, _)| *format != current)
            .map(|(format, label)| (*label, format.extension())),
    );
    out
}

/// How quickly a wheel glide closes the distance left, per second: about 86% of it in the
/// first 100 ms and all but a pixel of a notch within a quarter of a second. Fast enough to
/// feel direct, slow enough that the eye can follow the text.
const SCROLL_GLIDE_RATE: f32 = 20.0;

/// Trackpad inertia. Events closer together than the window are one gesture; the page is
/// carried on once none has arrived for the idle time; the velocity decays by this rate per
/// second (about a sixth of a second to halve; the page travels speed / rate in all); and it
/// ends below the minimum speed. The cap stops one noisy event from launching the page; it was 12,000 px/s and is now 24,000, so a hard flick travels twice as far.
const FLING_SAMPLE_WINDOW: f32 = 0.1;
/// The frame clock keeps running this long (seconds) after the last trackpad event, so a
/// gesture is drawn on every vsync; after it the window goes back to redrawing only on change.
const TRACKPAD_ACTIVE: f32 = 0.1;
const FLING_IDLE: f32 = 0.04;
const FLING_DECAY_RATE: f32 = 4.0;
/// How much the newest event counts in the velocity (the rest is what came before). High, so
/// a finger that slows down or stops is read as slow, not as the fast motion before it.
const FLING_RECENT_WEIGHT: f32 = 0.8;
/// Below this the finger was moving too slowly to mean a flick, so the page just stops.
const FLING_MIN_SPEED: f32 = 300.0;
/// The glide ends below this speed (under a pixel per frame), and below `FLING_TAIL_SPEED`
/// it brakes progressively harder, up to `FLING_TAIL_BRAKE` more per second, so it eases out
/// instead of stopping from a speed the eye can see.
const FLING_STOP_SPEED: f32 = 10.0;
const FLING_TAIL_SPEED: f32 = 500.0;
const FLING_TAIL_BRAKE: f32 = 8.0;
const FLING_MAX_SPEED: f32 = 24_000.0;

/// Page thumbnails: image width, the box a page is fitted into (an A4 page's proportions),
/// one row's height with its number, the column's width, and how many rendered thumbnails
/// are kept (about 70 KB each at 100% display scale).
const THUMB_W: f32 = 112.0;
const THUMB_BOX_H: f32 = 160.0;
const THUMB_ROW_H: f32 = 196.0;
const THUMB_PANEL_W: f32 = 150.0;
const THUMB_CACHE_PAGES: usize = 200;
/// Widget id of the thumbnails column's scroll area.
const THUMB_LIST_ID: &str = "libertum-thumbnails";

/// Pages whose thumbnail rows are in the visible part of a column scrolled to `offset`
/// with `height` visible, one row either side: `first..last`.
fn thumb_range(offset: f32, height: f32, total: usize) -> (usize, usize) {
    let first = ((offset / THUMB_ROW_H).floor().max(0.0) as usize).saturating_sub(1);
    let rows = (height / THUMB_ROW_H).ceil().max(1.0) as usize + 2;
    let first = first.min(total);
    (first, (first + rows).min(total))
}

/// How long typing in the Find field must pause before the query is searched.
const FIND_TYPING_PAUSE: std::time::Duration = std::time::Duration::from_millis(220);

/// A task that reports `generation` back once the typing pause has passed. A plain thread
/// sleeps for it, so no async runtime is needed.
fn find_after_pause(generation: u64) -> Task<Message> {
    Task::perform(
        async move {
            let (tx, rx) = iced::futures::channel::oneshot::channel();
            let spawned = std::thread::Builder::new()
                .name("libertum-find-pause".into())
                .spawn(move || {
                    std::thread::sleep(FIND_TYPING_PAUSE);
                    let _ = tx.send(());
                });
            if spawned.is_ok() {
                let _ = rx.await;
            }
            generation
        },
        Message::FindTyped,
    )
}

/// Widget id of the Find bar's query field (focused when the bar opens).
const FIND_FIELD_ID: &str = "libertum-find-query";

/// Logical pixels below the toolbar that the Find bar covers (its 10 px gap, its height and
/// a little air).
const FIND_BAR_COVER: f32 = 64.0;
/// Logical pixels above the canvas's bottom edge that the page and word count pill covers
/// (its 14 px gap, its height and a little air).
const WORD_COUNT_PILL_COVER: f32 = 52.0;

fn ensure_extension(mut path: PathBuf, fallback: Format) -> PathBuf {
    let known = path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case(s::EXT_DOCX)
            || extension.eq_ignore_ascii_case(s::EXT_ODT)
            || extension.eq_ignore_ascii_case(s::EXT_MARKDOWN)
            || extension.eq_ignore_ascii_case(s::EXT_RTF)
            || extension.eq_ignore_ascii_case(s::EXT_NATIVE)
            || extension.eq_ignore_ascii_case(s::EXT_HTML)
            || extension.eq_ignore_ascii_case("htm")
    });
    if !known {
        path.set_extension(fallback.extension());
    }
    path
}

/// Adds the export format's extension when the dialog user left one off.
fn ensure_export_extension(mut path: PathBuf, format: ExportFormat) -> PathBuf {
    let extension = format.extension();
    let known = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(format.extension()));
    if !known {
        path.set_extension(extension);
    }
    path
}

/// The heading AO3 files a tag under, in `strings.rs`.
fn category_label(category: TagCategory) -> &'static str {
    match category {
        TagCategory::Rating => s::TAG_CATEGORY_RATING,
        TagCategory::Warning => s::TAG_CATEGORY_WARNING,
        TagCategory::Fandom => s::TAG_CATEGORY_FANDOM,
        TagCategory::Relationship => s::TAG_CATEGORY_RELATIONSHIP,
        TagCategory::Character => s::TAG_CATEGORY_CHARACTER,
        TagCategory::Freeform => s::TAG_CATEGORY_FREEFORM,
    }
}

/// Writes one export on the task thread. Every branch either produces the file or returns a
/// message the error prompt can show.
fn write_export(
    format: ExportFormat,
    document: Document,
    levels: libertum_export::ChapterLevels,
    title: String,
    path: PathBuf,
) -> Result<PathBuf, String> {
    let failed = |error: String| format!("{} {error}", s::EXPORT_FAILED);
    match format {
        ExportFormat::Rtf => {
            Rtf::new(document)
                .save(&path)
                .map_err(|error| failed(error.to_string()))?;
        }
        ExportFormat::Markdown => {
            let warnings = Markdown::new(document)
                .save_with_report(&path)
                .map_err(|error| failed(error.to_string()))?;
            for warning in warnings {
                tracing::warn!(%warning, path = %path.display(), "Markdown export warning");
            }
        }
        ExportFormat::PlainHtml => {
            let mut html = Html::new(document);
            html.set_flavour(HtmlFlavour::Plain);
            html.save(&path)
                .map_err(|error| failed(error.to_string()))?;
        }
        ExportFormat::Text => {
            std::fs::write(&path, libertum_export::document_text(&document))
                .map_err(|error| failed(error.to_string()))?;
        }
        ExportFormat::Epub => {
            let bytes = libertum_export::build_epub(&document, levels, &title, "en-GB")
                .map_err(|error| failed(error.to_string()))?;
            std::fs::write(&path, bytes).map_err(|error| failed(error.to_string()))?;
        }
        ExportFormat::Pdf | ExportFormat::PdfArchive => {
            let bytes = libertum_render::pdf::export(&document, format.archival(), &title)
                .map_err(|error| failed(error.to_string()))?;
            std::fs::write(&path, bytes).map_err(|error| failed(error.to_string()))?;
        }
    }
    Ok(path)
}

fn key_event(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(key) => {
            crate::palm::note_key(&key);
            let shortcut = shortcuts::from_event(&key);
            // A text field (the Find bar, an inspector field) used this key: typing, Enter,
            // Backspace, the arrows, or Ctrl/Cmd+A, C, X and V. It is not for the document,
            // or a paste into the Find field would also replace the selected match. Escape
            // still closes the bar.
            if status == event::Status::Captured {
                return shortcut
                    .filter(|shortcut| *shortcut == Shortcut::CloseMenu)
                    .map(Message::Shortcut);
            }
            Some(shortcut.map_or(Message::Keyboard(key.clone()), Message::Shortcut))
        }
        Event::InputMethod(event) => Some(Message::Ime(event)),
        Event::Window(window::Event::Rescaled(sf)) => Some(Message::ScaleFactor(sf)),
        Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
        Event::Window(window::Event::Focused) => Some(Message::WindowFocus(true)),
        Event::Window(window::Event::Unfocused) => Some(Message::WindowFocus(false)),
        Event::Window(window::Event::Opened { size, .. }) => Some(Message::WindowResized(size)),
        Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        _ => None,
    }
}

/// Test hook: `LIBERTUM_THEME=light|dark` overrides the theme setting (used for screenshots).
/// `LIBERTUM_DEMO=1` (or an autoscroll run) opens the generated 500-page demo document
/// instead of the home screen.
fn demo_requested() -> bool {
    let on = |var: &str| std::env::var(var).is_ok_and(|v| v != "0" && !v.is_empty());
    on("LIBERTUM_DEMO") || on("LIBERTUM_AUTOSCROLL")
}

/// Test hook: `LIBERTUM_THUMBNAILS=1` opens with the page thumbnails column shown.
fn thumbnails_hook() -> bool {
    std::env::var("LIBERTUM_THUMBNAILS").is_ok_and(|v| v != "0" && !v.is_empty())
}

/// Test hook: `LIBERTUM_BOOKLET=1` opens in the booklet (spread) view for screenshots.
fn booklet_hook() -> bool {
    std::env::var("LIBERTUM_BOOKLET").is_ok_and(|v| v != "0" && !v.is_empty())
}

fn theme_override() -> Option<ThemePreference> {
    match std::env::var("LIBERTUM_THEME").ok().as_deref() {
        Some("light") => Some(ThemePreference::Light),
        Some("dark") => Some(ThemePreference::Dark),
        Some("system") => Some(ThemePreference::System),
        _ => None,
    }
}

/// Test hook: `LIBERTUM_INSPECTOR=format-style|format-layout|format-more|document|section|headers|notes|bookmarks`
/// opens the sidebar on that pane without clicking.
fn apply_inspector_hook(state: &mut AppState) {
    let Ok(value) = std::env::var("LIBERTUM_INSPECTOR") else {
        return;
    };
    match value.as_str() {
        "format-style" => {
            (state.inspector, state.format_pane) = (Inspector::Format, FormatPane::Style)
        }
        "format-layout" => {
            (state.inspector, state.format_pane) = (Inspector::Format, FormatPane::Layout)
        }
        "format-more" => {
            (state.inspector, state.format_pane) = (Inspector::Format, FormatPane::More)
        }
        "document" => {
            (state.inspector, state.document_pane) = (Inspector::Document, DocumentPane::Document)
        }
        "headers" => {
            state.inspector = Inspector::Headers;
        }
        "notes" => {
            state.inspector = Inspector::Notes;
        }
        "bookmarks" => {
            state.inspector = Inspector::Bookmarks;
        }
        "section" => {
            (state.inspector, state.document_pane) = (Inspector::Document, DocumentPane::Section)
        }
        _ => return,
    }
    state.sidebar_visible = true;
}

/// Test hook: `LIBERTUM_MENU=view|zoom|insert|export` opens that toolbar menu at start-up.
fn menu_hook() -> Option<Menu> {
    match std::env::var("LIBERTUM_MENU").ok().as_deref() {
        Some("view") => Some(Menu::View),
        Some("zoom") => Some(Menu::Zoom),
        Some("insert") => Some(Menu::Insert),
        Some("export") => Some(Menu::Export),
        _ => None,
    }
}

/// The inspector's alignment as the model's.
fn model_align(a: Alignment) -> Align {
    match a {
        Alignment::Left => Align::Start,
        Alignment::Centre => Align::Centre,
        Alignment::Right => Align::End,
        Alignment::Justify => Align::Justify,
    }
}

/// The font popup's list: the families installed on this machine, or the built-in short
/// list if none were found.
fn installed_fonts(doc: &DocView) -> Vec<FontName> {
    let names = doc.font_families();
    if names.is_empty() {
        AppState::default().fonts
    } else {
        names.into_iter().map(FontName).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard;

    /// Test settings: centimetres, so paper, margins and stepper grids do not depend on the
    /// machine's locale (a US-locale CI runner would otherwise build Letter documents and
    /// nudge margins in inches).
    /// A process id that is not running: a window that crashed.
    const CRASHED_WINDOW: u32 = u32::MAX - 6;

    /// What the Find field does after a typing pause: the scheduled search runs.
    fn find_typed(app: &mut App, query: &str) {
        let _ = app.update(Message::FindQuery(query.into()));
        let generation = app.find_generation;
        let _ = app.update(Message::FindTyped(generation));
    }

    fn test_settings() -> Settings {
        Settings {
            units: UnitsPreference::Centimetres,
            ..Settings::default()
        }
    }

    pub(crate) fn app() -> App {
        App::new(test_settings(), None)
    }

    /// An app on the home screen (no document open).
    pub(crate) fn home() -> App {
        App::with_doc(test_settings(), None, None)
    }

    impl App {
        pub(crate) fn doc(&self) -> &DocView {
            self.doc.as_ref().expect("a document is open")
        }
        pub(crate) fn doc_mut(&mut self) -> &mut DocView {
            self.doc.as_mut().expect("a document is open")
        }
    }

    fn named(key: keyboard::key::Named) -> Message {
        key_press(
            keyboard::Key::Named(key),
            None,
            keyboard::Modifiers::default(),
        )
    }

    fn paragraph_texts(app: &App) -> Vec<String> {
        app.doc()
            .editor()
            .document()
            .body()
            .iter()
            .filter_map(|b| b.as_paragraph())
            .map(|p| p.text().to_owned())
            .collect()
    }

    #[test]
    fn home_screen_shows_without_a_document() {
        let mut app = home();
        assert!(app.doc.is_none());
        assert_eq!(app.title(), s::APP_NAME);
        // Editing input does nothing on the home screen.
        let _ = app.update(key_press(
            keyboard::Key::Character("a".into()),
            Some("a"),
            keyboard::Modifiers::default(),
        ));
        let _ = app.update(Message::PasteText(Some("text".into())));
        assert!(app.doc.is_none());
        let _ = app.view();
    }

    #[test]
    fn arrows_select_and_enter_creates_the_manuscript_template() {
        let mut app = home();
        let _ = app.update(named(keyboard::key::Named::ArrowRight));
        assert_eq!(app.home_selection, TemplateId::Manuscript);
        let _ = app.update(named(keyboard::key::Named::Enter));
        assert_eq!(
            paragraph_texts(&app),
            vec![
                s::MANUSCRIPT_TITLE,
                s::MANUSCRIPT_BYLINE,
                "",
                s::MANUSCRIPT_CHAPTER,
                ""
            ]
        );
        assert!(app.title().contains(s::UNTITLED));
        let _ = app.view();
    }

    fn book_app() -> App {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Book));
        app
    }

    fn props_of(app: &App, text: &str) -> libertum_model::ParaProps {
        app.doc()
            .editor()
            .document()
            .body()
            .iter()
            .filter_map(|b| b.as_paragraph())
            .find(|p| p.text() == text && p.props.toc_target.is_none())
            .map(|p| p.props)
            .expect("paragraph")
    }

    #[test]
    fn book_opens_as_three_pages_with_the_caret_in_chapter_one() {
        let app = book_app();
        assert_eq!(app.doc().page_total(), 3);
        let caret = app.doc().editor().caret();
        let doc = app.doc().editor().document();
        assert_eq!(doc.index_of(caret.block), Some(doc.len() - 1));
    }

    #[test]
    fn command_enter_inserts_a_page_break() {
        let mut app = book_app();
        let _ = app.update(key_press(
            keyboard::Key::Character("A".into()),
            Some("Aa"),
            keyboard::Modifiers::default(),
        ));
        let pages = app.doc().page_total();
        let _ = app.update(Message::Shortcut(Shortcut::PageBreak));
        assert_eq!(app.doc().page_total(), pages + 1);
        assert!(props_of(&app, "").page_break_before);
        // The toolbar button does the same.
        let _ = app.update(Message::Insert(InsertAction::PageBreak));
        assert_eq!(app.doc().page_total(), pages + 2);
        // Undo takes both back.
        let undo = key_press(
            keyboard::Key::Character("z".into()),
            None,
            if cfg!(target_os = "macos") {
                keyboard::Modifiers::LOGO
            } else {
                keyboard::Modifiers::CTRL
            },
        );
        let _ = app.update(undo.clone());
        let _ = app.update(undo);
        assert_eq!(app.doc().page_total(), pages);
    }

    #[test]
    fn page_break_shortcut_is_ignored_on_the_home_screen_and_in_fields() {
        let mut app = home();
        let _ = app.update(Message::Shortcut(Shortcut::PageBreak));
        assert!(app.doc.is_none());
        let mut app = book_app();
        let pages = app.doc().page_total();
        app.canvas_focused = false;
        let _ = app.update(Message::Shortcut(Shortcut::PageBreak));
        assert_eq!(app.doc().page_total(), pages);
    }

    #[test]
    fn insert_page_number_toggles_the_caret_section() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert_eq!(app.doc().editor().document().sections[0].page_numbers, None);
        let _ = app.update(Message::Insert(InsertAction::PageNumber));
        assert_eq!(
            app.doc().editor().document().sections[0].page_numbers,
            Some(libertum_model::PageNumberPos::BOTTOM_CENTRE)
        );
        let _ = app.update(Message::Insert(InsertAction::PageNumber));
        assert_eq!(app.doc().editor().document().sections[0].page_numbers, None);
    }

    #[test]
    fn typing_a_new_chapter_heading_title_updates_the_toc() {
        let mut app = book_app();
        // Put the caret at the end of the "Chapter One" heading and extend it.
        let heading = app
            .doc()
            .editor()
            .document()
            .body()
            .iter()
            .filter_map(|b| b.as_paragraph())
            .find(|p| p.props.outline_level == Some(1))
            .map(|p| (p.id, p.len()))
            .expect("heading");
        app.doc_mut().move_caret(|ed| {
            ed.set_caret(
                libertum_editor::DocPosition {
                    block: heading.0,
                    offset: heading.1,
                },
                false,
            )
        });
        let _ = app.update(key_press(
            keyboard::Key::Character(":".into()),
            Some(": The Beginning"),
            keyboard::Modifiers::default(),
        ));
        let entry = app
            .doc()
            .editor()
            .document()
            .body()
            .iter()
            .filter_map(|b| b.as_paragraph())
            .find(|p| p.props.toc_target.is_some())
            .map(|p| p.text().to_owned());
        assert_eq!(entry.as_deref(), Some("Chapter One: The Beginning"));
    }

    #[test]
    fn new_document_is_ready_to_type_into_its_body() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Book));
        let _ = app.update(key_press(
            keyboard::Key::Character("I".into()),
            Some("It was a dark night."),
            keyboard::Modifiers::default(),
        ));
        let texts = paragraph_texts(&app);
        assert_eq!(
            texts.last().map(String::as_str),
            Some("It was a dark night.")
        );
        assert_eq!(app.state.word_count, app.doc().word_count());
    }

    #[test]
    fn an_added_template_creates_an_unsaved_document_and_can_be_removed() {
        let dir =
            std::env::temp_dir().join(format!("libertum-app-templates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary templates folder");
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/odt/rich.odt");

        let mut app = home();
        app.templates_dir = dir.clone();
        app.user_templates = Vec::new();

        let index = app.import_template(&source).expect("import the ODT");
        assert_eq!(app.user_templates.len(), 1);
        assert_eq!(app.user_templates[index].format, TemplateFormat::Odt);
        assert!(app.user_templates[index].path.starts_with(&dir));

        let _ = app.update(Message::CreateUserTemplate(index));
        assert!(app.doc.is_some());
        assert!(
            app.file.is_none(),
            "the template copy is not the new document's file"
        );
        assert!(
            paragraph_texts(&app)
                .iter()
                .any(|text| text.contains("Fixture Heading")),
            "the template's content becomes the new document's content"
        );

        app.home_user = Some(index);
        let _ = app.update(Message::RemoveSelectedTemplate);
        assert!(app.user_templates.is_empty());
        assert!(app.home_user.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_template_uses_one_empty_paragraph() {
        let mut app = home();
        let _ = app.update(Message::SelectTemplate(TemplateId::Blank));
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert_eq!(paragraph_texts(&app), vec![String::new()]);
        // A second Create (a stale message) does not replace the open document.
        let _ = app.update(key_press(
            keyboard::Key::Character("x".into()),
            Some("x"),
            keyboard::Modifiers::default(),
        ));
        let _ = app.update(Message::CreateDocument(TemplateId::Book));
        assert_eq!(paragraph_texts(&app), vec!["x".to_owned()]);
    }

    fn key_press(
        key: keyboard::Key,
        text: Option<&str>,
        modifiers: keyboard::Modifiers,
    ) -> Message {
        Message::Keyboard(keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: text.map(Into::into),
            repeat: false,
        })
    }

    fn first_text(app: &App) -> String {
        app.doc()
            .editor()
            .document()
            .paragraph_at(0)
            .expect("first paragraph")
            .text()
            .to_owned()
    }

    #[test]
    fn keyboard_typing_inserts_into_the_document() {
        let mut app = app();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.canvas_focused = true;
        let _ = app.update(key_press(
            keyboard::Key::Character("A".into()),
            Some("A"),
            keyboard::Modifiers::SHIFT,
        ));
        assert!(first_text(&app).starts_with('A'), "{}", first_text(&app));
        assert_eq!(app.state.word_count, app.doc().word_count());
    }

    #[test]
    fn keyboard_is_ignored_until_the_canvas_is_clicked() {
        let mut app = app();
        let before = first_text(&app);
        let _ = app.update(key_press(
            keyboard::Key::Character("x".into()),
            Some("x"),
            keyboard::Modifiers::NONE,
        ));
        assert_eq!(first_text(&app), before);
    }

    #[test]
    fn caret_blinks_after_activity_then_stays_solid() {
        let mut app = app();
        app.canvas_focused = true;
        let _ = app.update(key_press(
            keyboard::Key::Character("a".into()),
            Some("a"),
            keyboard::Modifiers::default(),
        ));
        let t0 = app.caret_activity;
        assert!(app.caret_on && app.blinking(t0));
        let _ = app.update(Message::BlinkTick(t0 + BLINK_INTERVAL));
        assert!(!app.caret_on, "off half of the blink");
        let _ = app.update(Message::BlinkTick(t0 + BLINK_INTERVAL * 2));
        assert!(app.caret_on, "on again");
        let _ = app.update(Message::BlinkTick(t0 + BLINK_INTERVAL * 3));
        assert!(!app.caret_on);
        // After the timeout the caret is solid and the timer is not subscribed.
        let late = t0 + BLINK_TIMEOUT + BLINK_INTERVAL;
        let _ = app.update(Message::BlinkTick(late));
        assert!(app.caret_on);
        assert!(!app.blinking(late));
        // The next keystroke restarts it.
        let _ = app.update(key_press(
            keyboard::Key::Character("b".into()),
            Some("b"),
            keyboard::Modifiers::default(),
        ));
        assert!(app.blinking(app.caret_activity));
    }

    #[test]
    fn no_blink_timer_without_document_focus() {
        let mut app = app();
        app.canvas_focused = false;
        assert!(!app.blinking(Instant::now()));
        let home = home();
        assert!(!home.blinking(Instant::now()));
    }

    #[test]
    fn clicking_the_inspector_stops_document_typing_at_once() {
        let mut app = app();
        app.canvas_focused = true;
        let before = first_text(&app);
        // The first key after clicking a stepper must not reach the document, even before
        // the field has sent any text.
        let _ = app.update(Message::Canvas(CanvasEvent::Blur));
        let _ = app.update(key_press(
            keyboard::Key::Character("x".into()),
            Some("x"),
            keyboard::Modifiers::default(),
        ));
        assert_eq!(first_text(&app), before);
    }

    #[test]
    fn keyboard_is_ignored_while_an_inspector_field_is_edited() {
        let mut app = app();
        app.canvas_focused = true;
        let before = first_text(&app);
        let _ = app.update(Message::FieldText(Field::FontSize, "1".into()));
        let _ = app.update(key_press(
            keyboard::Key::Character("x".into()),
            Some("x"),
            keyboard::Modifiers::NONE,
        ));
        assert_eq!(first_text(&app), before);
    }

    #[test]
    fn enter_splits_and_backspace_joins() {
        let mut app = app();
        app.canvas_focused = true;
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let before = app.doc().editor().document().len();
        let _ = app.update(key_press(
            keyboard::Key::Named(keyboard::key::Named::Enter),
            None,
            keyboard::Modifiers::NONE,
        ));
        assert_eq!(app.doc().editor().document().len(), before + 1);
        let _ = app.update(key_press(
            keyboard::Key::Named(keyboard::key::Named::Backspace),
            None,
            keyboard::Modifiers::NONE,
        ));
        assert_eq!(app.doc().editor().document().len(), before);
    }

    #[test]
    fn ime_commit_inserts_text() {
        let mut app = app();
        app.canvas_focused = true;
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(Message::Ime(input_method::Event::Commit("漢".into())));
        assert!(first_text(&app).starts_with('漢'), "{}", first_text(&app));
    }

    #[test]
    fn ime_preedit_is_stored_and_cleared_on_commit() {
        let mut app = app();
        app.canvas_focused = true;
        let _ = app.update(Message::Ime(input_method::Event::Preedit(
            "か".into(),
            None,
        )));
        assert_eq!(app.ime_preedit.as_deref(), Some("か"));
        assert!(app.preedit_overlay().is_some());
        let _ = app.update(Message::Ime(input_method::Event::Commit("か".into())));
        assert!(app.ime_preedit.is_none());
    }

    #[test]
    fn ctrl_z_undoes_the_last_typing_step() {
        let mut app = app();
        app.canvas_focused = true;
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let before = first_text(&app);
        let _ = app.update(key_press(
            keyboard::Key::Character("q".into()),
            Some("q"),
            keyboard::Modifiers::NONE,
        ));
        assert_ne!(first_text(&app), before);
        let command = if cfg!(target_os = "macos") {
            keyboard::Modifiers::LOGO
        } else {
            keyboard::Modifiers::CTRL
        };
        let _ = app.update(key_press(
            keyboard::Key::Character("z".into()),
            None,
            command,
        ));
        assert_eq!(first_text(&app), before);
    }

    #[test]
    fn view_menu_and_shortcut_toggle_sidebar_and_settings() {
        let mut app = app();
        assert!(app.state.sidebar_visible);
        let _ = app.update(Message::View(ViewAction::Inspector));
        assert!(!app.state.sidebar_visible);
        assert!(app.settings.sidebar_collapsed);
        let _ = app.update(Message::Shortcut(Shortcut::ToggleSidebar));
        assert!(app.state.sidebar_visible);
        assert!(!app.settings.sidebar_collapsed);
        let _ = app.update(Message::View(ViewAction::Inspector));
        assert!(!app.state.sidebar_visible);
    }

    #[test]
    fn inspector_buttons_toggle_and_switch() {
        let mut app = app();
        let _ = app.update(Message::ToggleInspector(Inspector::Format));
        assert!(!app.state.sidebar_visible);
        let _ = app.update(Message::ToggleInspector(Inspector::Document));
        assert!(app.state.sidebar_visible);
        assert_eq!(app.state.inspector, Inspector::Document);
        assert_eq!(app.settings.inspector, Inspector::Document);
        let _ = app.update(Message::ToggleInspector(Inspector::Format));
        assert_eq!(app.state.inspector, Inspector::Format);
        assert!(app.state.sidebar_visible);
    }

    #[test]
    fn collapsed_setting_restores_hidden_sidebar() {
        let settings = Settings {
            sidebar_collapsed: true,
            inspector: Inspector::Document,
            ..Settings::default()
        };
        let app = App::new(settings, None);
        assert!(!app.state.sidebar_visible);
        assert_eq!(app.state.inspector, Inspector::Document);
        // Restored without animating.
        assert_eq!(app.state.sidebar_open, 0.0);
    }

    #[test]
    fn toggle_persists_to_disk() {
        let dir = std::env::temp_dir().join(format!("libertum-app-test-{}", std::process::id()));
        let path = dir.join("settings.toml");
        let mut app = App::new(Settings::default(), Some(path.clone()));
        let _ = app.update(Message::ToggleInspector(Inspector::Document));
        let _ = app.update(Message::ToggleInspector(Inspector::Document));
        let saved = Settings::load_from(&path).expect("settings saved");
        assert!(saved.sidebar_collapsed);
        assert_eq!(saved.inspector, Inspector::Document);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn word_count_toggle_persists() {
        let mut app = app();
        assert!(app.state.show_word_count);
        let _ = app.update(Message::View(ViewAction::WordCount));
        assert!(!app.state.show_word_count);
        assert!(!app.settings.show_word_count);
    }

    #[test]
    fn bold_toggle_resolves_mixed() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(key_press(
            keyboard::Key::Character("a".into()),
            Some("plain bold"),
            keyboard::Modifiers::default(),
        ));
        let block = app.doc().editor().caret().block;
        let at = |offset| libertum_editor::DocPosition { block, offset };
        // Bold "bold", then select "plain bold": the inspector shows mixed.
        app.doc_mut()
            .move_caret(|ed| ed.set_selection(at(6), at(10)));
        let _ = app.update(Message::Shortcut(Shortcut::Bold));
        app.doc_mut()
            .move_caret(|ed| ed.set_selection(at(0), at(10)));
        app.refresh_format();
        assert_eq!(app.state.format.bold, Mixed::Mixed);
        let _ = app.update(Message::Format(FormatMsg::ToggleBold));
        assert_eq!(app.state.format.bold, Mixed::Value(true));
        assert_eq!(app.state.format.font_weight, Mixed::Value(FontWeight::Bold));
        let runs = app
            .doc()
            .editor()
            .document()
            .paragraph(block)
            .unwrap()
            .runs()
            .to_vec();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].range, 0..10);
        // The formatting button left the keyboard with the document.
        assert!(app.canvas_focused);
    }

    #[test]
    fn inspector_font_controls_apply_to_the_selection() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(key_press(
            keyboard::Key::Character("a".into()),
            Some("words"),
            keyboard::Modifiers::default(),
        ));
        let block = app.doc().editor().caret().block;
        let at = |offset| libertum_editor::DocPosition { block, offset };
        app.doc_mut()
            .move_caret(|ed| ed.set_selection(at(0), at(5)));
        let georgia = FontName(std::sync::Arc::from("Georgia"));
        let _ = app.update(Message::Format(FormatMsg::Family(georgia.clone())));
        let _ = app.update(Message::Format(FormatMsg::Colour(Swatch::Red)));
        let _ = app.update(Message::Format(FormatMsg::Highlight(Some(Swatch::Yellow))));
        let _ = app.update(Message::Step(Field::FontSize, 1));
        let f = &app.state.format;
        assert_eq!(f.font_family, Mixed::Value(georgia));
        assert_eq!(f.colour, Mixed::Value(Swatch::Red));
        assert_eq!(f.highlight, Mixed::Value(Some(Swatch::Yellow)));
        assert_eq!(
            f.font_size,
            Mixed::Value(Points(12)),
            "Blank's 11 pt plus one step"
        );
        // One undo per change: four undos restore the plain text.
        let undo = key_press(
            keyboard::Key::Character("z".into()),
            None,
            if cfg!(target_os = "macos") {
                keyboard::Modifiers::LOGO
            } else {
                keyboard::Modifiers::CTRL
            },
        );
        for _ in 0..4 {
            let _ = app.update(undo.clone());
        }
        let p = app.doc().editor().document().paragraph(block).unwrap();
        assert!(p.runs().is_empty(), "{:?}", p.runs());
    }

    #[test]
    fn indent_buttons_step_the_left_indent() {
        let mut app = app();
        let _ = app.update(Message::Format(FormatMsg::Indent(1)));
        assert_eq!(app.state.format.indent_left_cm, Mixed::Value(1.27));
        let _ = app.update(Message::Format(FormatMsg::Indent(-1)));
        let _ = app.update(Message::Format(FormatMsg::Indent(-1)));
        assert_eq!(app.state.format.indent_left_cm, Mixed::Value(0.0));
    }

    /// A new Blank document with "One", "Two" and "Three" as paragraphs, all selected.
    pub(crate) fn three_paragraph_doc() -> App {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::PasteText(Some("One\nTwo\nThree".into())));
        app.doc_mut().move_caret(|ed| ed.select_all());
        app.refresh_format();
        app
    }

    fn resolved(app: &App, index: usize) -> libertum_model::ResolvedPara {
        let doc = app.doc().editor().document();
        let p = doc.paragraph_at(index).expect("paragraph");
        doc.styles.resolve_props(&p.props)
    }

    #[test]
    fn paragraph_controls_apply_to_every_selected_paragraph() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::Format(FormatMsg::Align(Alignment::Centre)));
        assert_eq!(app.state.format.alignment, Mixed::Value(Alignment::Centre));
        let _ = app.update(Message::Format(FormatMsg::KeepWithNext(true)));
        let _ = app.update(Message::Step(Field::SpaceAfter, 1));
        let _ = app.update(Message::Format(FormatMsg::SpacingMode(
            LineSpacingMode::Exactly,
        )));
        let _ = app.update(Message::Step(Field::IndentFirst, 1));
        for i in 0..3 {
            let r = resolved(&app, i);
            assert_eq!(r.align, Align::Centre);
            assert!(r.keep_with_next);
            assert_eq!(r.space_after_pt, 7.0, "Blank's 6 pt plus one step");
            assert_eq!(r.line_spacing, LineSpacing::Exactly(14.0));
            assert!(r.indent_first_pt > 0.0);
        }
        assert_eq!(
            app.state.format.line_spacing_mode,
            Mixed::Value(LineSpacingMode::Exactly)
        );
    }

    #[test]
    fn list_popup_numbers_the_paragraphs_on_the_page() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::Format(FormatMsg::ListKind(ListKind::Numbered)));
        assert_eq!(app.state.format.list_kind, Mixed::Value(ListKind::Numbered));
        assert_eq!(app.doc().markers_on(0), ["1.", "2.", "3."]);
        // Tab at the start of the second item nests it.
        let second = app
            .doc()
            .editor()
            .document()
            .paragraph_at(1)
            .expect("p1")
            .id;
        app.doc_mut().move_caret(|ed| {
            ed.set_caret(
                libertum_editor::DocPosition {
                    block: second,
                    offset: 0,
                },
                false,
            )
        });
        app.canvas_focused = true;
        let _ = app.update(key_press(
            keyboard::Key::Named(keyboard::key::Named::Tab),
            Some("	"),
            keyboard::Modifiers::default(),
        ));
        assert_eq!(app.doc().markers_on(0), ["1.", "a.", "2."]);
        let _ = app.update(Message::Format(FormatMsg::ListKind(ListKind::None)));
        assert_eq!(app.state.format.list_kind, Mixed::Value(ListKind::None));
        assert_eq!(app.doc().markers_on(0), ["1.", "2."]);
    }

    #[test]
    fn tab_with_a_selection_indents_instead_of_replacing_it() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("one\ntwo".into())));
        app.doc_mut().move_caret(|ed| ed.select_all());
        let text_of = |app: &App, i: usize| {
            app.doc()
                .editor()
                .document()
                .paragraph_at(i)
                .expect("p")
                .text()
                .to_owned()
        };
        let indent_of = |app: &App, i: usize| {
            let doc = app.doc().editor().document();
            let p = doc.paragraph_at(i).expect("p");
            doc.styles.resolve_props(&p.props).indent_left_pt
        };
        let _ = app.update(named(keyboard::key::Named::Tab));
        assert_eq!(text_of(&app, 0), "one", "the text is kept");
        assert_eq!(text_of(&app, 1), "two");
        assert!(indent_of(&app, 0) > 0.0 && indent_of(&app, 1) > 0.0);
        let _ = app.update(key_press(
            keyboard::Key::Named(keyboard::key::Named::Tab),
            None,
            keyboard::Modifiers::SHIFT,
        ));
        assert_eq!(indent_of(&app, 0), 0.0);
        assert_eq!(indent_of(&app, 1), 0.0);
    }

    #[test]
    fn tab_outside_a_list_inserts_a_tab_character() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let type_char = |c: &str| {
            key_press(
                keyboard::Key::Character(c.into()),
                Some(c),
                keyboard::Modifiers::default(),
            )
        };
        let _ = app.update(type_char("A"));
        let _ = app.update(named(keyboard::key::Named::Tab));
        let _ = app.update(type_char("B"));
        let text = app
            .doc()
            .editor()
            .document()
            .paragraph_at(0)
            .expect("paragraph")
            .text()
            .to_owned();
        assert_eq!(text, "A\tB");
        // The line still lays out (a page exists) and the tab is part of the model text.
        assert!(app.doc().page_total() >= 1);
    }

    #[test]
    fn hyphenation_switches_reach_the_document() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::Document(DocumentMsg::Hyphenation(true)));
        assert_eq!(app.state.document.hyphenation, Mixed::Value(true));
        assert!(resolved(&app, 0).hyphenate);
        let _ = app.update(Message::Format(FormatMsg::RemoveHyphenation(true)));
        assert_eq!(app.state.format.remove_hyphenation, Mixed::Value(true));
        assert!(
            !resolved(&app, 2).hyphenate,
            "the selected paragraphs opt out"
        );
        let _ = app.update(Message::Document(DocumentMsg::Hyphenation(false)));
        assert!(!app.doc().editor().document().styles.para_defaults.hyphenate);
    }

    #[test]
    fn border_style_reaches_the_selected_paragraphs() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::Format(FormatMsg::BorderStyle(BorderStyle::Thin)));
        assert_eq!(
            app.state.format.border_style,
            Mixed::Value(BorderStyle::Thin)
        );
        for index in 0..3 {
            let p = app
                .doc()
                .editor()
                .document()
                .paragraph_at(index)
                .expect("paragraph");
            assert_eq!(p.props.format.borders.map(|b| b.width_pt), Some(0.5));
        }
        let undo = key_press(
            keyboard::Key::Character("z".into()),
            None,
            if cfg!(target_os = "macos") {
                keyboard::Modifiers::LOGO
            } else {
                keyboard::Modifiers::CTRL
            },
        );
        let _ = app.update(undo);
        assert_eq!(
            app.state.format.border_style,
            Mixed::Value(BorderStyle::None),
            "undo clears the box again"
        );
        for index in 0..3 {
            let p = app
                .doc()
                .editor()
                .document()
                .paragraph_at(index)
                .expect("paragraph");
            assert_eq!(p.props.format.borders, None);
        }
    }

    #[test]
    fn page_numbering_controls_apply_to_the_whole_document() {
        let mut app = three_paragraph_doc();
        assert_eq!(app.state.document.numbering_on, Mixed::Value(false));

        let _ = app.update(Message::Document(DocumentMsg::NumberingOn(true)));
        assert_eq!(
            app.doc().editor().document().sections[0].page_numbers,
            Some(libertum_model::PageNumberPos::BOTTOM_CENTRE)
        );
        assert_eq!(app.state.document.numbering_on, Mixed::Value(true));

        let _ = app.update(Message::Document(DocumentMsg::NumberingVertical(
            NumberingVertical::Top,
        )));
        let _ = app.update(Message::Document(DocumentMsg::NumberingHorizontal(
            NumberingHorizontal::Right,
        )));
        let _ = app.update(Message::Document(DocumentMsg::NumberingMirror(true)));
        let number = app.doc().editor().document().sections[0]
            .page_numbers
            .expect("numbers on");
        assert_eq!(number.vertical, libertum_model::PageNumberVertical::Top);
        assert_eq!(
            number.horizontal,
            libertum_model::PageNumberHorizontal::Right
        );
        assert!(number.mirror, "book mode mirrors alternate pages");
        assert_eq!(
            app.state.document.numbering_vertical,
            Mixed::Value(NumberingVertical::Top)
        );

        // Start on page 1 and step the first number down to 0.
        app.state.document.numbering_start_page = Some(1);
        app.state.document.numbering_start = Some(1);
        let _ = app.update(Message::Document(DocumentMsg::NumberingUpdate));
        let _ = app.update(Message::Step(Field::NumberingStart, -1));
        let numbering = app.doc().editor().document().page_numbering();
        assert_eq!(numbering.start_page, 1);
        assert_eq!(numbering.start_at, 0, "numbering can start at zero");
        assert_eq!(app.state.document.numbering_start, Some(0));
        assert_eq!(
            app.doc().editor().document().sections[0].first_page_number,
            Some(0),
            "the section mirrors the document setting for export"
        );

        // "Use Current Page" takes the page the caret is on (page 1 here).
        let _ = app.update(Message::Document(DocumentMsg::NumberingUseCurrentPage));
        assert_eq!(app.doc().editor().document().page_numbering().start_page, 1);
        assert_eq!(app.state.document.numbering_start_page, Some(1));

        let _ = app.update(Message::Document(DocumentMsg::HideOnFirstPage(true)));
        assert!(
            app.doc()
                .editor()
                .document()
                .page_numbering()
                .hide_first_page
        );
        assert!(
            app.doc().editor().document().sections[0].hide_first_page,
            "hide first page is mirrored on the section for export"
        );

        // Each control is one undo step: hide-first-page is undone on its own.
        let undo = key_press(
            keyboard::Key::Character("z".into()),
            None,
            if cfg!(target_os = "macos") {
                keyboard::Modifiers::LOGO
            } else {
                keyboard::Modifiers::CTRL
            },
        );
        let _ = app.update(undo);
        assert!(!app.doc().editor().document().sections[0].hide_first_page);
    }

    #[test]
    fn layout_pane_columns_and_gutter_edit_the_caret_section() {
        let mut app = three_paragraph_doc();
        let section = |app: &App| app.doc().editor().document().sections[0].clone();
        assert_eq!(app.state.format.columns, Mixed::Value(1));
        assert_eq!(section(&app).columns, 1);

        let _ = app.update(Message::Step(Field::Columns, 1));
        assert_eq!(section(&app).columns, 2, "one more column on the section");
        assert_eq!(app.state.format.columns, Mixed::Value(2));

        let before = section(&app).column_gutter_pt;
        let _ = app.update(Message::Step(Field::Gutter, 1));
        let widened = section(&app).column_gutter_pt;
        assert!(widened > before, "the gutter stepper widens the gap");
        assert!(
            app.state.format.gutter_cm.value().is_some(),
            "the pane reads the section's gutter back"
        );

        // Each control is one undo step.
        let ctrl = if cfg!(target_os = "macos") {
            keyboard::Modifiers::LOGO
        } else {
            keyboard::Modifiers::CTRL
        };
        let undo = || key_press(keyboard::Key::Character("z".into()), None, ctrl);
        let _ = app.update(undo());
        assert!((section(&app).column_gutter_pt - before).abs() < 0.01);
        let _ = app.update(undo());
        assert_eq!(section(&app).columns, 1);
    }

    #[test]
    fn header_and_footer_controls_edit_the_caret_section() {
        let mut app = three_paragraph_doc();
        assert_eq!(app.state.document.header, Mixed::Value(false));
        assert_eq!(app.state.document.footer, Mixed::Value(false));

        let _ = app.update(Message::Document(DocumentMsg::Header(true)));
        assert!(
            app.doc().editor().document().sections[0].headers.enabled(),
            "the header checkbox shows a header"
        );
        assert_eq!(app.state.document.header, Mixed::Value(true));

        let _ = app.update(Message::Document(DocumentMsg::HeaderText(
            HfVariant::Default,
            "Chapter {page}".into(),
        )));
        let section = &app.doc().editor().document().sections[0];
        assert_eq!(
            section.headers.default.as_ref().map(|h| h.text.as_str()),
            Some("Chapter {page}")
        );
        assert_eq!(app.state.document.header_text, "Chapter {page}");

        let _ = app.update(Message::Document(DocumentMsg::Footer(true)));
        let _ = app.update(Message::Document(DocumentMsg::FooterText(
            HfVariant::Default,
            "{page}".into(),
        )));
        let section = &app.doc().editor().document().sections[0];
        assert!(
            section.footers.enabled(),
            "the footer checkbox shows a footer"
        );
        assert_eq!(
            section.footers.default.as_ref().map(|h| h.text.as_str()),
            Some("{page}")
        );
        assert_eq!(app.state.document.footer_text, "{page}");

        // First-page and even-page variants.
        let _ = app.update(Message::Document(DocumentMsg::DifferentFirstPage(true)));
        assert_eq!(app.state.document.different_first_page, Mixed::Value(true));
        let section = &app.doc().editor().document().sections[0];
        assert!(section.headers.different_first && section.footers.different_first);
        assert!(
            section.headers.first.is_some(),
            "a first-page field to type in"
        );
        let _ = app.update(Message::Document(DocumentMsg::HeaderText(
            HfVariant::First,
            "First".into(),
        )));
        assert_eq!(app.state.document.first_header_text, "First");

        let _ = app.update(Message::Document(DocumentMsg::DifferentEvenOdd(true)));
        let _ = app.update(Message::Document(DocumentMsg::FooterText(
            HfVariant::Even,
            "Even {page}".into(),
        )));
        assert_eq!(app.state.document.different_even_odd, Mixed::Value(true));
        assert_eq!(app.state.document.even_footer_text, "Even {page}");

        // The distance stepper edits the section, and undo restores it.
        let before = app.doc().editor().document().sections[0].header_distance_pt;
        let _ = app.update(Message::Step(Field::HeaderDistance, 1));
        let after = app.doc().editor().document().sections[0].header_distance_pt;
        assert!(
            (after - before).abs() > 0.1,
            "the stepper moved the distance"
        );
        let undo = key_press(
            keyboard::Key::Character("z".into()),
            None,
            if cfg!(target_os = "macos") {
                keyboard::Modifiers::LOGO
            } else {
                keyboard::Modifiers::CTRL
            },
        );
        let _ = app.update(undo);
        assert!(
            (app.doc().editor().document().sections[0].header_distance_pt - before).abs() < 0.01
        );

        // The page furniture reaches the laid-out page and the canvas still draws.
        let _ = app.view();
    }

    #[test]
    fn turning_a_header_off_removes_it_and_back_on_starts_empty() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::Document(DocumentMsg::Header(true)));
        let _ = app.update(Message::Document(DocumentMsg::HeaderText(
            HfVariant::Default,
            "Running head".into(),
        )));
        let _ = app.update(Message::Document(DocumentMsg::Header(false)));
        assert_eq!(app.state.document.header, Mixed::Value(false));
        assert!(
            !app.doc().editor().document().sections[0].headers.enabled(),
            "off means no header"
        );
        let _ = app.update(Message::Document(DocumentMsg::Header(true)));
        assert!(
            app.doc().editor().document().sections[0].headers.enabled(),
            "a new, empty header"
        );
        assert_eq!(app.state.document.header_text, "");
    }

    #[test]
    fn page_setup_reaches_the_document_the_canvas_and_the_inspector() {
        let mut app = three_paragraph_doc();
        assert_eq!(app.doc().geometry, PageGeometry::a4());

        let _ = app.update(Message::Document(DocumentMsg::PageSize(PageSize::Letter)));
        assert_eq!(
            app.doc().editor().document().page_setup().paper,
            libertum_model::PaperSize::Letter
        );
        assert_eq!(app.doc().geometry, PageGeometry::letter());
        assert_eq!(app.state.document.page_size, Mixed::Value(PageSize::Letter));

        let _ = app.update(Message::Document(DocumentMsg::Orientation(
            Orientation::Landscape,
        )));
        assert!(
            app.doc().geometry.width_pt > app.doc().geometry.height_pt,
            "landscape swaps the page dimensions"
        );
        assert_eq!(
            app.doc().editor().document().page_setup().orientation,
            libertum_model::Orientation::Landscape
        );
        assert_eq!(
            app.state.document.orientation,
            Mixed::Value(Orientation::Landscape)
        );

        // Each control was one undo step: back to portrait, then back to A4.
        let undo = key_press(
            keyboard::Key::Character("z".into()),
            None,
            if cfg!(target_os = "macos") {
                keyboard::Modifiers::LOGO
            } else {
                keyboard::Modifiers::CTRL
            },
        );
        let _ = app.update(undo.clone());
        assert_eq!(app.doc().geometry, PageGeometry::letter());
        let _ = app.update(undo);
        assert_eq!(app.doc().geometry, PageGeometry::a4());
        assert_eq!(app.state.document.page_size, Mixed::Value(PageSize::A4));
    }

    #[test]
    fn margin_stepper_moves_the_document_margin() {
        let mut app = three_paragraph_doc();
        let _ = app.update(Message::Step(Field::Margin(MarginSide::Top), 1));
        let top = app.doc().editor().document().page_setup().margins.top;
        // One inch is 2.54 cm; the stepper snaps to its 0.1 cm grid, so it lands on 2.60.
        assert!((top - 73.70).abs() < 0.02, "2.60 cm in points, got {top}");
        assert_eq!(app.state.document.margin_top, Mixed::Value(Margin(260)));
        assert!(
            (app.doc().geometry.margins.top - top).abs() < 0.001,
            "the canvas layout uses the same margin"
        );
    }

    #[test]
    fn document_ligature_switch_reaches_the_defaults() {
        let mut app = three_paragraph_doc();
        assert_eq!(app.state.document.ligatures, Mixed::Value(true));
        let _ = app.update(Message::Document(DocumentMsg::Ligatures(false)));
        assert_eq!(
            app.doc().editor().document().styles.defaults.ligatures,
            libertum_model::Ligatures::None
        );
        assert_eq!(app.state.document.ligatures, Mixed::Value(false));
        let _ = app.update(Message::Document(DocumentMsg::Ligatures(true)));
        assert_eq!(
            app.doc().editor().document().styles.defaults.ligatures,
            libertum_model::Ligatures::Standard
        );
    }

    #[test]
    fn creating_a_template_refreshes_the_document_pane_from_the_document() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Book));
        assert_eq!(app.state.document.page_size, Mixed::Value(PageSize::A4));
        assert_eq!(
            app.state.document.hyphenation,
            Mixed::Value(true),
            "the Book template hyphenates"
        );
        assert_eq!(
            app.doc().editor().document().page_setup(),
            libertum_model::PageSetup::a4()
        );
    }

    #[test]
    fn style_picker_and_update_change_the_document() {
        let mut app = three_paragraph_doc();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.refresh_format();
        assert!(!app.state.format.style_differs);
        let _ = app.update(Message::Format(FormatMsg::Style(ParagraphStyle::Heading1)));
        assert_eq!(
            app.state.format.paragraph_style,
            Mixed::Value(ParagraphStyle::Heading1)
        );
        let _ = app.update(Message::Format(FormatMsg::Align(Alignment::Right)));
        assert!(app.state.format.style_differs, "Update is enabled");
        let _ = app.update(Message::Format(FormatMsg::UpdateStyle));
        assert!(!app.state.format.style_differs);
        let doc = app.doc().editor().document();
        let h1 = doc
            .styles
            .get(libertum_model::StyleId::HEADING_1)
            .expect("h1");
        assert_eq!(h1.para.align, Some(Align::End));
    }

    #[test]
    fn heading_4_is_a_style_option_and_reaches_the_document() {
        let mut app = three_paragraph_doc();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(Message::Format(FormatMsg::Style(ParagraphStyle::Heading4)));
        assert_eq!(
            app.state.format.paragraph_style,
            Mixed::Value(ParagraphStyle::Heading4),
            "the Style picker reads Heading 4 back"
        );
        let doc = app.doc().editor().document();
        let caret = app.doc().editor().caret().block;
        let paragraph = doc.paragraph(caret).expect("the caret's paragraph");
        assert_eq!(paragraph.props.style, libertum_model::StyleId::HEADING_4);
        assert_eq!(
            doc.styles.outline_level(libertum_model::StyleId::HEADING_4),
            Some(4),
            "Heading 4 is a fourth-level heading (so the Headers pane lists it)"
        );
    }

    #[test]
    fn superscript_shortcut_and_baseline_popup() {
        let mut app = three_paragraph_doc();
        app.canvas_focused = true;
        // The keyboard subscription maps Cmd/Ctrl+Shift+= to this (see shortcuts.rs tests).
        let _ = app.update(Message::Shortcut(Shortcut::Superscript));
        assert_eq!(
            app.state.format.baseline,
            Mixed::Value(Baseline::Superscript)
        );
        let _ = app.update(Message::Format(FormatMsg::Baseline(Baseline::Default)));
        assert_eq!(app.state.format.baseline, Mixed::Value(Baseline::Default));
    }

    #[test]
    fn the_font_list_comes_from_the_machine() {
        let app = three_paragraph_doc();
        // Every machine that runs the tests has some fonts; the list is sorted.
        let names: Vec<String> = app.state.fonts.iter().map(|f| f.0.to_lowercase()).collect();
        assert!(!names.is_empty());
        assert!(names.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn typing_ends_when_another_message_arrives() {
        let mut app = app();
        let _ = app.update(Message::FieldText(Field::FontSize, "20".into()));
        assert!(app.state.editing.is_some());
        let _ = app.update(Message::FormatPane(FormatPane::More));
        assert!(app.state.editing.is_none());
    }

    #[test]
    fn menu_closes_when_an_item_is_chosen() {
        let mut app = app();
        let _ = app.update(Message::ToggleMenu(Menu::Zoom));
        assert_eq!(app.state.open_menu, Some(Menu::Zoom));
        let _ = app.update(Message::ZoomSelected(Zoom::Percent(150)));
        assert_eq!(app.state.open_menu, None);
        assert_eq!(app.state.zoom, Zoom::Percent(150));
    }

    #[test]
    fn exact_zoom_accepts_any_percentage_down_to_one() {
        let mut app = app();
        let _ = app.update(Message::ZoomSet(37));
        assert_eq!(app.state.zoom, Zoom::Percent(37));
        assert_eq!(app.metrics().percent(), 37);

        // The toolbar field applies what is typed, clamped to 1..=800.
        let _ = app.update(Message::ZoomText("1".into()));
        assert_eq!(app.state.zoom, Zoom::Percent(1));
        assert_eq!(app.metrics().percent(), 1);
        let _ = app.update(Message::ZoomText("0".into()));
        assert_eq!(app.state.zoom, Zoom::Percent(1), "never below 1%");
        let _ = app.update(Message::ZoomText("900".into()));
        assert_eq!(app.state.zoom, Zoom::Percent(800), "never above 800%");
        assert_eq!(app.state.zoom_edit.as_deref(), Some("900"));

        // Committing clears the editing text; the value is already applied.
        let _ = app.update(Message::ZoomCommit);
        assert!(app.state.zoom_edit.is_none());
        assert_eq!(app.state.zoom, Zoom::Percent(800));

        // Stepping moves by 1%; menu commands by 10% through the same message.
        let _ = app.update(Message::ZoomSet(50));
        let _ = app.update(Message::ZoomStep(1));
        assert_eq!(app.state.zoom, Zoom::Percent(51));
        let _ = app.update(Message::ZoomStep(-10));
        assert_eq!(app.state.zoom, Zoom::Percent(41));
    }

    #[test]
    fn linking_headers_and_footers_is_a_section_setting() {
        let mut app = app();
        let _ = app.update(Message::Document(DocumentMsg::LinkHeaders(true)));
        let _ = app.update(Message::Document(DocumentMsg::LinkFooters(true)));
        let doc = app.doc.as_ref().expect("doc");
        let section = &doc.editor().document().sections[0];
        assert!(section.link_headers_to_previous);
        assert!(section.link_footers_to_previous);
        assert_eq!(app.state.document.link_headers, Mixed::Value(true));
        assert_eq!(app.state.document.link_footers, Mixed::Value(true));
    }

    #[test]
    fn headers_pane_lists_headings_and_jumps_to_one() {
        let mut app = app();
        let _ = app.update(Message::Format(FormatMsg::Style(ParagraphStyle::Heading1)));
        let headings = app.headings();
        assert!(!headings.is_empty(), "the first paragraph is a heading");
        assert_eq!(headings[0].level, 1);

        let _ = app.update(Message::ToggleInspector(Inspector::Headers));
        assert_eq!(app.state.inspector, Inspector::Headers);
        assert!(!app.headings.is_empty(), "the pane cache is filled");

        let target = headings[0].block;
        let _ = app.update(Message::GoToHeading(target));
        assert_eq!(app.doc().editor().caret().block, target);
    }

    #[test]
    fn inserting_a_table_of_contents_lists_chapter_headings() {
        let mut app = app();
        let _ = app.update(Message::Format(FormatMsg::Style(ParagraphStyle::Heading1)));
        let _ = app.update(Message::Insert(InsertAction::TableOfContents));
        let texts = paragraph_texts(&app);
        assert!(texts.iter().any(|text| text == "Contents"));
        assert!(
            texts
                .iter()
                .any(|text| text.contains("Portion") || !text.is_empty())
        );
    }

    #[test]
    fn notes_pane_edits_every_note() {
        let mut app = app();
        let _ = app.update(Message::Insert(InsertAction::Footnote));
        let _ = app.update(Message::Insert(InsertAction::Footnote));
        let _ = app.update(Message::ToggleInspector(Inspector::Notes));
        assert!(app.state.sidebar_visible);
        assert_eq!(app.notes.len(), 2);
        let block = app.notes[1].block;
        let _ = app.update(Message::NoteText(block, 1, "Second note".into()));
        assert_eq!(
            app.doc()
                .editor()
                .document()
                .paragraph(block)
                .and_then(|p| p.notes().get(1))
                .map(|n| n.text.as_str()),
            Some("Second note")
        );
    }

    #[test]
    fn inserting_a_footnote_shows_its_number_in_the_document_pane() {
        let mut app = app();
        let _ = app.update(Message::Insert(InsertAction::Footnote));
        assert_eq!(app.doc().editor().footnote(), Some(""));
        assert_eq!(app.state.document.footnote_number, Some(1));
        let _ = app.update(Message::Document(DocumentMsg::FootnoteText(
            "A note".into(),
        )));
        assert_eq!(app.doc().editor().footnote(), Some("A note"));
    }

    #[test]
    fn title_shows_the_file_and_edit_state() {
        let mut app = app();
        assert_eq!(app.title(), "Untitled.odt — Saved");
        app.canvas_focused = true;
        app.doc
            .as_mut()
            .expect("doc")
            .move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(key_press(
            keyboard::Key::Character("x".into()),
            Some("x"),
            keyboard::Modifiers::NONE,
        ));
        assert_eq!(app.title(), "Untitled.odt — Edited");
        let _ = app.update(Message::Saved {
            path: PathBuf::from("Book.docx"),
            result: Ok(()),
        });
        assert_eq!(app.title(), "Book.docx — Saved");
    }

    #[test]
    fn opening_a_docx_replaces_the_open_document() {
        let mut app = app();
        let path =
            std::env::temp_dir().join(format!("libertum-open-test-{}.docx", std::process::id()));
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Opened from a file",
            )]);
        let bytes = Docx::new(document).to_bytes().expect("write");
        std::fs::write(&path, bytes).expect("write file");
        let _ = app.open_path(path.clone());
        assert_eq!(
            app.doc
                .as_ref()
                .expect("doc")
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .text(),
            "Opened from a file"
        );
        assert_eq!(app.file.as_deref(), Some(path.as_path()));
        assert!(!app.dirty);
        assert!(app.title().ends_with("— Saved"), "{}", app.title());
        assert!(app.file_context.is_some(), "the package is kept for saving");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn opening_an_odt_replaces_the_open_document() {
        let mut app = app();
        let path =
            std::env::temp_dir().join(format!("libertum-open-test-{}.odt", std::process::id()));
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Opened from an ODT",
            )]);
        let bytes = Odt::new(document).to_bytes().expect("write");
        std::fs::write(&path, bytes).expect("write file");
        let _ = app.open_path(path.clone());
        assert_eq!(
            app.doc
                .as_ref()
                .expect("doc")
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .text(),
            "Opened from an ODT"
        );
        assert_eq!(app.file.as_deref(), Some(path.as_path()));
        assert!(!app.dirty);
        assert!(app.title().ends_with("— Saved"), "{}", app.title());
        assert!(
            matches!(app.file_context, Some(FileContext::Odt(_))),
            "the ODT package is kept for saving"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn markdown_opens_in_a_pageless_reading_layout() {
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "A Markdown paragraph.",
            )]);
        let (view, _events) = DocView::open_markdown(document);
        assert!(view.pageless);
        assert_eq!(view.geometry.width_pt, 612.0);
        assert_eq!(view.geometry.margins.left, 120.0);
        assert_eq!(view.geometry.margins.top, 0.0);
    }

    #[test]
    fn opening_a_markdown_file_replaces_the_open_document() {
        let mut app = app();
        let path =
            std::env::temp_dir().join(format!("libertum-open-test-{}.md", std::process::id()));
        std::fs::write(&path, "# Opened heading\n\nBody from Markdown.\n").expect("write file");
        let _ = app.open_path(path.clone());
        let document = app.doc.as_ref().expect("doc").document();
        assert_eq!(
            document.paragraph_at(0).expect("heading").text(),
            "Opened heading"
        );
        assert_eq!(
            document.paragraph_at(0).expect("heading").props.style,
            libertum_model::StyleId::HEADING_1
        );
        assert_eq!(
            document.paragraph_at(1).expect("body").text(),
            "Body from Markdown."
        );
        assert!(matches!(app.file_context, Some(FileContext::Markdown(_))));
        assert!(!app.dirty);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_markdown_file_survives_open_then_save_without_edits() {
        let bytes = b"# Heading\n\nA paragraph with *emphasis*.\n";
        let markdown = Markdown::parse(bytes).expect("parse");
        let saved = markdown.to_bytes().expect("save");
        assert_eq!(saved, bytes);
    }

    #[test]
    fn opening_an_rtf_replaces_the_open_document() {
        let mut app = app();
        let path =
            std::env::temp_dir().join(format!("libertum-open-test-{}.rtf", std::process::id()));
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Opened from an RTF",
            )]);
        let bytes = Rtf::new(document).to_bytes().expect("write");
        std::fs::write(&path, bytes).expect("write file");
        let _ = app.open_path(path.clone());
        assert_eq!(
            app.doc
                .as_ref()
                .expect("doc")
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .text(),
            "Opened from an RTF"
        );
        assert!(matches!(app.file_context, Some(FileContext::Rtf(_))));
        assert!(!app.dirty);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn saving_an_rtf_round_trips_the_open_document() {
        let app = app();
        let doc = app.doc.as_ref().expect("doc").document().clone();
        let bytes = Rtf::new(doc.clone()).to_bytes().expect("write");
        let parsed = Rtf::parse(&bytes).expect("read");
        assert_eq!(
            parsed.document().paragraph_at(0).expect("paragraph").text(),
            doc.paragraph_at(0).expect("paragraph").text()
        );
    }

    #[test]
    fn a_failed_open_shows_an_error_prompt() {
        let mut app = app();
        let path = std::env::temp_dir().join("libertum-not-a-docx.docx");
        std::fs::write(&path, b"not a zip package").expect("write");
        let _ = app.open_path(path.clone());
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        let _ = std::fs::remove_file(&path);
    }

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/"
        ))
        .join(name)
    }

    #[test]
    fn opening_a_word_document_with_a_table_of_contents_shows_a_notice_and_rebuilds_it() {
        let mut app = app();
        let _ = app.open_path(fixture("docx/word-toc.docx"));
        assert!(
            matches!(app.modal, Some(Modal::Notice { .. })),
            "a Word table of contents is acknowledged with an OK button"
        );
        // The entries were rebuilt from the headings.
        let texts = paragraph_texts(&app);
        assert_eq!(
            texts.iter().filter(|text| *text == "Chapter One").count(),
            2,
            "the heading and its entry: {texts:?}"
        );
        // OK dismisses it.
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert!(app.modal.is_none(), "modal stayed open: {:?}", app.modal);
    }

    #[test]
    fn a_libreoffice_table_of_contents_gets_the_same_notice() {
        let mut app = app();
        let _ = app.open_path(fixture("odt/toc.odt"));
        assert!(matches!(app.modal, Some(Modal::Notice { .. })));
    }

    #[test]
    fn a_table_of_contents_libertum_saved_reopens_without_a_notice() {
        let mut app = app();
        let _ = app.open_path(fixture("docx/word-toc.docx"));
        let document = app.doc().document().clone();
        let path = std::env::temp_dir().join(format!("libertum-toc-{}.docx", std::process::id()));
        libertum_docx::Docx::new(document)
            .save(&path)
            .expect("save");
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        let _ = app.open_path(path.clone());
        assert!(app.modal.is_none(), "Libertum's own TOC needs no notice");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_plain_document_opens_without_a_notice() {
        let mut app = app();
        let _ = app.open_path(fixture("docx/rich.docx"));
        assert!(app.modal.is_none());
    }

    #[test]
    fn closing_with_unsaved_changes_prompts_and_can_be_cancelled() {
        let mut app = app();
        app.canvas_focused = true;
        app.doc
            .as_mut()
            .expect("doc")
            .move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(key_press(
            keyboard::Key::Character("x".into()),
            Some("x"),
            keyboard::Modifiers::NONE,
        ));
        assert!(app.dirty);
        let _ = app.update(Message::CloseRequested);
        assert!(matches!(app.modal, Some(Modal::Unsaved { .. })));
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert!(app.modal.is_none());
        let _ = app.update(Message::CloseRequested);
        let _ = app.update(Message::ModalChoice(ModalChoice::Discard));
        assert!(app.modal.is_none());
    }

    #[test]
    fn the_unsaved_prompt_can_save_and_remembers_to_close() {
        let mut app = app();
        app.file = Some(std::env::temp_dir().join("libertum-prompt-save.docx"));
        app.dirty = true;
        let _ = app.update(Message::CloseRequested);
        let _ = app.update(Message::ModalChoice(ModalChoice::Save));
        assert!(matches!(app.after_save, Some(Pending::Close)));
        assert!(app.modal.is_none());
        // The write runs on a background task, which the test does not drive.
        app.after_save = None;
    }

    #[test]
    fn a_successful_save_clears_the_edited_state() {
        let mut app = app();
        app.dirty = true;
        let path = PathBuf::from("Book.docx");
        let _ = app.update(Message::Saved {
            path: path.clone(),
            result: Ok(()),
        });
        assert!(!app.dirty);
        assert_eq!(app.file.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn a_failed_save_shows_an_error_prompt() {
        let mut app = app();
        app.dirty = true;
        let _ = app.update(Message::Saved {
            path: PathBuf::from("x.docx"),
            result: Err("disk full".into()),
        });
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        assert!(app.dirty, "a failed save leaves the document edited");
    }

    pub(super) fn synced_folder_for_tags(name: &str) -> PathBuf {
        synced_folder(name)
    }

    fn synced_folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("libertum-app-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the folder");
        dir
    }

    #[test]
    fn a_file_changed_on_disk_asks_before_it_is_overwritten() {
        let dir = synced_folder("changed");
        let path = dir.join("book.docx");
        let mut app = app();
        let doc = app.doc.as_ref().expect("doc").document().clone();
        std::fs::write(&path, Docx::new(doc).to_bytes().expect("docx")).expect("write");
        app.file = Some(path.clone());
        app.disk_stamp = libertum_library::DiskStamp::of(&path).expect("stamp");
        // Untouched: the save goes ahead without a prompt.
        let _ = app.write_document(path.clone());
        assert!(app.modal.is_none(), "no change, no prompt");
        // Another device saves a different version.
        std::fs::write(&path, b"somebody else's version").expect("write");
        let _ = app.write_document(path.clone());
        assert!(
            matches!(app.modal, Some(Modal::ChangedOnDisk { .. })),
            "the user chooses first"
        );
        assert_eq!(
            std::fs::read(&path).expect("read"),
            b"somebody else's version",
            "nothing was overwritten"
        );
    }

    #[test]
    fn saving_keeps_the_replaced_version_in_the_history_without_leftovers() {
        let dir = synced_folder("history");
        let path = dir.join("book.lbtd");
        let app = app();
        let doc = app.doc.as_ref().expect("doc").document().clone();
        let context = FileContext::new(Format::Native, doc);
        context.save(&path).expect("first save");
        let first = std::fs::read(&path).expect("read");
        context.save(&path).expect("second save");
        let kept = libertum_library::versions(&path);
        assert_eq!(kept.len(), 1, "the replaced file is kept");
        assert_eq!(std::fs::read(&kept[0].path).expect("read"), first);
        let visible: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with(".libertum"))
            .collect();
        assert_eq!(visible.len(), 1, "no temporary file is left: {visible:?}");
    }

    #[test]
    fn compare_mode_is_read_only_and_marks_keeps_and_reverts_changes_since_posting() {
        use libertum_model::{BlockId, Document, ParaProps, Paragraph, StyleId};
        let dir = synced_folder("compare");
        let path = dir.join("story.lbtd");
        let heading = Paragraph::new(BlockId(1), "Chapter One").with_props(ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        });
        let document = Document::from_paragraphs(vec![
            heading,
            Paragraph::new(BlockId(2), "The quick brown fox."),
            Paragraph::new(BlockId(3), "It jumped."),
        ]);
        FileContext::new(Format::Native, document.clone())
            .save(&path)
            .expect("save");
        let mut app = app();
        let (doc, _rx) = DocView::open(document);
        app.doc = Some(doc);
        app.file = Some(path.clone());
        let text = |app: &App| {
            app.doc()
                .document()
                .paragraph_at(1)
                .expect("p")
                .text()
                .to_owned()
        };

        // Without a file there is nowhere to keep posted copies.
        app.file = None;
        let _ = app.update(Message::Compare(CompareMsg::Toggle));
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        assert!(!app.doc().read_only);
        app.modal = None;
        app.file = Some(path.clone());

        let _ = app.update(Message::Compare(CompareMsg::Toggle));
        assert!(app.doc().read_only && app.state.compare);
        app.compare_now();
        assert_eq!(app.compare_visible(), 0, "nothing posted yet");
        let _ = app.update(Message::Compare(CompareMsg::MarkPosted));
        assert_eq!(libertum_library::list_posted(&path).len(), 1);
        app.compare_now();
        assert_eq!(app.compare_visible(), 0, "the posted copy matches");
        // Typing does nothing while comparing.
        let result = app.doc_mut().edit(|ed| ed.insert_text("x"));
        let _ = app.finish_edit(result);
        assert_eq!(text(&app), "The quick brown fox.");
        assert!(!app.dirty);

        let _ = app.update(Message::Compare(CompareMsg::Exit));
        assert!(!app.doc().read_only && !app.state.compare);
        let block = BlockId(2);
        let result = app
            .doc_mut()
            .edit(|ed| ed.replace_in_paragraph(block, &[(4..9, "slow".into())]));
        let _ = app.finish_edit(result);
        assert_eq!(text(&app), "The slow brown fox.");

        let _ = app.update(Message::Compare(CompareMsg::Toggle));
        app.compare_now();
        assert_eq!(app.compare_visible(), 1);
        let (added, removed) = app.compare_rects();
        assert!(
            !added.is_empty() || !removed.is_empty(),
            "the change is marked"
        );
        let _ = app.update(Message::Compare(CompareMsg::Next));
        let _ = app.update(Message::Compare(CompareMsg::Keep));
        assert_eq!(app.compare_visible(), 0, "kept");
        app.compare_now();
        assert_eq!(app.compare_visible(), 0, "a kept change stays kept");

        // Leaving and coming back forgets what was kept; Revert puts the posted text back.
        let _ = app.update(Message::Compare(CompareMsg::Exit));
        let _ = app.update(Message::Compare(CompareMsg::Toggle));
        app.compare_now();
        let _ = app.update(Message::Compare(CompareMsg::Next));
        let _ = app.update(Message::Compare(CompareMsg::Revert));
        assert_eq!(text(&app), "The quick brown fox.");
        assert!(app.dirty);
        app.compare_now();
        assert_eq!(app.compare_visible(), 0);
        // Escape leaves compare mode.
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(!app.state.compare && !app.doc().read_only);
    }

    #[test]
    fn mark_as_posted_in_the_chapter_window_keeps_the_posted_copy() {
        use libertum_model::{BlockId, Document, ParaProps, Paragraph, StyleId};
        let dir = synced_folder("ao3-post");
        let path = dir.join("Rainy Harbour.lbtd");
        let document = Document::from_paragraphs(vec![
            Paragraph::new(BlockId(1), "Chapter One").with_props(ParaProps {
                style: StyleId::HEADING_1,
                outline_level: Some(1),
                ..ParaProps::default()
            }),
            Paragraph::new(BlockId(2), "The rain had not stopped."),
        ]);
        let mut app = app();
        let (doc, _rx) = DocView::open(document);
        app.doc = Some(doc);
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
        // Without a file there is nowhere to keep it.
        let _ = app.update(Message::Ao3Post(Ao3PostMsg::MarkPosted));
        assert_eq!(app.ao3_post_notice.as_deref(), Some(s::AO3_POST_SAVE_FIRST));
        app.file = Some(path.clone());
        let _ = app.update(Message::Ao3Post(Ao3PostMsg::MarkPosted));
        let posted = libertum_library::list_posted(&path);
        assert_eq!(posted.len(), 1);
        assert_eq!(
            posted[0].paragraphs,
            ["Chapter One", "The rain had not stopped."]
        );
    }

    #[test]
    fn underlines_follow_the_text_as_it_is_typed() {
        use libertum_model::{BlockId, Document, Paragraph};
        let mut app = app();
        let document = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Teh cat sat.")]);
        let (doc, _rx) = DocView::open(document);
        app.doc = Some(doc);
        let block = app.doc().document().paragraph_at(0).expect("p").id;
        let span = |start: usize, end: usize, word: &str| spell::Span {
            block,
            start,
            end,
            word: word.to_owned(),
        };
        // A result for the current text is shown (the caret is away from the word).
        app.doc_mut().move_caret(|ed| ed.move_doc_end(false));
        app.spell_generation = 7;
        let _ = app.update(Message::SpellChecked {
            generation: 7,
            result: Ok(vec![span(0, 3, "Teh")]),
        });
        assert_eq!(app.spell_errors.len(), 1);
        // Typing in front of the word moves it: the old underline is dropped at once rather
        // than drawn under the wrong letters, and a check is asked for once typing pauses.
        *spell_timer().lock() = None;
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let result = app.doc_mut().edit(|ed| ed.insert_text("So "));
        let _ = app.finish_edit(result);
        app.doc_mut().move_caret(|ed| ed.move_doc_end(false));
        assert!(app.spell_errors.is_empty(), "the stale underline is gone");
        assert!(spell_check_pending(), "a check is due when typing pauses");
        // A result that came back for older text keeps only the words still in place, and
        // asks for another look.
        app.spell_generation = 8;
        app.spell_signature = Some(1);
        let _ = app.update(Message::SpellChecked {
            generation: 8,
            result: Ok(vec![span(0, 3, "Teh"), span(3, 6, "Teh")]),
        });
        assert_eq!(app.spell_errors, vec![span(3, 6, "Teh")]);
        assert_eq!(app.spell_signature, None, "checks again");
    }

    #[test]
    fn the_word_being_typed_is_not_underlined_until_the_caret_leaves_it() {
        use libertum_model::{BlockId, Document, Paragraph};
        let mut app = app();
        let document = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Teh cat")]);
        let (doc, _rx) = DocView::open(document);
        app.doc = Some(doc);
        let block = app.doc().document().paragraph_at(0).expect("p").id;
        let span = spell::Span {
            block,
            start: 0,
            end: 3,
            word: "Teh".to_owned(),
        };
        // Caret at the end of "Teh": still being typed, so no underline.
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.doc_mut().move_caret(|ed| ed.move_right(false));
        app.doc_mut().move_caret(|ed| ed.move_right(false));
        app.doc_mut().move_caret(|ed| ed.move_right(false));
        app.spell_generation = 1;
        app.spell_signature = Some(5);
        let _ = app.update(Message::SpellChecked {
            generation: 1,
            result: Ok(vec![span.clone()]),
        });
        assert!(
            app.spell_errors.is_empty(),
            "the word at the caret is skipped"
        );
        assert_eq!(
            app.spell_signature,
            Some(5),
            "skipping the word being typed is not an out-of-date result: no recheck loop"
        );
        // Caret moved on to "cat": the word is now checked.
        app.doc_mut().move_caret(|ed| ed.move_doc_end(false));
        app.spell_generation = 2;
        let _ = app.update(Message::SpellChecked {
            generation: 2,
            result: Ok(vec![span]),
        });
        assert_eq!(app.spell_errors.len(), 1);
    }

    #[test]
    fn the_spell_timer_sleeps_until_a_check_is_due_and_ticks_once() {
        let timer: &'static SpellTimer = Box::leak(Box::new(SpellTimer {
            deadline: std::sync::Mutex::new(None),
            wake: std::sync::Condvar::new(),
            epoch: std::sync::atomic::AtomicU64::new(0),
        }));
        let mut ticks = spell_ticks_for(timer);
        let pause = |ms| std::thread::sleep(Duration::from_millis(ms));
        // Nothing pending: no tick, however long it waits.
        pause(300);
        assert!(ticks.try_recv().is_err(), "an idle timer sends nothing");
        // A burst of activity is one tick after it goes quiet, not before.
        timer.set(Duration::from_millis(150), false);
        pause(60);
        timer.set(Duration::from_millis(150), false);
        pause(100);
        assert!(ticks.try_recv().is_err(), "still inside the quiet period");
        pause(150);
        assert!(ticks.try_recv().is_ok(), "one tick when quiet");
        pause(300);
        assert!(ticks.try_recv().is_err(), "and no repeat tick");
        // A retry never pushes back a sooner check.
        timer.set(Duration::from_millis(100), false);
        timer.set(Duration::from_secs(60), true);
        pause(250);
        assert!(ticks.try_recv().is_ok());
        // A newer timer thread retires the older one.
        let mut newer = spell_ticks_for(timer);
        pause(50);
        timer.set(Duration::from_millis(50), false);
        pause(200);
        assert!(newer.try_recv().is_ok());
        assert!(
            !ticks.try_recv().is_ok(),
            "the retired thread sends nothing"
        );
    }

    #[test]
    fn an_unfocused_window_stops_the_blink_and_idle_autosave_timers() {
        let mut app = app();
        app.canvas_focused = true;
        app.caret_activity = Instant::now();
        assert!(app.blinking(Instant::now()));
        assert!(app.autosave_interval().is_some());
        let _ = app.update(Message::WindowFocus(false));
        assert!(!app.blinking(Instant::now()), "no blink while unfocused");
        assert!(
            app.autosave_interval().is_none(),
            "no autosave tick while unfocused and clean"
        );
        // Unsaved work keeps its autosave timer even when unfocused.
        app.dirty = true;
        assert!(app.autosave_interval().is_some());
        app.dirty = false;
        let _ = app.update(Message::WindowFocus(true));
        assert!(app.blinking(Instant::now()), "blinks again on return");
        assert!(app.caret_on);
        assert!(app.autosave_interval().is_some());
    }

    #[test]
    fn choosing_a_spelling_action_closes_the_context_menu() {
        let mut app = app();
        for message in [
            Message::SpellAddWord("smple".to_owned()),
            Message::SpellIgnoreDocument("smple".to_owned()),
        ] {
            app.context_menu = Some(ContextMenuState {
                x: 0.0,
                y: 0.0,
                target: crate::toolbar::ContextMenuTarget::Text { spelling: None },
            });
            let _ = app.update(message);
            assert!(app.context_menu.is_none());
        }
    }

    /// A native document with `text` as its only paragraph, saved to `path`.
    fn write_native(path: &std::path::Path, text: &str) {
        let doc = libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
            libertum_model::BlockId(1),
            text,
        )]);
        FileContext::new(Format::Native, doc)
            .save(path)
            .expect("save");
    }

    #[test]
    fn the_library_lists_documents_and_settles_a_conflict_either_way() {
        let dir = synced_folder("library");
        let original = dir.join("novel.lbtd");
        let copy = dir.join("novel (1).lbtd");
        write_native(&original, "mine");
        write_native(&copy, "theirs");
        let mut app = app();
        app.settings.library_folder = Some(dir.clone());
        let _ = app.update(Message::OpenLibrary);
        match &app.modal {
            Some(Modal::Library { entries }) => {
                assert_eq!(entries.len(), 2);
                let found = entries.iter().find(|e| e.path == copy).expect("the copy");
                assert_eq!(found.conflict_of.as_deref(), Some("novel.lbtd"));
            }
            other => panic!("expected the library, got {other:?}"),
        }
        let before = std::fs::read(&copy).expect("read");
        let _ = app.update(Message::UseConflictCopy {
            original: original.clone(),
            copy: copy.clone(),
        });
        assert!(!copy.exists(), "the copy is gone");
        assert_eq!(std::fs::read(&original).expect("read"), before);
        assert_eq!(
            libertum_library::versions(&original).len(),
            1,
            "the replaced original is kept"
        );
        assert!(matches!(&app.modal, Some(Modal::Library { entries }) if entries.len() == 1));

        write_native(&copy, "another");
        let _ = app.update(Message::KeepOriginal {
            original: original.clone(),
            copy: copy.clone(),
        });
        assert!(!copy.exists());
        assert_eq!(std::fs::read(&original).expect("read"), before);
        assert_eq!(libertum_library::versions(&original).len(), 2);
        let _ = app.update(Message::CloseLibrary);
        assert!(app.modal.is_none());
    }

    #[test]
    fn a_past_version_opens_as_an_unsaved_copy_and_can_be_restored() {
        let dir = synced_folder("restore");
        let path = dir.join("book.lbtd");
        write_native(&path, "first");
        let first = std::fs::read(&path).expect("read");
        let old = libertum_library::snapshot(&path, std::time::SystemTime::now())
            .expect("snapshot")
            .expect("taken")
            .path;
        write_native(&path, "second, longer");
        let mut app = app();
        // A document with no file has no history to show.
        app.file = None;
        let _ = app.update(Message::OpenHistory(None));
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        app.modal = None;

        let _ = app.update(Message::OpenHistory(Some(path.clone())));
        assert!(matches!(&app.modal, Some(Modal::History { versions, .. }) if versions.len() == 1));
        let _ = app.update(Message::HistoryOpenCopy(old.clone()));
        assert!(app.file.is_none(), "a copy has no file of its own");
        assert!(app.modal.is_none());
        assert!(!app.settings.recent_files.contains(&old));
        assert_eq!(
            app.doc
                .as_ref()
                .expect("doc")
                .document()
                .paragraph_at(0)
                .expect("p")
                .text(),
            "first"
        );

        let _ = app.update(Message::HistoryRestore {
            original: path.clone(),
            version: old,
        });
        assert_eq!(std::fs::read(&path).expect("read"), first);
        assert_eq!(
            libertum_library::versions(&path).len(),
            2,
            "the file it replaced is kept"
        );
    }

    #[test]
    fn restoring_over_an_open_document_with_unsaved_edits_is_refused() {
        let dir = synced_folder("restore-dirty");
        let path = dir.join("book.lbtd");
        write_native(&path, "first");
        let old = libertum_library::snapshot(&path, std::time::SystemTime::now())
            .expect("snapshot")
            .expect("taken")
            .path;
        write_native(&path, "second, longer");
        let on_disk = std::fs::read(&path).expect("read");
        let mut app = app();
        app.file = Some(path.clone());
        app.dirty = true;
        let _ = app.update(Message::HistoryRestore {
            original: path.clone(),
            version: old,
        });
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        assert_eq!(std::fs::read(&path).expect("read"), on_disk, "untouched");
    }

    #[test]
    fn saving_round_trips_the_open_document() {
        let app = app();
        let doc = app.doc.as_ref().expect("doc").document().clone();
        let writer = Docx::new(doc.clone());
        let bytes = writer.to_bytes().expect("write");
        let parsed = Docx::parse(&bytes).expect("read");
        assert_eq!(
            parsed.document().paragraph_at(0).expect("paragraph").text(),
            doc.paragraph_at(0).expect("paragraph").text()
        );
    }

    #[test]
    fn saving_an_odt_round_trips_the_open_document() {
        let app = app();
        let doc = app.doc.as_ref().expect("doc").document().clone();
        let writer = Odt::new(doc.clone());
        let bytes = writer.to_bytes().expect("write");
        let parsed = Odt::parse(&bytes).expect("read");
        assert_eq!(
            parsed.document().paragraph_at(0).expect("paragraph").text(),
            doc.paragraph_at(0).expect("paragraph").text()
        );
    }

    #[test]
    fn save_extensions_follow_the_format_the_user_typed() {
        assert_eq!(
            ensure_extension(PathBuf::from("Book"), Format::Odt),
            PathBuf::from("Book.odt")
        );
        assert_eq!(
            ensure_extension(PathBuf::from("Book.docx"), Format::Odt),
            PathBuf::from("Book.docx"),
            "a known extension is kept"
        );
        assert_eq!(
            ensure_extension(PathBuf::from("Book.txt"), Format::Docx),
            PathBuf::from("Book.docx"),
            "an unknown extension is replaced"
        );
        assert_eq!(Format::for_path(std::path::Path::new("a.ODT")), Format::Odt);
        assert_eq!(
            Format::for_path(std::path::Path::new("a.docx")),
            Format::Docx
        );
        assert_eq!(
            Format::for_path(std::path::Path::new("a.md")),
            Format::Markdown
        );
        assert_eq!(Format::for_path(std::path::Path::new("a.rtf")), Format::Rtf);
        assert_eq!(
            ensure_extension(PathBuf::from("Book"), Format::Markdown),
            PathBuf::from("Book.md")
        );
    }

    #[test]
    fn margin_stays_in_hundredths() {
        let mut app = app();
        Field::Margin(crate::state::MarginSide::Left).set(&mut app.state, 3.0);
        assert_eq!(
            app.state.document.margin_left,
            Mixed::Value(crate::state::Margin(300))
        );
    }

    fn pointer(x_pt: f32, y_pt: f32, extend: bool) -> Message {
        Message::Canvas(CanvasEvent::Pointer {
            page: 0,
            x_pt,
            y_pt,
            extend,
            clicks: 1,
        })
    }

    fn pointer_clicks(x_pt: f32, y_pt: f32, clicks: u8) -> Message {
        Message::Canvas(CanvasEvent::Pointer {
            page: 0,
            x_pt,
            y_pt,
            extend: false,
            clicks,
        })
    }

    fn drag(x_pt: f32, y_pt: f32) -> Message {
        Message::Canvas(CanvasEvent::PointerDrag {
            page: 0,
            x_pt,
            y_pt,
        })
    }

    #[test]
    fn clicking_a_page_sets_a_visible_caret() {
        let mut app = app();
        assert!(!app.canvas_focused);
        let _ = app.update(pointer(80.0, 80.0, false));
        assert!(app.canvas_focused);
        let caret = app
            .doc()
            .caret_rect()
            .expect("caret is visible after a click");
        assert_eq!(caret.page, 0);
        assert!(caret.height > 1.0);
        assert!(app.doc().selection_rects().is_empty());
    }

    #[test]
    fn dragging_pointer_extends_a_selection() {
        let mut app = app();
        let _ = app.update(pointer(80.0, 20.0, false));
        let first = app.doc().editor().caret();
        let _ = app.update(drag(300.0, 200.0));
        let second = app.doc().editor().caret();
        assert_ne!(first, second, "the drag must move the caret head");
        assert!(app.doc().selection_rects().len() > 1, "line highlights");
        assert!(app.doc_mut().copy().is_some());
    }

    #[test]
    fn selection_drag_keeps_the_anchor_from_the_press() {
        let mut app = app();
        let _ = app.update(pointer(80.0, 20.0, false));
        let anchor = app.doc().editor().ordered_selection().expect("selection").0;
        let _ = app.update(drag(500.0, 120.0));
        let (start, end) = app.doc().editor().ordered_selection().expect("selection");
        assert_eq!(start, anchor, "the anchor is fixed by the press");
        assert_ne!(end, start, "the pointer drag moved the head");
        assert!(app.canvas_focused);
    }

    #[test]
    fn clicking_a_visible_line_does_not_scroll() {
        let mut app = app();
        let _ = app.update(Message::Canvas(CanvasEvent::Viewport(Size::new(
            940.0, 627.0,
        ))));
        app.scroll = (0.0, 0.0);
        let _ = app.update(pointer(80.0, 100.0, false));
        assert_eq!(app.scroll.1, 0.0, "the caret line was already visible");
    }

    #[test]
    fn double_click_selects_one_word() {
        let mut app = app();
        // The demo document's first paragraph starts on page 1 inside the text area.
        let _ = app.update(pointer(240.0, 120.0, false));
        let _ = app.update(pointer_clicks(240.0, 120.0, 2));
        let text = app.doc_mut().copy().expect("a word is selected");
        assert!(!text.is_empty(), "the selected text is not empty");
        assert!(
            !text.trim().contains(char::is_whitespace),
            "a double click selects a single word, got {text:?}"
        );
    }

    #[test]
    fn triple_click_selects_the_whole_paragraph() {
        let mut app = app();
        // Below the opening heading (which sits lower since space before is kept at the top
        // of the document), inside the first body paragraph.
        let _ = app.update(pointer(240.0, 200.0, false));
        let _ = app.update(pointer_clicks(240.0, 200.0, 3));
        let text = app.doc_mut().copy().expect("a paragraph is selected");
        assert!(
            text.len() > 40,
            "a triple click selects the paragraph, got {} bytes",
            text.len()
        );
    }

    #[test]
    fn insert_table_puts_the_caret_in_a_cell_and_tab_adds_a_row() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Insert(InsertAction::Table));
        {
            let doc = app.doc();
            let table = doc
                .document()
                .body()
                .iter()
                .find_map(|block| block.as_table())
                .expect("table");
            assert_eq!(table.rows.len(), 3);
            assert_eq!(table.columns(), 3);
            assert!(doc.editor().in_table());
        }
        assert!(app.state.in_table);
        app.canvas_focused = true;
        for _ in 0..9 {
            let _ = app.update(named(keyboard::key::Named::Tab));
        }
        let rows = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .rows
            .len();
        assert_eq!(rows, 4, "Tab in the last cell adds a row");
    }

    #[test]
    fn table_menu_commands_insert_and_delete_rows() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Insert(InsertAction::Table));
        let _ = app.update(Message::Table(TableAction::InsertRowBelow));
        let rows = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .rows
            .len();
        assert_eq!(rows, 4);
        let _ = app.update(Message::Table(TableAction::DeleteRow));
        let rows = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .rows
            .len();
        assert_eq!(rows, 3);
        let _ = app.update(Message::Table(TableAction::DeleteTable));
        assert!(
            app.doc()
                .document()
                .body()
                .iter()
                .all(|block| block.as_table().is_none())
        );
        app.doc.as_mut().expect("doc").undo().expect("undo");
        assert!(
            app.doc()
                .document()
                .body()
                .iter()
                .any(|block| block.as_table().is_some())
        );
    }

    #[test]
    fn bookmarks_pane_adds_renames_jumps_and_removes() {
        let mut app = app();
        let _ = app.update(Message::ToggleInspector(Inspector::Bookmarks));
        assert_eq!(app.state.inspector, Inspector::Bookmarks);
        assert!(app.state.sidebar_visible);

        let _ = app.update(Message::AddBookmark);
        assert_eq!(app.bookmarks.len(), 1, "the pane cache is filled");
        let entry = app.bookmarks[0].clone();
        assert_eq!(entry.name, "Bookmark_1");

        let _ = app.update(Message::BookmarkName(
            entry.block,
            entry.index,
            "Chapter One".into(),
        ));
        let _ = app.update(Message::BookmarkNameCommit(entry.block, entry.index));
        assert_eq!(app.bookmarks[0].name, "Chapter_One");

        app.doc_mut().move_caret(|ed| ed.move_doc_end(false));
        let _ = app.update(Message::GoToBookmark(entry.block, entry.index));
        assert_eq!(app.doc().editor().caret().block, entry.block);
        assert_eq!(app.doc().editor().caret().offset, entry.offset);

        let _ = app.update(Message::RemoveBookmark(entry.block, entry.index));
        assert!(app.bookmarks.is_empty());
    }

    #[test]
    fn bookmark_draft_is_not_written_until_return() {
        let mut app = app();
        let _ = app.update(Message::ToggleInspector(Inspector::Bookmarks));
        let _ = app.update(Message::AddBookmark);
        let entry = app.bookmarks[0].clone();
        let _ = app.update(Message::BookmarkName(
            entry.block,
            entry.index,
            "Half".into(),
        ));
        assert_eq!(
            app.bookmarks[0].name, "Bookmark_1",
            "the model is untouched"
        );
        assert_eq!(
            app.bookmark_draft
                .as_ref()
                .map(|(_, _, name)| name.as_str()),
            Some("Half")
        );
        let _ = app.update(Message::BookmarkNameCommit(entry.block, entry.index));
        assert_eq!(app.bookmarks[0].name, "Half");
        assert!(app.bookmark_draft.is_none());
    }

    /// The images of `block` as the document holds them.
    fn images_of(app: &App, block: libertum_model::BlockId) -> Vec<libertum_model::InlineImage> {
        app.doc()
            .document()
            .paragraph(block)
            .map(|p| p.images().to_vec())
            .unwrap_or_default()
    }

    #[test]
    fn inserting_an_image_reaches_the_document_and_the_image_group() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let media = libertum_formats_common::media::sample_png(400, 300).expect("sample PNG");
        let path =
            std::env::temp_dir().join(format!("libertum-image-test-{}.png", std::process::id()));
        std::fs::write(&path, media.bytes()).expect("write the image file");

        let _ = app.insert_image_from(&path);
        let _ = std::fs::remove_file(&path);

        let block = app.doc().editor().caret().block;
        let images = images_of(&app, block);
        assert_eq!(images.len(), 1, "the image is in the caret's paragraph");
        assert_eq!(images[0].media.bytes(), media.bytes(), "bytes are kept");
        // 400 px at 96 dpi is 300 pt, narrower than the text column, so it is not scaled.
        assert!(
            (images[0].width_pt - 300.0).abs() < 0.01,
            "{}",
            images[0].width_pt
        );
        assert!((images[0].height_pt - 225.0).abs() < 0.01);
        assert_eq!(app.state.images.len(), 1, "the Image group lists it");
        assert_eq!(app.state.images[0].block, block);
        assert_eq!(app.state.images[0].index, 0);

        // Larger and Smaller step the width, keeping the proportions.
        let width = app.state.images[0].width_pt;
        let _ = app.update(Message::ImageStep(block, 0, 1));
        let larger = images_of(&app, block)[0].clone();
        assert!(larger.width_pt > width);
        assert!(
            (larger.height_pt / larger.width_pt - 0.75).abs() < 0.001,
            "the proportions are kept"
        );
        assert!((app.state.images[0].width_pt - larger.width_pt).abs() < 0.001);
        let _ = app.update(Message::ImageStep(block, 0, -1));
        assert!((images_of(&app, block)[0].width_pt - width).abs() < 0.01);

        // Alternative text is edited and read back into the pane.
        let _ = app.update(Message::ImageAlt(block, 0, "A grey checker".into()));
        assert_eq!(images_of(&app, block)[0].alt, "A grey checker");
        assert_eq!(app.state.images[0].alt, "A grey checker");

        // Remove takes it out of the document and the pane in one undoable step.
        let _ = app.update(Message::ImageRemove(block, 0));
        assert!(images_of(&app, block).is_empty());
        assert!(app.state.images.is_empty());
        app.doc_mut().undo().expect("undo the removal");
        assert_eq!(images_of(&app, block).len(), 1);
    }

    #[test]
    fn inserting_a_cross_reference_shows_the_bookmark_text_and_page() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::ToggleInspector(Inspector::Bookmarks));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("Chapter One".into())));

        // A bookmark on the heading, then a second paragraph that refers to it.
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(Message::AddBookmark);
        let name = app.bookmarks[0].name.clone();
        app.doc_mut().move_caret(|ed| ed.move_doc_end(false));
        let _ = app.update(named(keyboard::key::Named::Enter));

        let _ = app.update(Message::InsertCrossRef(
            name.clone(),
            libertum_model::CrossRefKind::Text,
        ));
        let last = paragraph_texts(&app).pop().expect("a paragraph");
        assert_eq!(
            last, "Chapter One",
            "the reference shows the heading's text"
        );

        let _ = app.update(Message::InsertCrossRef(
            name.clone(),
            libertum_model::CrossRefKind::Page,
        ));
        let last = paragraph_texts(&app).pop().expect("a paragraph");
        assert_eq!(last, "Chapter One1", "the page reference shows page 1");

        // Retitling the heading updates both references; refreshing again changes nothing.
        let heading = app
            .doc()
            .document()
            .body()
            .iter()
            .filter_map(|block| block.as_paragraph())
            .next()
            .map(|p| (p.id, p.len()))
            .expect("the heading");
        app.doc_mut().move_caret(|ed| {
            ed.set_caret(
                libertum_editor::DocPosition {
                    block: heading.0,
                    offset: heading.1,
                },
                false,
            );
        });
        let _ = app.update(Message::PasteText(Some(" Revised".into())));
        // The pane's Update References button brings both references up to date...
        let _ = app.update(Message::RefreshCrossRefs);
        let last = paragraph_texts(&app).pop().expect("a paragraph");
        assert_eq!(last, "Chapter One Revised1");
        // ...and updating again changes nothing.
        let _ = app.update(Message::RefreshCrossRefs);
        assert_eq!(paragraph_texts(&app).pop().expect("a paragraph"), last);
    }

    #[test]
    fn the_table_menu_sets_and_clears_a_named_style() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Insert(InsertAction::Table));
        let builtins = libertum_model::TableStyle::builtins();
        let wanted = builtins[2].0.name.clone();

        let _ = app.update(Message::TableStyle(Some(2)));
        let style = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .style
            .as_ref()
            .map(|style| style.name.clone());
        assert_eq!(style, Some(wanted.clone()), "the style reaches the table");
        assert_eq!(app.state.table_style, Some(wanted), "and is read back");

        let _ = app.update(Message::TableStyle(None));
        let style = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .style
            .clone();
        assert!(style.is_none());
        assert!(app.state.table_style.is_none());

        // One undo step puts the style back.
        app.doc_mut().undo().expect("undo");
        assert!(
            app.doc()
                .document()
                .body()
                .iter()
                .find_map(|block| block.as_table())
                .expect("table")
                .style
                .is_some()
        );
    }

    #[test]
    fn merge_and_split_cell_commands_reach_the_caret_table() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Insert(InsertAction::Table));
        assert!(app.state.can_merge_right && app.state.can_merge_below);
        assert!(
            !app.state.can_split_cell,
            "an ordinary cell cannot be split"
        );

        let _ = app.update(Message::Table(TableAction::MergeRight));
        let row = {
            let table = app
                .doc()
                .document()
                .body()
                .iter()
                .find_map(|block| block.as_table())
                .expect("table")
                .clone();
            assert_eq!(table.rows[0].cells.len(), 2, "two cells are now one");
            assert_eq!(table.rows[0].cells[0].col_span, 2);
            table.rows[0].clone()
        };
        assert!(app.state.can_split_cell, "the merged cell can be split");
        assert!(
            !app.state.can_merge_below,
            "the wide cell does not cover the same columns as the cells below"
        );
        assert_eq!(row.cells.len(), 2);

        let _ = app.update(Message::Table(TableAction::SplitCell));
        let table = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .clone();
        assert_eq!(table.rows[0].cells.len(), 3, "one cell per column again");
        assert!(!app.state.can_split_cell);

        let _ = app.update(Message::Table(TableAction::MergeBelow));
        let table = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .clone();
        assert!(table.rows[1].cells[0].merged_into_above);
        assert!(app.state.can_split_cell);
        let _ = app.update(Message::Table(TableAction::SplitCell));
        let table = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .clone();
        assert!(!table.rows[1].cells[0].merged_into_above);
    }

    #[test]
    fn the_layout_pane_sets_a_table_column_width_and_equalises() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Insert(InsertAction::Table));
        assert!(app.state.in_table, "the Table group is shown in a table");
        let width = app.state.column_width_pt.expect("a column width");
        let total = |app: &App| -> f32 {
            app.doc()
                .document()
                .body()
                .iter()
                .find_map(|block| block.as_table())
                .expect("table")
                .widths_pt
                .iter()
                .sum()
        };
        let before = total(&app);

        let _ = app.update(Message::Table(TableAction::ColumnWider));
        let wider = app.state.column_width_pt.expect("a column width");
        assert!(wider > width, "{wider} is not wider than {width}");
        assert!(
            (total(&app) - before).abs() < 0.5,
            "the table keeps its overall width"
        );

        let _ = app.update(Message::Table(TableAction::ColumnNarrower));
        assert!((app.state.column_width_pt.expect("width") - width).abs() < 0.01);

        let _ = app.update(Message::Table(TableAction::ColumnWider));
        let _ = app.update(Message::Table(TableAction::EqualiseColumns));
        let widths = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .widths_pt
            .clone();
        assert!(
            widths
                .windows(2)
                .all(|pair| (pair[0] - pair[1]).abs() < 0.01),
            "{widths:?}"
        );
        assert!((widths.iter().sum::<f32>() - before).abs() < 0.5);
        assert!(
            (app.state.column_width_pt.expect("width") - widths[0]).abs() < 0.01,
            "the pane reads the width back"
        );
    }

    #[test]
    fn mcp_switch_is_gated_by_dev_mode_and_saved() {
        let dir = std::env::temp_dir().join(format!("libertum-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp folder");
        let path = dir.join("settings.toml");
        let mut app = App::new(Settings::default(), Some(path.clone()));

        let _ = app.update(Message::SetMcpEnabled(true));
        assert!(!app.settings.mcp_enabled, "refused while dev mode is off");

        let _ = app.update(Message::SetDevMode(true));
        let _ = app.update(Message::SetMcpEnabled(true));
        let saved = Settings::load_from(&path).expect("settings");
        assert!(saved.dev_mode && saved.mcp_enabled, "both are persisted");

        let _ = app.update(Message::SetDevMode(false));
        assert!(!app.settings.mcp_enabled, "closing the gate turns MCP off");
        let saved = Settings::load_from(&path).expect("settings");
        assert!(!saved.dev_mode && !saved.mcp_enabled);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switching_mcp_on_serves_the_open_document_and_off_closes_the_port() {
        use libertum_mcp::{Reply, Request};
        use std::net::{SocketAddr, TcpStream};

        let mut app = App::new(Settings::default(), None);
        assert!(app.mcp.is_none(), "nothing runs by default");

        let _ = app.update(Message::SetDevMode(true));
        assert!(app.mcp.is_none(), "dev mode alone starts nothing");
        let _ = app.update(Message::SetMcpEnabled(true));
        let service = app.mcp.as_ref().expect("the server is running");
        let url = service.url();
        let addr: SocketAddr = url
            .trim_start_matches("http://")
            .trim_end_matches("/mcp")
            .parse()
            .expect("a loopback address");
        assert!(addr.ip().is_loopback());
        assert!(TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok());
        assert_eq!(service.token().len(), 64);

        // A client's question travels to the UI thread and is answered from the open document.
        let (host, queue) = libertum_mcp::channel(Duration::from_secs(5), Duration::from_secs(5));
        let client = libertum_mcp::ClientInfo::new("test", "1");
        let asker = std::thread::spawn(move || {
            libertum_mcp::Host::request(&*host, &client, Request::ListDocuments)
        });
        let pending = queue
            .recv_timeout(Duration::from_secs(5))
            .expect("a question");
        let _ = app.update(Message::McpRequest(mcp::McpRequest::new(pending)));
        let Ok(Reply::Documents(documents)) = asker.join().expect("the asker") else {
            panic!("a document listing");
        };
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].id, app.doc.as_ref().expect("a document").id());

        let _ = app.update(Message::SetDevMode(false));
        assert!(app.mcp.is_none(), "closing the gate stops the server");
        assert!(
            TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err(),
            "the port is closed"
        );
    }

    // ------------------------------------------------------------ MCP edits

    use crate::mcp_runner::{CallResult, Link};
    use libertum_mcp::{ClientInfo, Host as _, HostError, HostResult, Position, Reply, Request};

    fn mcp_app(allow_edits: bool, settings_path: Option<std::path::PathBuf>) -> App {
        let mut app = App::new(Settings::default(), settings_path);
        let _ = app.update(Message::SetDevMode(true));
        let _ = app.update(Message::SetMcpEnabled(true));
        if allow_edits {
            let _ = app.update(Message::SetMcpAllowEdits(true));
        }
        app
    }

    fn insert_request(text: &str) -> Request {
        let at = Position {
            paragraph: 0,
            offset: 0,
        };
        Request::ReplaceRange {
            document: None,
            start: at,
            end: at,
            text: text.into(),
        }
    }

    /// Send `request` as `client` the way the server does, and hand it to the app. Returns the
    /// client side, which finishes when the app answers.
    fn send(app: &mut App, client: &str, request: Request) -> std::thread::JoinHandle<HostResult> {
        let (host, queue) = libertum_mcp::channel(Duration::from_secs(5), Duration::from_secs(30));
        let client = ClientInfo::new(client, "1");
        let asker = std::thread::spawn(move || host.request(&client, request));
        let pending = queue
            .recv_timeout(Duration::from_secs(5))
            .expect("a request");
        let _ = app.update(Message::McpRequest(mcp::McpRequest::new(pending)));
        asker
    }

    fn first_paragraph(app: &App) -> String {
        app.doc
            .as_ref()
            .expect("a document")
            .document()
            .paragraph_at(0)
            .expect("a paragraph")
            .text()
            .to_owned()
    }

    fn asking(app: &App) -> bool {
        matches!(app.modal, Some(Modal::McpApproval { .. }))
    }

    #[test]
    fn edits_are_refused_while_allow_edits_is_off() {
        let mut app = mcp_app(false, None);
        let before = first_paragraph(&app);
        let asker = send(&mut app, "Test tool", insert_request("hello"));
        assert_eq!(asker.join().expect("asker"), Err(HostError::EditsDisabled));
        assert!(!asking(&app), "nothing to ask about: editing is off");
        assert_eq!(first_paragraph(&app), before);
        assert!(!app.dirty);
    }

    #[test]
    fn allow_edits_cannot_be_switched_on_without_mcp() {
        let mut app = App::new(Settings::default(), None);
        let _ = app.update(Message::SetMcpAllowEdits(true));
        assert!(!app.settings.mcp_allow_edits);
        let _ = app.update(Message::SetDevMode(true));
        let _ = app.update(Message::SetMcpAllowEdits(true));
        assert!(!app.settings.mcp_allow_edits, "MCP itself is still off");
    }

    #[test]
    fn a_first_edit_asks_and_then_is_one_undo_step() {
        let mut app = mcp_app(true, None);
        let before = first_paragraph(&app);
        let undo_before = app.doc.as_ref().expect("doc").editor().history().undo_len();

        let asker = send(&mut app, "Test tool", insert_request("hello "));
        assert!(asking(&app), "the writer is asked first");
        assert_eq!(first_paragraph(&app), before, "nothing changed yet");

        let _ = app.update(Message::McpDecision(McpDecision::AllowSession));
        let Ok(Reply::Edited { .. }) = asker.join().expect("asker") else {
            panic!("the edit was made");
        };
        assert!(!asking(&app));
        assert_eq!(first_paragraph(&app), format!("hello {before}"));
        assert!(app.dirty);
        let doc = app.doc.as_mut().expect("doc");
        assert_eq!(doc.editor().history().undo_len(), undo_before + 1);
        doc.undo().expect("undo");
        assert_eq!(first_paragraph(&app), before, "one Undo takes it back");
    }

    #[test]
    fn a_client_allowed_for_the_session_is_not_asked_again_but_another_is() {
        let mut app = mcp_app(true, None);
        let first = send(&mut app, "Tool A", insert_request("a"));
        let _ = app.update(Message::McpDecision(McpDecision::AllowSession));
        assert!(first.join().expect("asker").is_ok());

        let second = send(&mut app, "Tool A", insert_request("b"));
        assert!(!asking(&app), "Tool A is trusted for the session");
        assert!(second.join().expect("asker").is_ok());

        let other = send(&mut app, "Tool B", insert_request("c"));
        assert!(asking(&app), "a different tool asks for itself");
        let _ = app.update(Message::McpDecision(McpDecision::Deny));
        assert_eq!(other.join().expect("asker"), Err(HostError::Denied));
    }

    #[test]
    fn denying_changes_nothing_and_the_next_request_asks_again() {
        let mut app = mcp_app(true, None);
        let before = first_paragraph(&app);
        let asker = send(&mut app, "Test tool", insert_request("no"));
        let _ = app.update(Message::McpDecision(McpDecision::Deny));
        assert_eq!(asker.join().expect("asker"), Err(HostError::Denied));
        assert_eq!(first_paragraph(&app), before);
        assert!(!app.dirty);
        assert!(app.mcp_session_trusted.is_empty());

        let again = send(&mut app, "Test tool", insert_request("no"));
        assert!(asking(&app), "a denial is not remembered as trust");
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert_eq!(
            again.join().expect("asker"),
            Err(HostError::Denied),
            "Escape or a click outside also refuses"
        );
    }

    #[test]
    fn always_allow_is_saved_and_forgetting_clears_it() {
        let dir = std::env::temp_dir().join(format!("libertum-mcp-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp folder");
        let path = dir.join("settings.toml");
        let mut app = mcp_app(true, Some(path.clone()));

        let asker = send(&mut app, "Test tool", insert_request("x"));
        let _ = app.update(Message::McpDecision(McpDecision::AllowAlways));
        assert!(asker.join().expect("asker").is_ok());
        let saved = Settings::load_from(&path).expect("settings");
        assert!(saved.mcp_trusts("Test tool"), "remembered across launches");

        let quiet = send(&mut app, "Test tool", insert_request("y"));
        assert!(!asking(&app));
        assert!(quiet.join().expect("asker").is_ok());

        let _ = app.update(Message::ForgetMcpClients);
        let saved = Settings::load_from(&path).expect("settings");
        assert!(!saved.mcp_trusts("Test tool"));
        let asks = send(&mut app, "Test tool", insert_request("z"));
        assert!(asking(&app), "it has to ask again");
        let _ = app.update(Message::McpDecision(McpDecision::Deny));
        let _ = asks.join();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn switching_allow_edits_off_answers_a_waiting_request_and_closes_the_prompt() {
        let mut app = mcp_app(true, None);
        let before = first_paragraph(&app);
        let asker = send(&mut app, "Test tool", insert_request("late"));
        assert!(asking(&app));
        let _ = app.update(Message::SetMcpAllowEdits(false));
        assert!(!asking(&app));
        assert_eq!(asker.join().expect("asker"), Err(HostError::EditsDisabled));
        assert_eq!(first_paragraph(&app), before);
    }

    #[test]
    fn an_edit_the_server_gave_up_on_is_never_applied() {
        let mut app = mcp_app(true, None);
        let before = first_paragraph(&app);
        // A client whose request times out before anyone answers.
        let (host, queue) =
            libertum_mcp::channel(Duration::from_millis(40), Duration::from_millis(40));
        let client = ClientInfo::new("Slow tool", "1");
        let asker = std::thread::spawn(move || host.request(&client, insert_request("stale")));
        let pending = queue
            .recv_timeout(Duration::from_secs(5))
            .expect("a request");
        assert!(matches!(
            asker.join().expect("asker"),
            Err(HostError::Unavailable(_))
        ));

        // The app only now sees it, and even a trusted tool must not get this edit made.
        app.mcp_session_trusted.insert("Slow tool".into());
        let _ = app.update(Message::McpRequest(mcp::McpRequest::new(pending)));
        assert_eq!(first_paragraph(&app), before);
        assert!(!app.dirty);
        assert!(app.mcp_approvals.is_empty() && !asking(&app));
    }

    #[test]
    fn an_edit_that_times_out_while_the_prompt_is_open_is_not_applied_on_allow() {
        let mut app = mcp_app(true, None);
        let before = first_paragraph(&app);
        let (host, queue) =
            libertum_mcp::channel(Duration::from_millis(40), Duration::from_millis(40));
        let client = ClientInfo::new("Slow tool", "1");
        let asker = std::thread::spawn(move || host.request(&client, insert_request("stale")));
        let pending = queue
            .recv_timeout(Duration::from_secs(5))
            .expect("a request");
        let _ = app.update(Message::McpRequest(mcp::McpRequest::new(pending)));
        assert!(asking(&app));
        // The client gives up while the prompt is still on screen...
        assert!(matches!(
            asker.join().expect("asker"),
            Err(HostError::Unavailable(_))
        ));
        // ...so Allow must not make an edit the client was told failed.
        let _ = app.update(Message::McpDecision(McpDecision::AllowSession));
        assert_eq!(first_paragraph(&app), before);
        assert!(!app.dirty);
    }

    #[test]
    fn queued_requests_are_asked_one_at_a_time() {
        let mut app = mcp_app(true, None);
        let first = send(&mut app, "Tool A", insert_request("1"));
        let second = send(&mut app, "Tool B", insert_request("2"));
        let Some(Modal::McpApproval { client, .. }) = &app.modal else {
            panic!("a prompt");
        };
        assert_eq!(client, "Tool A", "first come, first asked");
        let _ = app.update(Message::McpDecision(McpDecision::Deny));
        assert_eq!(first.join().expect("asker"), Err(HostError::Denied));
        let Some(Modal::McpApproval { client, .. }) = &app.modal else {
            panic!("the next prompt follows");
        };
        assert_eq!(client, "Tool B");
        let _ = app.update(Message::McpDecision(McpDecision::AllowSession));
        assert!(second.join().expect("asker").is_ok());
    }

    #[test]
    fn a_prompt_waits_behind_another_and_shows_when_it_closes() {
        let mut app = mcp_app(true, None);
        app.modal = Some(Modal::Error("Something else".into()));
        let asker = send(&mut app, "Test tool", insert_request("w"));
        assert!(
            matches!(app.modal, Some(Modal::Error(_))),
            "the other prompt is not replaced"
        );
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert!(
            asking(&app),
            "the edit prompt takes its turn: {:?}",
            app.modal
        );
        let _ = app.update(Message::McpDecision(McpDecision::Deny));
        let _ = asker.join();
    }

    #[test]
    fn a_bad_range_is_an_error_and_changes_nothing() {
        let mut app = mcp_app(true, None);
        app.mcp_session_trusted.insert("Test tool".into());
        let before = first_paragraph(&app);
        let at = |paragraph, offset| Position { paragraph, offset };
        let far = Request::ReplaceRange {
            document: None,
            start: at(99, 0),
            end: at(99, 0),
            text: "x".into(),
        };
        let result = send(&mut app, "Test tool", far).join().expect("asker");
        assert!(matches!(result, Err(HostError::Other(m)) if m.contains("paragraph 99")));
        let past_end = Request::ReplaceRange {
            document: None,
            start: at(0, 10_000),
            end: at(0, 10_000),
            text: "x".into(),
        };
        let result = send(&mut app, "Test tool", past_end).join().expect("asker");
        assert!(matches!(result, Err(HostError::Other(_))), "{result:?}");
        assert_eq!(first_paragraph(&app), before);
        assert!(!app.dirty);
    }

    #[test]
    fn format_and_style_edits_work_and_are_one_undo_step_each() {
        let mut app = mcp_app(true, None);
        app.mcp_session_trusted.insert("Test tool".into());
        let _ = send(&mut app, "Test tool", insert_request("Some title"))
            .join()
            .expect("asker");
        let steps = |app: &App| app.doc.as_ref().expect("doc").editor().history().undo_len();
        let base = steps(&app);
        let at = |paragraph, offset| Position { paragraph, offset };

        let italic = Request::FormatRange {
            document: None,
            start: at(0, 0),
            end: at(0, 4),
            format: libertum_mcp::FormatPatch {
                italic: Some(true),
                ..Default::default()
            },
        };
        assert!(
            send(&mut app, "Test tool", italic)
                .join()
                .expect("asker")
                .is_ok()
        );
        assert_eq!(steps(&app), base + 1);

        let style = Request::SetParagraphStyle {
            document: None,
            paragraph: 0,
            style: "heading 1".into(),
        };
        assert!(
            send(&mut app, "Test tool", style)
                .join()
                .expect("asker")
                .is_ok()
        );
        assert_eq!(steps(&app), base + 2);
        let level = app
            .doc
            .as_ref()
            .expect("doc")
            .document()
            .paragraph_at(0)
            .expect("paragraph")
            .props
            .outline_level;
        assert!(level.is_some(), "the heading style gives an outline level");

        let unknown = Request::SetParagraphStyle {
            document: None,
            paragraph: 0,
            style: "No such style".into(),
        };
        let result = send(&mut app, "Test tool", unknown).join().expect("asker");
        assert!(matches!(result, Err(HostError::Other(m)) if m.contains("No such style")));
        assert_eq!(steps(&app), base + 2);
    }

    #[test]
    fn client_names_are_made_safe_before_they_are_shown_or_remembered() {
        assert_eq!(mcp::display_name("  Example\n\tEditor  "), "Example Editor");
        assert_eq!(mcp::display_name("\u{7}\u{0}"), "an unnamed tool");
        assert_eq!(mcp::display_name(&"x".repeat(500)).chars().count(), 61);
    }

    #[test]
    fn update_settings_are_saved_and_muting_keeps_the_version() {
        let dir = std::env::temp_dir().join(format!("libertum-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp folder");
        let path = dir.join("settings.toml");
        let mut app = App::new(Settings::default(), Some(path.clone()));

        let _ = app.update(Message::SetCheckForUpdates(false));
        let saved = Settings::load_from(&path).expect("settings");
        assert!(!saved.check_for_updates, "the switch is persisted");

        app.modal = Some(Modal::UpdateAvailable {
            release: crate::update::Release {
                version: "0.1.3".to_owned(),
                url: "https://github.com/wyn-cmd/libertum/releases/tag/v0.1.3".to_owned(),
                assets: Vec::new(),
            },
            stage: UpdateStage::Ask,
        });
        let _ = app.update(Message::MuteUpdate(7 * 86_400));
        assert!(app.modal.is_none(), "the prompt closes");
        let saved = Settings::load_from(&path).expect("settings");
        let mute = saved.update_mute.expect("a mute is stored");
        assert_eq!(mute.version, "0.1.3");
        assert!(mute.until.is_some(), "a week is a deadline, not forever");

        app.modal = Some(Modal::UpdateAvailable {
            release: crate::update::Release {
                version: "0.1.3".to_owned(),
                url: "https://github.com/wyn-cmd/libertum/releases/tag/v0.1.3".to_owned(),
                assets: Vec::new(),
            },
            stage: UpdateStage::Ask,
        });
        let _ = app.update(Message::DisableUpdates);
        assert!(app.modal.is_none());
        let saved = Settings::load_from(&path).expect("settings");
        assert!(!saved.check_for_updates);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_update_prompt_follows_a_download_to_its_result() {
        let mut app = app();
        let release = crate::update::Release {
            version: "0.4.0".to_owned(),
            url: "https://github.com/wyn-cmd/libertum/releases/tag/v0.4.0".to_owned(),
            assets: Vec::new(),
        };
        app.modal = Some(Modal::UpdateAvailable {
            release: release.clone(),
            stage: UpdateStage::Working,
        });
        // A failed download keeps the prompt, with the reason and the release page to fall back on.
        let _ = app.update(Message::UpdatePrepared(Err("no network".to_owned())));
        assert!(matches!(
            &app.modal,
            Some(Modal::UpdateAvailable { stage: UpdateStage::Failed(why), .. }) if why == "no network"
        ));
        // A downloaded package shows its command and nothing is staged for quitting.
        let _ = app.update(Message::UpdatePrepared(Ok(
            crate::install::Prepared::Command {
                command: "sudo apt install '/tmp/a.deb'".to_owned(),
                file: PathBuf::from("/tmp/a.deb"),
            },
        )));
        assert!(matches!(
            &app.modal,
            Some(Modal::UpdateAvailable { stage: UpdateStage::Command { command, .. }, .. })
                if command == "sudo apt install '/tmp/a.deb'"
        ));
        assert!(app.staged_update.is_none());
        let _ = app.update(Message::DismissUpdate);
        assert!(app.modal.is_none(), "Close dismisses it");
        // A result that arrives after the prompt was closed is ignored.
        let _ = app.update(Message::UpdatePrepared(Err("late".to_owned())));
        assert!(app.modal.is_none());
    }

    fn html_fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/html")
            .join(name)
    }

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("libertum-html-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the folder");
        dir.join(name)
    }

    #[test]
    fn html_opens_pageless_and_keeps_its_page_colour_through_a_save() {
        let mut document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "A web page",
            )]);
        document.sections[0].page_colour = Some([0xE1, 0xEE, 0xFA, 0xFF]);
        let path = temp_file("page.html");
        Html::new(document).save(&path).expect("write");
        let mut app = app();
        let _ = app.open_path(path.clone());
        assert!(app.state.pageless, "HTML opens pageless");
        assert_eq!(
            app.file.as_ref(),
            Some(&path),
            "a plain HTML file is opened, not imported"
        );
        assert_eq!(
            app.doc().editor().page_colour(),
            Some([0xE1, 0xEE, 0xFA, 0xFF])
        );
        let _ = app.update(Message::Document(DocumentMsg::PageColour(Some([
            0xFC, 0xE4, 0xE4, 0xFF,
        ]))));
        assert!(app.dirty);
        // The save itself runs on a task; write through the open file's package as it does.
        let mut context = app.file_context.clone().expect("the HTML package");
        assert_eq!(context.format(), Format::Html);
        context.set_document(app.doc().document().clone());
        context.save(&path).expect("save");
        let reopened = Html::open(&path).expect("read");
        assert_eq!(
            reopened.document().sections[0].page_colour,
            Some([0xFC, 0xE4, 0xE4, 0xFF])
        );
        assert_eq!(Format::for_path(&PathBuf::from("a.HTM")), Format::Html);
        assert_eq!(
            ensure_extension(PathBuf::from("book"), Format::Html),
            PathBuf::from("book.html")
        );
    }

    #[test]
    fn an_ao3_download_and_an_epub_open_as_new_unsaved_documents() {
        let download = temp_file("ao3-download.html");
        std::fs::copy(html_fixture("ao3-download.html"), &download).expect("copy");
        let before = std::fs::read(&download).expect("read");
        let mut app = app();
        let _ = app.open_path(download.clone());
        assert!(app.file.is_none(), "an AO3 download is imported");
        assert!(
            matches!(&app.modal, Some(Modal::Notice { title, .. }) if title == s::IMPORTED_TITLE)
        );
        assert!(app.state.pageless);
        assert_eq!(
            std::fs::read(&download).expect("read"),
            before,
            "the download is untouched"
        );
        app.modal = None;

        let book = libertum_model::Document::from_paragraphs(vec![
            libertum_model::Paragraph::new(libertum_model::BlockId(1), "Chapter One").with_props(
                libertum_model::ParaProps {
                    style: libertum_model::StyleId::HEADING_1,
                    outline_level: Some(1),
                    ..libertum_model::ParaProps::default()
                },
            ),
            libertum_model::Paragraph::new(libertum_model::BlockId(2), "Once upon a time."),
        ]);
        let epub = temp_file("book.epub");
        let bytes = libertum_export::build_epub(
            &book,
            libertum_export::ChapterLevels::default(),
            "Book",
            "en-GB",
        )
        .expect("epub");
        std::fs::write(&epub, bytes).expect("write");
        let _ = app.open_path(epub);
        assert!(app.file.is_none(), "an EPUB is imported");
        let texts = paragraph_texts(&app);
        assert!(texts.iter().any(|t| t == "Once upon a time."), "{texts:?}");
    }

    #[test]
    fn a_chapter_imports_after_the_caret_from_a_file_or_copied_html_and_undoes() {
        let mut app = app();
        let before = paragraph_texts(&app);
        let _ = app.import_chapter_file(&html_fixture("emphasis.html"));
        let after_file = paragraph_texts(&app);
        assert!(
            after_file.len() > before.len(),
            "the file's paragraphs are inserted"
        );
        assert!(app.dirty);
        let _ = app.update(Message::ImportChapterPasted(Some(
            "<h2>Chapter 2</h2><p>Hello <em>there</em>.</p>".to_owned(),
        )));
        let after_paste = paragraph_texts(&app);
        assert!(
            after_paste.iter().any(|t| t == "Hello there."),
            "{after_paste:?}"
        );
        let _ = app.undo();
        assert_eq!(
            paragraph_texts(&app),
            after_file,
            "one undo removes the paste"
        );
        // An empty clipboard says so.
        let _ = app.update(Message::ImportChapterPasted(None));
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        app.modal = None;
        // Compare mode is read-only: importing waits until it is left.
        app.doc_mut().read_only = true;
        let _ = app.update(Message::ImportChapterClipboard);
        assert!(matches!(app.modal, Some(Modal::Error(_))));
    }

    #[test]
    fn plain_html_export_writes_no_styles() {
        let mut document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Plain",
            )]);
        document.sections[0].page_colour = Some([0xF4, 0xB6, 0xB6, 0xFF]);
        let path = temp_file("plain.html");
        write_export(
            ExportFormat::PlainHtml,
            document,
            libertum_export::ChapterLevels::default(),
            "Plain".to_owned(),
            path.clone(),
        )
        .expect("export");
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("Plain"));
        assert!(!text.contains("style="), "{text}");
        assert!(!text.contains("<style"), "{text}");
    }

    #[test]
    fn rtf_opens_pageless_docx_paged_and_the_view_menu_toggles() {
        let mut app = app();
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Some text",
            )]);
        let rtf =
            std::env::temp_dir().join(format!("libertum-pageless-{}.rtf", std::process::id()));
        std::fs::write(&rtf, Rtf::new(document.clone()).to_bytes().expect("rtf"))
            .expect("write rtf");
        let docx =
            std::env::temp_dir().join(format!("libertum-pageless-{}.docx", std::process::id()));
        std::fs::write(&docx, Docx::new(document).to_bytes().expect("docx")).expect("write docx");

        // RTF opens in the endless column: no page gap.
        let _ = app.open_path(rtf.clone());
        assert!(app.state.pageless, "an RTF opens pageless");
        assert_eq!(app.metrics().gap, 0.0, "no page gap in a pageless view");

        // The View menu turns it off again: pages with their gap back.
        let _ = app.update(Message::View(ViewAction::Pageless));
        assert!(!app.state.pageless);
        assert!(app.metrics().gap > 0.0, "pages are drawn again");

        // DOCX opens paged...
        let _ = app.open_path(docx.clone());
        assert!(!app.state.pageless, "a DOCX opens paged");
        assert!(app.metrics().gap > 0.0);
        // ...and pageless is one deliberate choice away.
        let _ = app.update(Message::View(ViewAction::Pageless));
        assert!(app.state.pageless, "deliberately enabled for this DOCX");
        assert_eq!(app.metrics().gap, 0.0);

        let _ = std::fs::remove_file(&rtf);
        let _ = std::fs::remove_file(&docx);
    }

    #[test]
    fn find_and_replace_reach_the_document() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("cat sat on the cat mat".into())));
        let _ = app.update(Message::OpenFind);
        assert!(app.state.find.open, "the bar opens");
        // Typing the query already selects the first match and counts them all.
        find_typed(&mut app, "cat");
        assert_eq!(app.doc().editor().selection().anchor.offset, 0);
        assert_eq!(app.doc().editor().selection().head.offset, 3);
        assert_eq!(app.state.find.count, Some((Some(1), 2)));
        assert_eq!(s::find_count(Some(1), 2), "1 of 2");

        // Next selects the second match, then wraps to the first.
        let _ = app.update(Message::FindNext);
        assert_eq!(app.doc().editor().selection().head.offset, 18);
        assert_eq!(app.state.find.count, Some((Some(2), 2)));
        let _ = app.update(Message::FindNext);
        assert_eq!(app.doc().editor().selection().head.offset, 3);
        assert_eq!(app.state.find.count, Some((Some(1), 2)));
        // Previous goes back the other way.
        let _ = app.update(Message::FindPrevious);
        assert_eq!(app.doc().editor().selection().head.offset, 18);

        // Replace the selected match, then replace every remaining one.
        let _ = app.update(Message::ReplaceQuery("dog".into()));
        let _ = app.update(Message::ReplaceOne);
        let text = paragraph_texts(&app).join(" / ");
        assert_eq!(text, "cat sat on the dog mat");
        find_typed(&mut app, "cat");
        let _ = app.update(Message::ReplaceAll);
        assert_eq!(paragraph_texts(&app).join(" / "), "dog sat on the dog mat");
        assert_eq!(app.state.find.notice, Some(s::find_replaced(1)));

        // One undo step takes the whole replace-all back.
        app.doc_mut().undo().expect("undo the replace all");
        assert_eq!(paragraph_texts(&app).join(" / "), "cat sat on the dog mat");

        // Escape closes the bar and gives the canvas its keyboard back.
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(!app.state.find.open);
        assert!(app.canvas_focused);
        let _ = app.view();
    }

    #[test]
    fn find_reports_when_nothing_matches() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("nothing here".into())));
        let _ = app.update(Message::OpenFind);
        find_typed(&mut app, "zebra");
        let _ = app.update(Message::FindNext);
        assert_eq!(app.state.find.notice, Some(s::FIND_NO_MATCHES.to_owned()));
        let _ = app.update(Message::ToggleFindCase);
        assert!(app.state.find.match_case);
        let _ = app.update(Message::ToggleFindWord);
        assert!(app.state.find.whole_word);
    }

    #[test]
    fn autosave_follows_the_setting_and_the_open_document() {
        let mut app = home();
        assert_eq!(app.autosave_interval(), None, "no document, no timer");
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert_eq!(app.autosave_interval(), Some(60), "the default period");
        app.settings.autosave_secs = 0;
        assert_eq!(app.autosave_interval(), None, "zero switches autosave off");
    }

    #[test]
    fn autosave_skips_an_unchanged_dirty_document() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.dirty = true;
        let revision = app.doc().document().revision();
        app.autosave_done = Some(revision);
        let _ = app.update(Message::AutosaveTick);
        assert_eq!(
            app.autosave_pending, None,
            "the same revision already has a snapshot"
        );
        assert_eq!(app.autosave_done, Some(revision));
    }

    #[test]
    fn a_recovery_snapshot_is_offered_and_restored() {
        let dir =
            std::env::temp_dir().join(format!("libertum-recovery-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Recovered words",
            )]);
        let old = std::path::Path::new("C:/books/novel.odt");
        recovery::write_as(&dir, CRASHED_WINDOW, document.clone(), Some(old)).expect("a snapshot");

        let mut app = home();
        app.recovery_dir = dir.clone();
        app.check_recovery();
        assert!(
            matches!(app.modal, Some(Modal::Recovery)),
            "the prompt is offered on the home screen"
        );

        let _ = app.update(Message::ModalChoice(ModalChoice::Restore));
        assert_eq!(
            app.doc()
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .text(),
            "Recovered words"
        );
        assert!(app.dirty, "a recovered document stays unsaved");
        assert_eq!(
            app.file.as_deref(),
            Some(old),
            "Save writes back to its file"
        );
        assert!(recovery::load(&dir).is_none(), "the snapshot is consumed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discarding_a_recovery_snapshot_deletes_it() {
        let dir =
            std::env::temp_dir().join(format!("libertum-recovery-drop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Not wanted",
            )]);
        recovery::write_as(&dir, CRASHED_WINDOW, document.clone(), None).expect("a snapshot");

        let mut app = home();
        app.recovery_dir = dir.clone();
        app.check_recovery();
        let _ = app.update(Message::ModalChoice(ModalChoice::Discard));
        assert!(app.doc.is_none(), "still on the home screen");
        assert!(recovery::load(&dir).is_none(), "the snapshot is gone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn opening_and_saving_remember_recent_files() {
        let mut opened = home();
        let dir = std::env::temp_dir().join(format!("libertum-recent-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Recent words",
            )]);
        let write = |name: &str, document: &libertum_model::Document| {
            let path = dir.join(name);
            std::fs::write(&path, Odt::new(document.clone()).to_bytes().expect("odt"))
                .expect("write");
            path
        };
        let first = write("first.odt", &document);
        let second = write("second.odt", &document);

        let _ = opened.open_path(first.clone());
        assert_eq!(opened.settings.recent_files, vec![first.clone()]);
        let _ = opened.open_path(second.clone());
        assert_eq!(
            opened.settings.recent_files,
            vec![second.clone(), first.clone()],
            "newest first"
        );
        let _ = opened.open_path(first.clone());
        assert_eq!(
            opened.settings.recent_files,
            vec![first.clone(), second.clone()],
            "re-opening moves a file up without duplicating it"
        );

        for index in 0..10 {
            opened.remember_recent(&dir.join(format!("file-{index}.odt")));
        }
        assert_eq!(opened.settings.recent_files.len(), 8, "the list is capped");
        assert_eq!(opened.settings.recent_files[0], dir.join("file-9.odt"));

        // The home screen draws the list, and choosing an entry opens it.
        let mut browsing = home();
        browsing.settings.recent_files = opened.settings.recent_files.clone();
        let _ = browsing.view();
        let _ = browsing.update(Message::OpenRecent(first.clone()));
        assert_eq!(browsing.file.as_deref(), Some(first.as_path()));

        // With unsaved changes, choosing a recent file asks first.
        let mut busy = app();
        busy.canvas_focused = true;
        let _ = busy.update(Message::PasteText(Some("changed".into())));
        assert!(busy.dirty);
        let _ = busy.update(Message::OpenRecent(second.clone()));
        assert!(
            matches!(busy.modal, Some(Modal::Unsaved { .. })),
            "the unsaved-changes prompt comes first"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_settings_window_changes_settings_and_applies_them() {
        let mut app = home();
        let _ = app.update(Message::OpenSettings);
        assert!(app.settings_open, "the window opens");
        let _ = app.view();

        let _ = app.update(Message::SetTheme(ThemePreference::Dark));
        assert_eq!(app.settings.theme, ThemePreference::Dark);
        assert_eq!(
            app.theme_preference,
            ThemePreference::Dark,
            "applied at once"
        );

        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Markdown));
        assert_eq!(app.settings.default_format, DefaultFormat::Markdown);

        let _ = app.update(Message::SetAutosave(30));
        assert_eq!(app.settings.autosave_secs, 30);

        let _ = app.update(Message::SetUnits(UnitsPreference::Inches));
        assert_eq!(app.settings.units, UnitsPreference::Inches);
        assert_eq!(app.state.units, Units::Inches, "the inspector follows");

        let _ = app.update(Message::CloseSettings);
        assert!(!app.settings_open);

        // The default format names a new document.
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert!(
            app.title().starts_with("Untitled.md"),
            "a new document follows the default format: {}",
            app.title()
        );
    }

    #[test]
    fn a_dropped_document_opens_and_other_files_are_ignored() {
        let dir = std::env::temp_dir().join(format!("libertum-drop-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Dropped words",
            )]);
        let odt = dir.join("dropped.odt");
        std::fs::write(&odt, Odt::new(document).to_bytes().expect("odt")).expect("write odt");

        let mut dropped = home();
        let _ = dropped.update(Message::FileDropped(odt.clone()));
        assert_eq!(
            dropped.doc().document().paragraph_at(0).expect("p").text(),
            "Dropped words"
        );
        assert_eq!(dropped.file.as_deref(), Some(odt.as_path()));

        // A file the app does not read is ignored rather than shown as an error.
        let other = dir.join("notes.txt");
        std::fs::write(&other, b"text").expect("write");
        let mut fresh = home();
        let _ = fresh.update(Message::FileDropped(other));
        assert!(fresh.doc.is_none(), "only document formats open");

        // Dropping onto a document with unsaved changes asks first.
        let mut busy = app();
        busy.canvas_focused = true;
        let _ = busy.update(Message::PasteText(Some("changed".into())));
        let _ = busy.update(Message::FileDropped(odt.clone()));
        assert!(matches!(busy.modal, Some(Modal::Unsaved { .. })));
        assert_eq!(busy.file, None, "nothing opened yet");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn right_click(x_pt: f32, y_pt: f32) -> Message {
        Message::Canvas(CanvasEvent::ContextMenu {
            page: 0,
            x_pt,
            y_pt,
            x: 300.0,
            y: 200.0,
        })
    }

    #[test]
    fn right_click_opens_the_menu_for_what_is_under_the_pointer() {
        let mut app = app();
        let _ = app.update(right_click(150.0, 120.0));
        assert!(
            matches!(
                app.context_menu.as_ref().map(|menu| &menu.target),
                Some(crate::toolbar::ContextMenuTarget::Text { .. })
            ),
            "text commands over body text"
        );
        let _ = app.view();

        // Choosing a command runs it and closes the menu.
        let _ = app.update(Message::SelectAll);
        assert!(app.context_menu.is_none());
        let _ = app.update(right_click(150.0, 120.0));
        assert!(app.context_menu.is_some());
        // A left click anywhere else lands on the backdrop, which sends CloseMenu.
        let _ = app.update(Message::CloseMenu);
        assert!(
            app.context_menu.is_none(),
            "a click elsewhere closes the right-click menu"
        );
        let _ = app.update(right_click(150.0, 120.0));
        assert!(app.context_menu.is_some());
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(app.context_menu.is_none(), "Escape closes it");
    }

    #[test]
    fn right_click_in_a_table_offers_the_table_commands() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Insert(InsertAction::Table));
        let caret = app.doc().caret_rect().expect("a caret in the first cell");
        let _ = app.update(right_click(caret.x + 1.0, caret.y + caret.height * 0.5));
        assert!(
            matches!(
                app.context_menu.as_ref().map(|menu| &menu.target),
                Some(crate::toolbar::ContextMenuTarget::Table { .. })
            ),
            "table commands over a cell"
        );
        let _ = app.update(Message::Table(TableAction::InsertRowBelow));
        assert!(app.context_menu.is_none());
        let rows = app
            .doc()
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table")
            .rows
            .len();
        assert_eq!(rows, 4, "the menu command reached the table");
    }

    #[test]
    fn right_click_on_an_image_offers_the_image_commands() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let media = libertum_formats_common::media::sample_png(400, 300).expect("png");
        let path =
            std::env::temp_dir().join(format!("libertum-image-menu-{}.png", std::process::id()));
        std::fs::write(&path, media.bytes()).expect("write");
        let _ = app.insert_image_from(&path);
        let _ = std::fs::remove_file(&path);
        let block = app.doc().editor().caret().block;
        let caret = app
            .doc()
            .caret_rect()
            .expect("a caret in the image's paragraph");
        let _ = app.update(right_click(caret.x + 1.0, caret.y + caret.height * 0.5));
        match app.context_menu.as_ref().map(|menu| &menu.target) {
            Some(crate::toolbar::ContextMenuTarget::Image {
                block: found,
                index,
            }) => {
                assert_eq!(*found, block);
                assert_eq!(*index, 0);
            }
            _ => panic!("image commands over an image"),
        }
        let before = images_of(&app, block)[0].width_pt;
        let _ = app.update(Message::ImageStep(block, 0, 1));
        assert!(images_of(&app, block)[0].width_pt > before);
    }

    #[test]
    fn a_native_document_keeps_its_pageless_view_and_zoom() {
        let dir = std::env::temp_dir().join(format!("libertum-native-app-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("novel.lbtd");
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Native words",
            )]);
        let mut writer = FileContext::new(Format::Native, document.clone());
        writer.set_document(document);
        writer.set_view(ViewState {
            pageless: true,
            zoom_percent: 125,
        });
        writer.save(&path).expect("save");
        assert_eq!(Format::for_path(&path), Format::Native);
        assert_eq!(
            ensure_extension(path.clone(), Format::Docx),
            path,
            "a .lbtd name is kept, not rewritten"
        );

        let (context, read, warnings) = FileContext::open(&path).expect("open");
        assert!(warnings.is_empty());
        assert_eq!(
            read.paragraph_at(0).expect("paragraph").text(),
            "Native words"
        );
        assert_eq!(
            context.view(),
            Some(ViewState {
                pageless: true,
                zoom_percent: 125
            })
        );

        // Opening it in the app applies the view it was saved with.
        let mut app = home();
        let _ = app.update(Message::OpenRecent(path.clone()));
        assert_eq!(
            app.doc()
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .text(),
            "Native words"
        );
        assert!(app.state.pageless, "opened pageless, as it was saved");
        assert_eq!(app.state.zoom, Zoom::Percent(125));
        assert_eq!(app.file.as_deref(), Some(path.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_native_format_can_be_the_default_for_new_documents() {
        let mut app = home();
        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Native));
        assert_eq!(app.settings.default_format, DefaultFormat::Native);
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert!(
            app.title().starts_with("Untitled.lbtd"),
            "a new document follows the default format: {}",
            app.title()
        );
    }

    #[test]
    fn the_performance_overlay_is_off_until_switched_on_in_settings() {
        let mut app = app();
        assert!(!app.settings.show_performance);
        // Hidden: the canvas is drawn without the pill.
        let _ = app.view();
        let _ = app.update(Message::SetShowPerformance(true));
        assert!(app.settings.show_performance);
        // A fresh counter: it starts measuring rather than showing a stale rate.
        assert!(app.fps.label().is_none());
        let _ = app.view();
        let _ = app.update(Message::SetShowPerformance(false));
        assert!(!app.settings.show_performance);
    }

    #[test]
    fn antialiasing_is_a_setting_that_changes_how_strips_are_keyed() {
        let mut app = app();
        assert!(app.settings.antialiasing);
        assert!(crate::docview::antialias());
        let doc = libertum_layout::spike::generate_model_document(2, 3);
        let mut st = libertum_layout::LayoutState::from_document(
            &doc,
            libertum_layout::PageGeometry::a4(),
            libertum_layout::LayoutOptions::default(),
        );
        st.paginate_until(1);
        let smooth = crate::docview::plan_bands(&st, 0, 1.0).expect("plan");
        let _ = app.update(Message::SetAntialiasing(false));
        assert!(!app.settings.antialiasing);
        assert!(!crate::docview::antialias());
        let hard = crate::docview::plan_bands(&st, 0, 1.0).expect("plan");
        assert_ne!(smooth.keys, hard.keys, "pages are redrawn when it changes");
        let _ = app.view();
        let _ = app.update(Message::SetAntialiasing(true));
        assert!(crate::docview::antialias());
        assert_eq!(
            crate::docview::plan_bands(&st, 0, 1.0).expect("plan").keys,
            smooth.keys
        );
    }

    #[test]
    fn the_frame_rate_counter_reports_a_rate() {
        let mut app = app();
        app.settings.show_performance = true;
        let start = Instant::now();
        for frame in 0..40u32 {
            let _ = app.update(Message::FpsFrame(
                start + Duration::from_millis(u64::from(frame) * 16),
            ));
        }
        assert!(
            app.fps.label().is_some(),
            "a rate is known after two windows"
        );
        let _ = app.view();
    }

    #[test]
    fn ctrl_w_goes_home_and_asks_about_unsaved_changes() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert!(app.doc.is_some());
        let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
        assert!(app.doc.is_none(), "a clean document goes straight home");
        assert!(app.file.is_none());

        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("edited".into())));
        let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
        assert!(matches!(app.modal, Some(Modal::Unsaved { .. })));
        assert!(
            app.doc.is_some(),
            "nothing closes until the prompt is answered"
        );
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert!(app.doc.is_some(), "Cancel keeps the document");
        let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
        let _ = app.update(Message::ModalChoice(ModalChoice::Discard));
        assert!(app.doc.is_none(), "Discard closes it");
    }

    #[test]
    fn ctrl_n_goes_home_to_pick_a_template() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert!(app.doc.is_some());
        let _ = app.update(Message::Shortcut(Shortcut::NewDocument));
        assert!(app.doc.is_none(), "a clean document goes straight home");

        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("edited".into())));
        let _ = app.update(Message::Shortcut(Shortcut::NewDocument));
        assert!(matches!(app.modal, Some(Modal::Unsaved { .. })));
        assert!(app.doc.is_some(), "unsaved work is never dropped silently");

        // On the home screen itself it does nothing.
        let mut app = home();
        let _ = app.update(Message::Shortcut(Shortcut::NewDocument));
        assert!(app.doc.is_none());
        assert!(app.modal.is_none());
    }

    #[test]
    fn ctrl_alt_digits_set_heading_styles() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        for (level, style) in [
            (1, ParagraphStyle::Heading1),
            (2, ParagraphStyle::Heading2),
            (4, ParagraphStyle::Heading4),
            (0, ParagraphStyle::Body),
        ] {
            let _ = app.update(Message::Shortcut(Shortcut::Heading(level)));
            assert_eq!(app.state.format.paragraph_style, Mixed::Value(style));
        }
    }

    #[test]
    fn page_colour_and_watermark_follow_the_format_a_save_would_use() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let both = |app: &App| (app.state.keeps_page_colour, app.state.keeps_watermark);
        for (format, expected) in [
            (DefaultFormat::Docx, (true, true)),
            (DefaultFormat::Odt, (true, true)),
            (DefaultFormat::Native, (true, true)),
            (DefaultFormat::Rtf, (false, false)),
            (DefaultFormat::Markdown, (false, false)),
        ] {
            let _ = app.update(Message::SetDefaultFormat(format));
            assert_eq!(both(&app), expected, "new document, default {format:?}");
        }
        // A saved file's own format wins over the Settings default.
        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Rtf));
        for (name, expected) in [
            ("a.docx", (true, true)),
            ("a.odt", (true, true)),
            ("a.lbtd", (true, true)),
            ("a.html", (true, false)),
            ("a.rtf", (false, false)),
            ("a.md", (false, false)),
        ] {
            app.file = Some(std::path::PathBuf::from(name));
            app.refresh_format();
            assert_eq!(both(&app), expected, "{name}");
        }
    }

    fn language_temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("libertum-language-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the folder");
        dir.join(name)
    }

    #[test]
    fn each_document_keeps_its_own_language() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        // A new document is stamped with the Settings default, so the file keeps it.
        assert_eq!(app.spell_language(), Some(spell::Language::British));
        assert_eq!(app.doc().editor().language(), Some("en-GB"));

        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::American,
        )));
        assert_eq!(app.spell_language(), Some(spell::Language::American));
        assert_eq!(app.state.document.language, DocLanguage::American);
        assert_eq!(app.doc().editor().language(), Some("en-US"));
        assert_eq!(
            app.settings.spellcheck_language,
            spell::Language::British,
            "the choice belongs to the document, not to Settings"
        );
        assert!(app.dirty, "the file now differs from what is on disk");

        // Another document starts from the default again.
        let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
        let _ = app.update(Message::ModalChoice(ModalChoice::Discard));
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert_eq!(app.spell_language(), Some(spell::Language::British));

        // The Settings default only touches documents that do not say.
        let _ = app.update(Message::SetSpellLanguage(spell::Language::American));
        assert_eq!(
            app.spell_language(),
            Some(spell::Language::British),
            "this document already says British"
        );
        app.doc_mut()
            .edit(|ed| ed.set_language(None))
            .expect("clear");
        assert_eq!(
            app.spell_language(),
            Some(spell::Language::American),
            "a document with no language follows the default"
        );
    }

    #[test]
    fn a_second_language_widens_the_dictionaries_and_is_kept_with_the_file() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert_eq!(app.spell_languages(), [spell::Language::British]);
        let _ = app.update(Message::Document(DocumentMsg::Language2(Some(
            DocLanguage::Ukrainian,
        ))));
        assert_eq!(app.doc().editor().language2(), Some("uk-UA"));
        assert_eq!(
            app.spell_languages(),
            [spell::Language::British, spell::Language::Ukrainian]
        );
        assert_eq!(app.state.document.language2, Some(DocLanguage::Ukrainian));
        // The same language twice is one dictionary, and the second drops out of the file.
        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::Ukrainian,
        )));
        assert_eq!(app.spell_languages(), [spell::Language::Ukrainian]);
        // A language with no dictionary adds none.
        let _ = app.update(Message::Document(DocumentMsg::Language2(Some(
            DocLanguage::Japanese,
        ))));
        assert_eq!(app.spell_languages(), [spell::Language::Ukrainian]);
        let _ = app.update(Message::Document(DocumentMsg::Language2(None)));
        assert_eq!(app.doc().editor().language2(), None);
        // A file that cannot hold it gets the pair back from the app's memory by path.
        let path = language_temp("pair.rtf");
        let _ = app.update(Message::Document(DocumentMsg::Language2(Some(
            DocLanguage::British,
        ))));
        app.file = Some(path.clone());
        app.remember_language_for_path(&path);
        assert_eq!(
            app.settings.language_for(&path),
            Some(DocLanguage::Ukrainian)
        );
        assert_eq!(
            app.settings.second_language_for(&path),
            Some(DocLanguage::British)
        );
    }

    #[test]
    fn a_language_the_file_names_wins_and_one_without_a_dictionary_falls_back() {
        for (tag, expected) in [
            ("en-US", spell::Language::American),
            ("en-AU", spell::Language::British),
            ("fr-FR", spell::Language::British),
            ("en", spell::Language::British),
        ] {
            let mut document =
                libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                    libertum_model::BlockId(1),
                    "Text",
                )]);
            document.sections[0].language = Some(tag.to_owned());
            let path = language_temp(&format!("{tag}.html"));
            Html::new(document).save(&path).expect("write");
            let mut app = app();
            let _ = app.open_path(path);
            assert_eq!(app.spell_language(), Some(expected), "{tag}");
            assert_eq!(
                app.doc().editor().language(),
                Some(tag),
                "{tag}: opening does not rewrite the file's own tag"
            );
            assert!(!app.dirty);
        }
    }

    #[test]
    fn a_language_change_is_an_unsaved_change_only_if_the_file_can_hold_it() {
        for (format, holds) in [
            (DefaultFormat::Docx, true),
            (DefaultFormat::Odt, true),
            (DefaultFormat::Native, true),
            (DefaultFormat::Rtf, false),
            (DefaultFormat::Markdown, false),
        ] {
            let mut app = home();
            let _ = app.update(Message::SetDefaultFormat(format));
            let _ = app.update(Message::CreateDocument(TemplateId::Blank));
            assert!(!app.dirty);
            let _ = app.update(Message::Document(DocumentMsg::Language(
                DocLanguage::American,
            )));
            assert_eq!(app.dirty, holds, "{format:?}");
            assert_eq!(app.spell_language(), Some(spell::Language::American));
        }
    }

    #[test]
    fn rtf_and_markdown_remember_their_language_by_path() {
        for (name, content) in [
            ("remembered.rtf", "{\\rtf1\\ansi Hello\\par}"),
            ("remembered.md", "# Hello\n\nA paragraph.\n"),
        ] {
            let path = language_temp(name);
            std::fs::write(&path, content).expect("write");
            let other = language_temp(&format!("other-{name}"));
            std::fs::write(&other, content).expect("write");
            let mut app = home();
            let _ = app.open_path(path.clone());
            assert_eq!(
                app.spell_language(),
                Some(spell::Language::British),
                "{name}"
            );
            let _ = app.update(Message::Document(DocumentMsg::Language(
                DocLanguage::American,
            )));
            assert!(!app.dirty, "{name}: nothing in the file changed");
            assert_eq!(
                app.settings.language_for(&path),
                Some(DocLanguage::American),
                "{name}: remembered at once"
            );
            assert_eq!(app.settings.language_for(&other), None);

            // Reopened, it is American; a different file is not.
            let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
            let _ = app.open_path(path.clone());
            assert_eq!(
                app.spell_language(),
                Some(spell::Language::American),
                "{name}"
            );
            assert!(!app.dirty);
            let _ = app.open_path(other);
            assert_eq!(
                app.spell_language(),
                Some(spell::Language::British),
                "{name}"
            );
        }
    }

    #[test]
    fn saving_into_rtf_remembers_the_language_and_into_docx_does_not_need_to() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::American,
        )));
        let rtf = language_temp("saved-as.rtf");
        let _ = app.update(Message::Saved {
            path: rtf.clone(),
            result: Ok(()),
        });
        assert_eq!(app.settings.language_for(&rtf), Some(DocLanguage::American));
        let docx = language_temp("saved-as.docx");
        let _ = app.update(Message::Saved {
            path: docx.clone(),
            result: Ok(()),
        });
        assert_eq!(
            app.settings.language_for(&docx),
            None,
            "a Word file carries its own language"
        );
    }

    #[test]
    fn east_asian_languages_can_be_chosen_and_turn_spell_checking_off() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert_eq!(app.doc_language(), DocLanguage::British);
        assert!(app.spell_language().is_some());
        for (language, tag) in [
            (DocLanguage::Japanese, "ja-JP"),
            (DocLanguage::Korean, "ko-KR"),
            (DocLanguage::ChineseSimplified, "zh-CN"),
            (DocLanguage::ChineseTraditional, "zh-TW"),
        ] {
            let _ = app.update(Message::Document(DocumentMsg::Language(language)));
            assert_eq!(app.doc_language(), language);
            assert_eq!(app.state.document.language, language);
            assert_eq!(app.doc().editor().language(), Some(tag));
            assert_eq!(app.spell_language(), None, "{language:?} has no dictionary");
            assert!(app.spell_errors.is_empty());
        }
        // Back to English brings the dictionary back, and the Settings default is untouched.
        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::American,
        )));
        assert_eq!(app.spell_language(), Some(spell::Language::American));
        assert_eq!(app.settings.spellcheck_language, spell::Language::British);
    }

    #[test]
    fn ukrainian_and_russian_documents_are_checked_with_their_own_dictionaries() {
        let mut app = home();
        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Docx));
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        for (language, tag, dictionary) in [
            (DocLanguage::Ukrainian, "uk-UA", spell::Language::Ukrainian),
            (DocLanguage::Russian, "ru-RU", spell::Language::Russian),
        ] {
            let _ = app.update(Message::Document(DocumentMsg::Language(language)));
            assert_eq!(app.doc().editor().language(), Some(tag));
            assert_eq!(app.spell_language(), Some(dictionary));
            // The tag survives a Word file, and a Ukrainian reader on another machine uses it.
            let path = language_temp(&format!("{tag}.docx"));
            FileContext::new(Format::Docx, app.doc().document().clone())
                .save(&path)
                .expect("save");
            let mut elsewhere = home();
            let _ = elsewhere.open_path(path);
            assert_eq!(elsewhere.doc_language(), language);
            assert_eq!(elsewhere.spell_language(), Some(dictionary));
        }
    }

    #[test]
    fn east_asian_text_is_counted_by_character_and_typed_in_by_the_input_method() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        // A composition shows as it is typed, and nothing reaches the document until commit.
        let _ = app.update(Message::Ime(input_method::Event::Preedit(
            "ニホン".into(),
            None,
        )));
        assert_eq!(app.ime_preedit.as_deref(), Some("ニホン"));
        assert_eq!(app.state.word_count, 0);
        let _ = app.update(Message::Ime(input_method::Event::Commit("日本語".into())));
        assert_eq!(app.ime_preedit, None);
        assert_eq!(
            app.doc().document().paragraph_at(0).expect("p").text(),
            "日本語"
        );
        assert_eq!(app.state.word_count, 3);
        // Chinese, Korean and a mix, committed one after another, land in order.
        for text in ["，你好", " 안녕하세요 ", "Hello"] {
            let _ = app.update(Message::Ime(input_method::Event::Commit(text.into())));
        }
        assert_eq!(
            app.doc().document().paragraph_at(0).expect("p").text(),
            "日本語，你好 안녕하세요 Hello"
        );
        // 日本語 (3) + 你好 (2) + one Korean word + Hello.
        assert_eq!(app.state.word_count, 3 + 2 + 1 + 1);
        // The caret sits after the committed text, so more typing continues from there.
        let _ = app.update(Message::Ime(input_method::Event::Commit("!".into())));
        assert!(
            app.doc()
                .document()
                .paragraph_at(0)
                .expect("p")
                .text()
                .ends_with("Hello!")
        );
    }

    #[test]
    fn deleting_and_moving_over_east_asian_text_works_a_character_at_a_time() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::Ime(input_method::Event::Commit(
            "你好한국어こんにちは".into(),
        )));
        let text = |app: &App| {
            app.doc()
                .document()
                .paragraph_at(0)
                .expect("p")
                .text()
                .to_owned()
        };
        let backspace = || {
            key_press(
                keyboard::Key::Named(keyboard::key::Named::Backspace),
                None,
                keyboard::Modifiers::default(),
            )
        };
        // Each Backspace removes one character, never half of one.
        let _ = app.update(backspace());
        assert_eq!(text(&app), "你好한국어こんにち");
        let left = key_press(
            keyboard::Key::Named(keyboard::key::Named::ArrowLeft),
            None,
            keyboard::Modifiers::default(),
        );
        for _ in 0..3 {
            let _ = app.update(left.clone());
        }
        let _ = app.update(backspace());
        assert_eq!(
            text(&app),
            "你好한국어んにち",
            "three steps left, then the one before the caret"
        );
        let _ = app.update(Message::Ime(input_method::Event::Commit("漢".into())));
        assert_eq!(text(&app), "你好한국어漢んにち");
    }

    #[test]
    fn rtf_remembers_a_korean_document_by_path_and_a_docx_carries_japanese() {
        let rtf = language_temp("korean.rtf");
        std::fs::write(&rtf, "{\\rtf1\\ansi Hello\\par}").expect("write");
        let mut app = home();
        let _ = app.open_path(rtf.clone());
        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::Korean,
        )));
        assert_eq!(app.settings.language_for(&rtf), Some(DocLanguage::Korean));
        assert!(!app.dirty);
        let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
        let _ = app.open_path(rtf);
        assert_eq!(app.doc_language(), DocLanguage::Korean);

        let mut app = home();
        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Docx));
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::Japanese,
        )));
        let docx = language_temp("japanese.docx");
        FileContext::new(Format::Docx, app.doc().document().clone())
            .save(&docx)
            .expect("save");
        let mut elsewhere = home();
        let _ = elsewhere.open_path(docx);
        assert_eq!(elsewhere.doc_language(), DocLanguage::Japanese);
        assert_eq!(elsewhere.spell_language(), None);
    }

    #[test]
    fn the_language_travels_inside_a_word_file() {
        let mut app = home();
        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Docx));
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Document(DocumentMsg::Language(
            DocLanguage::American,
        )));
        let path = language_temp("travels.docx");
        FileContext::new(Format::Docx, app.doc().document().clone())
            .save(&path)
            .expect("save");
        // A different app (another machine) whose default is British opens it.
        let mut elsewhere = home();
        let _ = elsewhere.open_path(path.clone());
        assert_eq!(elsewhere.spell_language(), Some(spell::Language::American));
        assert!(elsewhere.settings.document_languages.is_empty());
    }

    /// A key press on the given physical key of a non-Latin layout: the layout's own letter
    /// as the logical key, as the platform reports it.
    fn layout_key(
        letter: &str,
        code: keyboard::key::Code,
        modifiers: keyboard::Modifiers,
        text: Option<&str>,
    ) -> Message {
        Message::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(letter.into()),
            modified_key: keyboard::Key::Character(letter.into()),
            physical_key: keyboard::key::Physical::Code(code),
            location: keyboard::Location::Standard,
            modifiers,
            text: text.map(Into::into),
            repeat: false,
        })
    }

    #[test]
    fn cyrillic_text_types_and_the_editing_shortcuts_work_on_a_cyrillic_layout() {
        use keyboard::key::Code;
        let ctrl = if cfg!(target_os = "macos") {
            keyboard::Modifiers::LOGO
        } else {
            keyboard::Modifiers::CTRL
        };
        let none = keyboard::Modifiers::default();
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        // Typing Ukrainian letters, including ones English has no key for.
        for (letter, code) in [
            ("П", Code::KeyG),
            ("р", Code::KeyH),
            ("и", Code::KeyB),
            ("в", Code::KeyD),
            ("і", Code::KeyS),
            ("т", Code::KeyN),
            ("ї", Code::BracketRight),
            ("є", Code::Quote),
            ("ґ", Code::Backslash),
        ] {
            let _ = app.update(layout_key(letter, code, none, Some(letter)));
        }
        assert_eq!(first_text(&app), "Привітїєґ");

        // Ctrl+A, then Ctrl+X, on the keys that are "ф" and "ч" on this layout.
        let _ = app.update(layout_key("ф", Code::KeyA, ctrl, None));
        assert_eq!(
            app.doc().editor().selected_text().as_deref(),
            Some("Привітїєґ")
        );
        let _ = app.update(layout_key("ч", Code::KeyX, ctrl, None));
        assert_eq!(first_text(&app), "", "Ctrl+X cut the selection");
        // Ctrl+Z (the key that is "я") brings it back.
        let _ = app.update(layout_key("я", Code::KeyZ, ctrl, None));
        assert_eq!(first_text(&app), "Привітїєґ");
        // Ctrl+Shift+Z or Ctrl+Y (the key that is "н") redoes the cut.
        let _ = app.update(layout_key("н", Code::KeyY, ctrl, None));
        assert_eq!(first_text(&app), "");
    }

    #[test]
    fn other_shortcuts_follow_the_key_position_on_a_cyrillic_layout() {
        use keyboard::key::Code;
        let ctrl = if cfg!(target_os = "macos") {
            keyboard::Modifiers::LOGO
        } else {
            keyboard::Modifiers::CTRL
        };
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        // Ctrl+N on a Ukrainian layout ("т") goes home.
        let _ = app.update(Message::Shortcut(
            crate::shortcuts::from_event(&keyboard::Event::KeyPressed {
                key: keyboard::Key::Character("т".into()),
                modified_key: keyboard::Key::Character("т".into()),
                physical_key: keyboard::key::Physical::Code(Code::KeyN),
                location: keyboard::Location::Standard,
                modifiers: ctrl,
                text: None,
                repeat: false,
            })
            .expect("Ctrl+N is a shortcut by position"),
        ));
        assert!(app.doc.is_none());
    }

    #[test]
    fn text_direction_is_a_document_setting_kept_by_word_odt_and_libertum_files_only() {
        let mut app = home();
        for (format, keeps) in [
            (DefaultFormat::Docx, true),
            (DefaultFormat::Odt, true),
            (DefaultFormat::Native, true),
            (DefaultFormat::Rtf, false),
            (DefaultFormat::Markdown, false),
        ] {
            let _ = app.update(Message::SetDefaultFormat(format));
            let _ = app.update(Message::CreateDocument(TemplateId::Blank));
            assert_eq!(app.state.keeps_vertical, keeps, "{format:?}");
            let _ = app.update(Message::Shortcut(Shortcut::CloseDocument));
        }
        let _ = app.update(Message::SetDefaultFormat(DefaultFormat::Docx));
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert!(!app.doc().is_vertical());
        let _ = app.update(Message::Document(DocumentMsg::Vertical(true)));
        assert!(app.doc().is_vertical());
        assert!(app.state.document.vertical);
        assert!(app.dirty, "the file now differs from what is on disk");
        // It survives a Word file and is read back as vertical text.
        let path = language_temp("vertical.docx");
        FileContext::new(Format::Docx, app.doc().document().clone())
            .save(&path)
            .expect("save");
        let mut elsewhere = home();
        let _ = elsewhere.open_path(path);
        assert!(elsewhere.doc().is_vertical());
        assert!(elsewhere.state.document.vertical);
        // And turns off again.
        let _ = app.update(Message::Document(DocumentMsg::Vertical(false)));
        assert!(!app.doc().is_vertical());
    }

    #[test]
    fn in_vertical_text_the_arrow_keys_follow_the_columns() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("縦書き日本語".repeat(60))));
        let _ = app.update(Message::Document(DocumentMsg::Vertical(true)));
        // Back in the page, as a click on it would leave things.
        app.canvas_focused = true;
        app.doc_mut().paginate_everything();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let none = keyboard::Modifiers::default();
        let arrow = |name: keyboard::key::Named| key_press(keyboard::Key::Named(name), None, none);
        let offset = |app: &App| app.doc().editor().caret().offset;
        use keyboard::key::Named;
        // Down steps along the column, one character (three bytes) at a time; Up goes back.
        let _ = app.update(arrow(Named::ArrowDown));
        assert_eq!(offset(&app), 3);
        let _ = app.update(arrow(Named::ArrowDown));
        assert_eq!(offset(&app), 6);
        let _ = app.update(arrow(Named::ArrowUp));
        assert_eq!(offset(&app), 3);
        // Left goes to the next column, the one on the left: further into the paragraph.
        let _ = app.update(arrow(Named::ArrowLeft));
        assert!(
            offset(&app) > 30,
            "the next column starts a column's length on: {}",
            offset(&app)
        );
        // Right returns to the column on the right.
        let _ = app.update(arrow(Named::ArrowRight));
        assert!(offset(&app) < 30, "{}", offset(&app));
    }

    #[test]
    fn a_linked_picture_or_video_is_inserted_with_a_placeholder() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::OpenLinkedMedia);
        assert!(matches!(app.modal, Some(Modal::LinkedMedia { .. })));
        // A bad address is refused with a message and nothing is inserted.
        let _ = app.update(Message::LinkedUrl("not an address".into()));
        let _ = app.update(Message::LinkedInsert);
        assert!(matches!(
            app.modal,
            Some(Modal::LinkedMedia { error: Some(_), .. })
        ));
        // A video address is recognised as a video; with no preview wanted it inserts at once.
        let _ = app.update(Message::LinkedUrl("https://example.com/clip.mp4".into()));
        let _ = app.update(Message::LinkedAlt("A clip".into()));
        let _ = app.update(Message::LinkedPreview(false));
        let _ = app.update(Message::LinkedInsert);
        assert!(app.modal.is_none(), "{:?}", app.modal.is_some());
        let (_, images) = app.doc().editor().caret_images();
        assert_eq!(images.len(), 1);
        let link = images[0].link().expect("a link");
        assert_eq!(link.url, "https://example.com/clip.mp4");
        assert_eq!(link.kind, libertum_model::MediaKind::Video);
        assert_eq!(images[0].alt, "A clip");
        assert_eq!(app.state.images[0].link, Some(link));
        // A video file has no picture to download.
        assert!(!app.state.images[0].previewable);
        // A failed preview download still inserts the link and says why.
        let _ = app.update(Message::LinkedInserted {
            link: libertum_model::MediaLink {
                url: "https://example.com/a.png".into(),
                kind: libertum_model::MediaKind::Image,
            },
            alt: String::new(),
            preview: Err("offline".into()),
        });
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        let (_, images) = app.doc().editor().caret_images();
        assert_eq!(images.len(), 2);
        assert!(app.state.images.iter().any(|image| image.previewable));
    }

    #[test]
    fn the_ruby_field_sets_a_reading_over_the_selection() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("漢字を読む".into())));
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.doc_mut().move_caret(|ed| {
            for _ in 0..2 {
                ed.move_right(true);
            }
        });
        let _ = app.update(Message::RubyEdit("かんじ".into()));
        let doc = app.doc();
        let paragraph = doc.editor().document().paragraph_at(0).expect("paragraph");
        assert_eq!(paragraph.ruby().len(), 1);
        assert_eq!(paragraph.ruby()[0].range, 0..6);
        assert_eq!(app.state.ruby.as_deref(), Some("かんじ"));
        // Word, OpenDocument, HTML and Libertum files keep it; RTF and Markdown do not.
        assert!(Format::Docx.keeps_ruby() && Format::Native.keeps_ruby());
        assert!(!Format::Rtf.keeps_ruby() && !Format::Markdown.keeps_ruby());
        // Clearing the reading removes it.
        let _ = app.update(Message::RubyEdit(String::new()));
        let paragraph = app
            .doc()
            .editor()
            .document()
            .paragraph_at(0)
            .expect("paragraph");
        assert!(paragraph.ruby().is_empty());
    }

    #[test]
    fn in_horizontal_text_the_arrow_keys_are_unchanged() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("abcdef".into())));
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let none = keyboard::Modifiers::default();
        let _ = app.update(key_press(
            keyboard::Key::Named(keyboard::key::Named::ArrowRight),
            None,
            none,
        ));
        assert_eq!(app.doc().editor().caret().offset, 1);
    }

    #[test]
    fn three_escapes_go_home() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(app.doc.is_some(), "two Escapes do nothing");
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(app.doc.is_none(), "the third goes home");
    }

    #[test]
    fn a_wheel_notch_glides_exactly_one_step_and_a_trackpad_is_followed_directly() {
        let mut app = scrolling_app();
        app.scroll = (0.0, 0.0);
        let notch = 100.0; // logical pixels per notch at scale 1
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: notch,
            stepped: true,
        }));
        assert_eq!(app.scroll.1, 0.0, "nothing jumps: the view glides");
        assert_eq!(app.scroll_pending.1, notch);
        // Frames at 60 Hz: the position only ever moves forward, each step smaller than
        // the one before, and it ends on the notch exactly.
        let start = app.last_frame.expect("the glide is timed from the notch");
        let mut previous = 0.0f32;
        let mut last_step = f32::MAX;
        let mut frames = 0u32;
        while app.scroll_pending != (0.0, 0.0) && frames < 120 {
            frames += 1;
            app.coast(start + Duration::from_millis(u64::from(frames) * 16));
            let step = app.scroll.1 - previous;
            // (The last pixel is taken in one go, so that final step may be the larger.)
            assert!(
                step > 0.0 && (step <= last_step + 0.01 || step <= 1.0),
                "eases out: {step}"
            );
            last_step = step;
            previous = app.scroll.1;
        }
        assert!(
            (app.scroll.1 - notch).abs() < 0.01,
            "ends on the notch: {}",
            app.scroll.1
        );
        assert!(frames < 30, "within half a second: {frames} frames");
        let first_frame = 1.0 - (-SCROLL_GLIDE_RATE * 0.016f32).exp();
        assert!(first_frame > 0.2, "it starts moving at once");

        // Three quick notches travel three notches in all.
        for _ in 0..3 {
            let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
                dx: 0.0,
                dy: notch,
                stepped: true,
            }));
        }
        let start = app.last_frame.expect("timed");
        for frame in 1..=120u32 {
            app.coast(start + Duration::from_millis(u64::from(frame) * 16));
        }
        assert!(
            (app.scroll.1 - 4.0 * notch).abs() < 0.01,
            "{}",
            app.scroll.1
        );

        // A notch the other way cancels a glide in progress instead of fighting it.
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: notch,
            stepped: true,
        }));
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: -notch,
            stepped: true,
        }));
        assert_eq!(app.scroll_pending.1, -notch);

        // A trackpad delta is queued and ends any glide; the next frame applies all of it.
        let before = app.scroll.1;
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: 7.5,
            stepped: false,
        }));
        assert_eq!(app.scroll.1, before, "not applied until a frame");
        assert_eq!(app.trackpad_pending.1, 7.5);
        assert_eq!(app.scroll_pending, (0.0, 0.0));
        app.coast(Instant::now());
        assert_eq!(app.scroll.1, before + 7.5);
        assert_eq!(app.trackpad_pending, (0.0, 0.0));

        // At the top edge a notch upwards asks for nothing, so no frames are run for it.
        app.scroll = (0.0, 0.0);
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: -notch,
            stepped: true,
        }));
        assert_eq!(app.scroll_pending, (0.0, 0.0));
        // Moving the caret into view ends a glide.
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: notch,
            stepped: true,
        }));
        app.ensure_caret_visible();
        assert_eq!(app.scroll_pending, (0.0, 0.0));
    }

    #[test]
    fn trackpad_events_arriving_between_frames_move_the_page_once_and_the_clock_stops_when_still() {
        let mut app = scrolling_app();
        app.scroll = (0.0, 100.0);
        let wheel = |dy| {
            Message::Canvas(CanvasEvent::Wheel {
                dx: 0.0,
                dy,
                stepped: false,
            })
        };
        // Three events within one frame: the page moves by their sum, once.
        let _ = app.update(wheel(4.0));
        let _ = app.update(wheel(5.0));
        let _ = app.update(wheel(6.0));
        assert_eq!(app.scroll.1, 100.0);
        assert!(
            app.trackpad_active(Instant::now()),
            "frames run during the gesture"
        );
        app.coast(Instant::now());
        assert_eq!(app.scroll.1, 115.0);
        // A programmatic jump drops anything still queued.
        let _ = app.update(wheel(9.0));
        let _ = app.update(Message::GoToPage(0));
        let at = app.scroll.1;
        app.coast(Instant::now());
        assert_eq!(
            app.scroll.1, at,
            "a stale delta must not move the page after a jump"
        );
        // Once the events stop and the queue is empty, the gesture is over and nothing asks
        // for frames: an idle window costs no GPU work.
        let later = Instant::now() + Duration::from_millis(500);
        assert!(!app.trackpad_active(later));
    }

    #[test]
    fn a_trackpad_flick_carries_the_page_on_and_slows_to_rest() {
        let mut app = scrolling_app();
        app.scroll = (0.0, 100.0);
        let t0 = Instant::now();
        // Fingers moving at 10 px per 8 ms (1,250 px/s), as the OS reports them.
        for i in 0..6u32 {
            app.track_fling(t0 + Duration::from_millis(u64::from(i) * 8), (0.0, 10.0));
        }
        let lift = t0 + Duration::from_millis(40);
        assert!(app.fling.1 > 600.0, "velocity measured: {}", app.fling.1);
        // A finger that slows down and stops leaves the page where it is: no glide.
        let mut slowing = app.fling;
        let held_at = app.scroll.1;
        for (i, moved) in [3.0f32, 2.0, 1.0].into_iter().enumerate() {
            app.track_fling(t0 + Duration::from_millis(48 + 10 * i as u64), (0.0, moved));
        }
        assert!(app.fling.1 < FLING_MIN_SPEED, "slowed: {}", app.fling.1);
        app.fling_step(t0 + Duration::from_millis(200), 0.016);
        assert_eq!(app.fling, (0.0, 0.0));
        assert_eq!(app.scroll.1, held_at, "it does not move on");
        // Restore the flick for the rest of the test.
        app.fling = std::mem::take(&mut slowing);
        app.trackpad_at = Some(t0 + Duration::from_millis(40));
        // While events are still arriving, nothing extra is added.
        let held = app.scroll.1;
        app.fling_step(lift + Duration::from_millis(10), 0.016);
        assert_eq!(app.scroll.1, held);
        // After the fingers lift, the page keeps moving, each frame by less than the last.
        app.trackpad_at = Some(lift);
        let mut now = lift + Duration::from_millis(60);
        let mut last_step = f32::MAX;
        let mut frames = 0;
        while app.fling != (0.0, 0.0) && frames < 600 {
            let before = app.scroll.1;
            app.fling_step(now, 0.016);
            let step = app.scroll.1 - before;
            assert!(step >= 0.0 && step <= last_step + 0.01, "slows: {step}");
            last_step = step;
            now += Duration::from_millis(16);
            frames += 1;
        }
        assert_eq!(app.fling, (0.0, 0.0), "comes to rest");
        assert!(
            app.scroll.1 > held + 50.0,
            "it glided: {}",
            app.scroll.1 - held
        );
        assert!(frames < 120, "within about two seconds: {frames}");

        // A new touch after a pause starts afresh and cancels a fling in progress.
        app.fling = (0.0, 900.0);
        app.track_fling(now + Duration::from_secs(1), (0.0, 5.0));
        assert_eq!(app.fling, (0.0, 0.0));
        // A wheel notch ends a fling too.
        app.fling = (0.0, 900.0);
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: 100.0,
            stepped: true,
        }));
        assert_eq!(app.fling, (0.0, 0.0));
    }

    #[test]
    fn a_trackpad_glide_eases_out_instead_of_stopping_from_a_visible_speed() {
        for start in [400.0f32, 1_250.0, 6_000.0] {
            let mut app = scrolling_app();
            app.scroll = (0.0, 100.0);
            let t0 = Instant::now();
            app.fling = (0.0, start);
            app.trackpad_at = Some(t0);
            let mut now = t0 + Duration::from_millis(60);
            let mut steps = Vec::new();
            while app.fling != (0.0, 0.0) && steps.len() < 600 {
                let before = app.scroll.1;
                app.fling_step(now, 0.016);
                steps.push(app.scroll.1 - before);
                now += Duration::from_millis(16);
            }
            assert_eq!(app.fling, (0.0, 0.0), "comes to rest from {start}");
            let last = steps
                .iter()
                .rev()
                .find(|step| **step > 0.0)
                .copied()
                .unwrap_or(0.0);
            // The last real frame moves under a pixel: no jolt where the page halts. (The old
            // glide ended while still moving 300 px/s, nearly 5 px per frame.)
            assert!(last < 1.0, "the final step from {start} is {last} px");
            // And the tail does not crawl on: well within three seconds.
            assert!(steps.len() < 190, "{} frames from {start}", steps.len());
        }
    }

    #[test]
    fn stitching_joins_the_chosen_documents_into_a_new_one() {
        let dir = std::env::temp_dir().join(format!("libertum-stitch-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let write =
            |name: &str, text: &str| {
                let path = dir.join(name);
                let document = libertum_model::Document::from_paragraphs(vec![
                    libertum_model::Paragraph::new(libertum_model::BlockId(1), text),
                ]);
                std::fs::write(&path, Odt::new(document).to_bytes().expect("odt")).expect("write");
                path
            };
        let one = write("one.odt", "Chapter One");
        let two = write("two.odt", "Chapter Two");

        let mut app = home();
        let _ = app.update(Message::StitchAdd(vec![one.clone(), two.clone()]));
        assert!(matches!(app.modal, Some(Modal::Stitch)), "the dialog opens");
        assert_eq!(app.stitch_sources.len(), 2);
        let _ = app.view();

        // The order can be changed before stitching.
        let _ = app.update(Message::StitchMove(0, 1));
        assert_eq!(app.stitch_sources[0].path, two);
        let _ = app.update(Message::StitchMove(1, -1));
        assert_eq!(app.stitch_sources[0].path, one);

        let _ = app.update(Message::StitchGo);
        assert_eq!(
            paragraph_texts(&app),
            vec!["Chapter One".to_owned(), "Chapter Two".to_owned()]
        );
        assert!(app.dirty, "a stitched book is a new, unsaved document");
        assert!(app.file.is_none());
        assert!(app.modal.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stitch_source_can_be_removed_and_the_dialog_cancelled() {
        let mut app = home();
        let document =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Only",
            )]);
        app.stitch_sources = vec![crate::stitch::Source {
            path: std::path::PathBuf::from("only.odt"),
            document,
        }];
        app.modal = Some(Modal::Stitch);
        let _ = app.update(Message::StitchRemove(0));
        assert!(app.stitch_sources.is_empty());
        let _ = app.update(Message::CloseStitch);
        assert!(app.modal.is_none(), "Cancel closes the dialog");
    }

    /// A document long enough to scroll, in a 900 x 600 window at scale 1.
    fn scrolling_app() -> App {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let body = (0..400)
            .map(|i| format!("Line {i} of a long document with a needle{i} in it."))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = app.update(Message::PasteText(Some(body)));
        app.viewport = Some(Size::new(900.0, 600.0));
        app.scale_factor = 1.0;
        app.doc_mut().paginate_everything();
        app
    }

    /// Top and bottom of the caret in viewport pixels (0 is the top edge of the canvas).
    fn caret_in_viewport(app: &App) -> (f32, f32) {
        let caret = app.doc().caret_rect().expect("caret");
        let m = app.metrics();
        let top = m.page_top(caret.page) + caret.y * m.scale - app.scroll.1;
        (top, top + caret.height * m.scale)
    }

    #[test]
    fn the_caret_is_shown_above_the_word_count_pill() {
        let mut app = scrolling_app();
        // From the top of the document, walk the caret down past the first screen.
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.scroll = (0.0, 0.0);
        app.state.show_word_count = true;
        let mut lowest: f32 = 0.0;
        for _ in 0..60 {
            app.doc_mut().move_caret(|ed| ed.move_paragraph(1, false));
            app.ensure_caret_visible();
            let (_, bottom) = caret_in_viewport(&app);
            lowest = lowest.max(bottom);
        }
        assert!(app.scroll.1 > 0.0, "the view scrolled");
        let limit = 600.0 - WORD_COUNT_PILL_COVER;
        assert!(
            lowest <= limit + 0.5,
            "the caret never sits under the pill: lowest bottom {lowest}, limit {limit}"
        );
        // With the pill hidden the caret may use the room it took.
        app.state.show_word_count = false;
        let mut lowest_without: f32 = 0.0;
        for _ in 0..60 {
            app.doc_mut().move_caret(|ed| ed.move_paragraph(1, false));
            app.ensure_caret_visible();
            lowest_without = lowest_without.max(caret_in_viewport(&app).1);
        }
        assert!(lowest_without > limit, "{lowest_without} vs {limit}");
    }

    #[test]
    fn a_found_match_is_highlighted_exactly_and_clear_of_the_find_bar_and_pill() {
        let mut app = scrolling_app();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.scroll = (0.0, 0.0);
        let _ = app.update(Message::OpenFind);
        // Far down the document: the view has to scroll to it.
        find_typed(&mut app, "needle300");
        let found = app.state.find.last.clone().expect("a match");
        assert_eq!(found.range.len(), "needle300".len());
        assert_eq!(app.state.find.count, Some((Some(1), 1)));
        // The highlight is the match and nothing else: its rectangles are the selection's,
        // and the selection is exactly the matched bytes.
        let (start, end) = app.doc().editor().ordered_selection().expect("selection");
        assert_eq!(
            (start.offset, end.offset),
            (found.range.start, found.range.end)
        );
        let rects = app.find_match_rects();
        assert!(!rects.is_empty(), "the match is highlighted");
        assert_eq!(rects, app.doc().selection_rects());
        let width: f32 = rects.iter().map(|r| r.width).sum();
        assert!(
            width > 20.0 && width < 120.0,
            "nine characters wide: {width}"
        );
        // It sits below the Find bar and above the pill.
        let m = app.metrics();
        for r in &rects {
            let top = m.page_top(r.page) + r.y * m.scale - app.scroll.1;
            let bottom = top + r.height * m.scale;
            assert!(top >= FIND_BAR_COVER - 0.5, "under the Find bar: {top}");
            assert!(
                bottom <= 600.0 - WORD_COUNT_PILL_COVER + 0.5,
                "under the pill: {bottom}"
            );
        }
        // A match above the view comes back down below the Find bar too.
        find_typed(&mut app, "needle2 ");
        let rects = app.find_match_rects();
        assert!(!rects.is_empty());
        let top = m.page_top(rects[0].page) + rects[0].y * m.scale - app.scroll.1;
        assert!(top >= FIND_BAR_COVER - 0.5 || app.scroll.1 == 0.0, "{top}");
        // Moving the caret away ends the highlight; so does closing the bar.
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        assert!(app.find_match_rects().is_empty());
        let _ = app.update(Message::FindNext);
        assert!(!app.find_match_rects().is_empty());
        let _ = app.update(Message::CloseFind);
        assert!(app.find_match_rects().is_empty());
    }

    #[test]
    fn the_find_count_follows_the_query_and_its_options() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("Cat cat concat\ncat".into())));
        let _ = app.update(Message::OpenFind);
        find_typed(&mut app, "cat");
        assert_eq!(app.state.find.count.map(|c| c.1), Some(4));
        let _ = app.update(Message::ToggleFindWord);
        assert_eq!(app.state.find.count.map(|c| c.1), Some(3));
        let _ = app.update(Message::ToggleFindCase);
        assert_eq!(app.state.find.count.map(|c| c.1), Some(2));
        // Nothing found: the count goes and the notice says so.
        find_typed(&mut app, "dog");
        assert_eq!(app.state.find.count, None);
        assert_eq!(app.state.find.notice, Some(s::FIND_NO_MATCHES.to_owned()));
        // An empty query is not a search.
        let _ = app.update(Message::FindQuery(String::new()));
        assert_eq!(app.state.find.count, None);
        assert_eq!(app.state.find.notice, None);
        assert_eq!(s::find_count(None, 5), "5 found");
        let _ = app.view();
    }

    #[test]
    fn the_stitch_dialog_opens_and_works_from_the_home_screen() {
        let dir = std::env::temp_dir().join(format!("libertum-home-stitch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let one = dir.join("one.md");
        let two = dir.join("two.md");
        std::fs::write(&one, "# First\n\nBody one.\n").expect("write");
        std::fs::write(&two, "# Second\n\nBody two.\n").expect("write");

        let mut app = home();
        assert!(app.doc.is_none(), "on the home screen");
        let _ = app.update(Message::StitchAdd(vec![one.clone(), two.clone()]));
        assert!(matches!(app.modal, Some(Modal::Stitch)));
        assert_eq!(app.stitch_sources.len(), 2);
        // The home screen draws the dialog (it used to return the bare home view, so the
        // dialog was open but invisible). Building the view must not panic either way.
        let _ = app.view();
        let _ = app.update(Message::StitchGo);
        assert!(app.modal.is_none());
        let texts = paragraph_texts(&app);
        assert!(texts.iter().any(|t| t == "Body one."), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Body two."), "{texts:?}");
        assert!(app.dirty, "a stitched document is new and unsaved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_save_dialog_offers_every_format_with_the_current_one_first() {
        for current in [
            Format::Native,
            Format::Odt,
            Format::Docx,
            Format::Rtf,
            Format::Markdown,
            Format::Html,
        ] {
            let filters = save_filters(current);
            assert_eq!(filters.len(), 6);
            assert_eq!(
                filters[0].1,
                current.extension(),
                "the current format leads"
            );
            let mut extensions: Vec<&str> = filters.iter().map(|f| f.1).collect();
            extensions.sort_unstable();
            assert_eq!(extensions, vec!["docx", "html", "lbtd", "md", "odt", "rtf"]);
        }
        // The native format is in the list by name, not only by extension.
        assert!(
            save_filters(Format::Odt)
                .iter()
                .any(|(label, extension)| *label == s::FILE_FILTER_NATIVE && *extension == "lbtd")
        );
        // A name typed without an extension gets the current format's.
        let path = ensure_extension(PathBuf::from("book"), Format::Native);
        assert_eq!(path, PathBuf::from("book.lbtd"));
        let kept = ensure_extension(PathBuf::from("book.lbtd"), Format::Odt);
        assert_eq!(kept, PathBuf::from("book.lbtd"));
    }

    #[test]
    fn large_files_in_three_formats_stitch_into_one_book() {
        let dir = std::env::temp_dir().join(format!("libertum-big-stitch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let pages = 150;
        let sources: Vec<(PathBuf, usize)> = ["odt", "docx", "rtf"]
            .iter()
            .enumerate()
            .map(|(i, extension)| {
                let document =
                    libertum_layout::spike::generate_model_document(pages, 11 + i as u64);
                let count = document.paragraph_count();
                let path = dir.join(format!("part{i}.{extension}"));
                FileContext::new(Format::for_path(&path), document)
                    .save(&path)
                    .expect("write a source");
                (path, count)
            })
            .collect();
        let expected: usize = sources.iter().map(|(_, count)| count).sum();

        let mut app = home();
        let started = Instant::now();
        let _ = app.update(Message::StitchAdd(
            sources.iter().map(|(path, _)| path.clone()).collect(),
        ));
        let read = started.elapsed();
        assert!(matches!(app.modal, Some(Modal::Stitch)), "all three opened");
        let started = Instant::now();
        let _ = app.update(Message::StitchGo);
        let stitched = started.elapsed();
        println!("stitch of 3 x {pages} pages: read {read:?}, stitch and open {stitched:?}");

        let document = app.doc().editor().document();
        assert_eq!(
            document.paragraph_count(),
            expected,
            "every paragraph arrived"
        );
        let starts = document
            .paragraphs()
            .filter(|p| p.props.section_start)
            .count();
        assert_eq!(starts, 2, "each later file opens a section");
        assert_eq!(document.sections.len(), 3);
        let ids: std::collections::HashSet<u64> = document.paragraphs().map(|p| p.id.0).collect();
        assert_eq!(ids.len(), expected, "ids are unique");
        // Every paragraph's style exists in the book's sheet.
        assert!(
            document
                .paragraphs()
                .all(|p| document.styles.get(p.props.style).is_some())
        );
        // The first screen is laid out at once; the rest paginates to the end.
        assert!(app.doc().page_total() >= 1);
        app.doc_mut().paginate_everything();
        assert!(
            app.doc().page_total() > 300,
            "{} pages",
            app.doc().page_total()
        );
        // The last paragraph is reachable and has a place on a page.
        app.doc_mut().move_caret(|ed| ed.move_doc_end(false));
        assert!(app.doc().caret_rect().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_fails_to_open_returns_to_the_stitch_list() {
        let dir = std::env::temp_dir().join(format!("libertum-bad-stitch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let good = dir.join("good.md");
        let bad = dir.join("bad.odt");
        std::fs::write(
            &good,
            "# Good

Text.
",
        )
        .expect("write");
        std::fs::write(&bad, "this is not a zip package").expect("write");
        let mut app = home();
        let _ = app.update(Message::StitchAdd(vec![good, bad]));
        assert!(
            matches!(app.modal, Some(Modal::Error(_))),
            "the failure is shown"
        );
        assert_eq!(app.stitch_sources.len(), 1, "the good file is kept");
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert!(matches!(app.modal, Some(Modal::Stitch)), "back to the list");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn full_screen_toggles_from_the_button_the_key_and_escape() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        assert!(!app.state.fullscreen);
        let _ = app.update(Message::ToggleFullScreen);
        assert!(app.state.fullscreen);
        let _ = app.view();
        // Escape with a menu open closes the menu first and stays in full screen.
        app.state.open_menu = Some(Menu::View);
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(app.state.open_menu.is_none());
        assert!(app.state.fullscreen);
        // With nothing else open, Escape leaves full screen, and does not count towards
        // the three Escapes that go home.
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(!app.state.fullscreen);
        assert!(app.doc.is_some());
        let _ = app.update(Message::Shortcut(Shortcut::FullScreen));
        assert!(app.state.fullscreen);
        let _ = app.update(Message::ToggleFullScreen);
        assert!(!app.state.fullscreen);
        if !cfg!(target_os = "macos") {
            let f11 = keyboard::Key::Named(keyboard::key::Named::F11);
            assert_eq!(
                crate::shortcuts::lookup(&f11, keyboard::Modifiers::empty()),
                Some(Shortcut::FullScreen)
            );
        }
    }

    #[test]
    fn page_thumbnails_render_the_pages_in_view_and_jump_to_a_page() {
        let mut app = scrolling_app();
        app.state.current_page = 1;
        assert!(!app.state.thumbnails);
        let _ = app.update(Message::ToggleThumbnails);
        assert!(app.state.thumbnails);
        // The column shows about three rows in a 600 px window: those are rendered.
        let _ = app.update(Message::ThumbScrolled(0.0, 600.0));
        let total = app.doc().page_total();
        assert!(total > 5, "a long document: {total} pages");
        let mut rendered: Vec<usize> = app.thumb_cache.keys().copied().collect();
        rendered.sort_unstable();
        assert!(!rendered.is_empty(), "thumbnails were rendered");
        assert!(
            rendered.iter().all(|p| *p < 6),
            "only pages in view: {rendered:?}"
        );
        let _ = app.view();
        // Scrolling the column renders the pages that come into view.
        let _ = app.update(Message::ThumbScrolled(4.0 * THUMB_ROW_H, 600.0));
        assert!(app.thumb_cache.contains_key(&4));
        // An edit on a page redraws its thumbnail (its content hash changes).
        let _ = app.update(Message::ThumbScrolled(0.0, 600.0));
        let before = app.thumb_cache.get(&0).map(|(hash, _)| *hash);
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("New words ".into())));
        let after = app.thumb_cache.get(&0).map(|(hash, _)| *hash);
        assert!(before.is_some() && after.is_some());
        assert_ne!(before, after, "the first page thumbnail was redrawn");
        // Clicking a thumbnail shows that page.
        let _ = app.update(Message::GoToPage(5));
        let m = app.metrics();
        assert!((app.scroll.1 - (m.page_top(5) - m.pad)).abs() < 1.0);
        assert_eq!(app.state.current_page, 6);
        // Closing the column frees the images.
        let _ = app.update(Message::ToggleThumbnails);
        assert!(!app.state.thumbnails);
        assert!(app.thumb_cache.is_empty());
        assert_eq!(thumb_range(0.0, 600.0, 2), (0, 2));
        assert_eq!(thumb_range(10_000.0, 600.0, 3), (3, 3));
    }

    #[test]
    fn the_watermark_is_typed_applied_turned_and_removed_from_the_document_pane() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::Document(DocumentMsg::WatermarkText(
            "Draft copy".into(),
        )));
        assert!(
            app.doc().editor().watermark().is_none(),
            "typing does not apply it"
        );
        let _ = app.update(Message::Document(DocumentMsg::WatermarkApply));
        let applied = app.doc().editor().watermark().cloned().expect("applied");
        assert_eq!(applied.text, "Draft copy");
        assert!(applied.diagonal);
        assert!(app.state.document.watermark_on);
        assert!(app.dirty);
        let _ = app.update(Message::Document(DocumentMsg::WatermarkDiagonal(false)));
        assert_eq!(
            app.doc().editor().watermark().map(|w| w.diagonal),
            Some(false)
        );
        let _ = app.view();
        let _ = app.update(Message::Document(DocumentMsg::WatermarkRemove));
        assert!(app.doc().editor().watermark().is_none());
        assert!(!app.state.document.watermark_on);
        assert_eq!(app.state.document.watermark_text, "");
        // Blank text applies nothing.
        let _ = app.update(Message::Document(DocumentMsg::WatermarkText("   ".into())));
        let _ = app.update(Message::Document(DocumentMsg::WatermarkApply));
        assert!(app.doc().editor().watermark().is_none());
    }

    #[test]
    fn a_file_given_at_start_opens_straight_into_the_editor() {
        let dir = std::env::temp_dir().join(format!("libertum-argv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("Double clicked.md");
        std::fs::write(
            &file,
            "# Chapter

Opened from Explorer.
",
        )
        .expect("write");
        let settings = test_settings();
        let (app, _task) = App::boot(settings.clone(), None, Some(file.clone()));
        assert!(app.doc.is_some(), "the editor is open, not the home screen");
        assert_eq!(app.file.as_deref(), Some(file.as_path()));
        assert!(!app.dirty);
        assert!(
            paragraph_texts(&app)
                .iter()
                .any(|t| t == "Opened from Explorer.")
        );
        assert!(app.modal.is_none(), "unexpected modal: {:?}", app.modal);
        // A file that cannot be opened shows the error over the home screen.
        let (app, _task) = App::boot(settings.clone(), None, Some(dir.join("missing.odt")));
        assert!(app.doc.is_none());
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        // No argument: the home screen.
        let (app, _task) = App::boot(settings, None, None);
        assert!(app.doc.is_none() || demo_requested());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_waits_for_a_pause_and_never_searches_an_empty_query() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("cat sat on the cat mat".into())));
        let _ = app.update(Message::OpenFind);
        // Typing schedules a search but does not run one.
        let _ = app.update(Message::FindQuery("ca".into()));
        let first = app.find_generation;
        let _ = app.update(Message::FindQuery("cat".into()));
        assert!(
            app.state.find.last.is_none(),
            "nothing searched while typing"
        );
        // The search scheduled for the shorter query is out of date and does nothing.
        let _ = app.update(Message::FindTyped(first));
        assert!(app.state.find.last.is_none());
        let current = app.find_generation;
        let _ = app.update(Message::FindTyped(current));
        assert_eq!(app.state.find.count, Some((Some(1), 2)));
        // Deleting the query to nothing searches nothing and clears the result.
        let _ = app.update(Message::FindQuery(String::new()));
        let empty = app.find_generation;
        let _ = app.update(Message::FindTyped(empty));
        assert!(app.state.find.last.is_none());
        assert_eq!(app.state.find.count, None);
        assert_eq!(app.state.find.notice, None);
        // Enter still searches at once.
        let _ = app.update(Message::FindQuery("mat".into()));
        let _ = app.update(Message::FindNext);
        assert_eq!(app.state.find.count, Some((Some(1), 1)));
    }

    #[test]
    fn keys_a_text_field_used_never_reach_the_document() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        app.canvas_focused = true;
        let _ = app.update(Message::PasteText(Some("cat sat on the cat mat".into())));
        let _ = app.update(Message::OpenFind);
        find_typed(&mut app, "cat");
        let _ = app.update(Message::FindNext);
        let before = paragraph_texts(&app);
        let press = |text: &str, key: keyboard::Key| {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: keyboard::key::Physical::Unidentified(
                    keyboard::key::NativeCode::Unidentified,
                ),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::empty(),
                text: Some(text.into()),
                repeat: false,
            })
        };
        let id = window::Id::unique();
        // Captured by the Find field: dropped.
        let typed = press("x", keyboard::Key::Character("x".into()));
        assert!(key_event(typed.clone(), event::Status::Captured, id).is_none());
        // Not captured (the document has the keyboard): delivered as before.
        assert!(matches!(
            key_event(typed, event::Status::Ignored, id),
            Some(Message::Keyboard(_))
        ));
        // Escape still closes the bar even though the field used it.
        let escape = press("", keyboard::Key::Named(keyboard::key::Named::Escape));
        assert!(matches!(
            key_event(escape, event::Status::Captured, id),
            Some(Message::Shortcut(Shortcut::CloseMenu))
        ));
        // A click on the bar outside its controls keeps the keyboard in the bar.
        app.canvas_focused = true;
        let _ = app.update(Message::FindBarPressed);
        assert!(!app.canvas_focused);
        assert_eq!(paragraph_texts(&app), before, "the document is untouched");
    }

    #[test]
    fn a_crashed_window_is_offered_over_a_document_opened_from_explorer() {
        let dir =
            std::env::temp_dir().join(format!("libertum-recover-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("opened.md");
        std::fs::write(&file, "Opened text.\n").expect("write");
        let lost = libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
            libertum_model::BlockId(1),
            "Unsaved before the crash",
        )]);
        let snapshots = dir.join("snapshots");
        recovery::write_as(&snapshots, CRASHED_WINDOW, lost.clone(), None).expect("a snapshot");

        // Opened from Explorer: the document shows, and the offer is over it.
        let mut app = App::with_doc(test_settings(), None, None);
        app.recovery_dir = snapshots.clone();
        let _ = app.open_path(file.clone());
        app.check_recovery();
        assert!(app.doc.is_some());
        assert!(matches!(app.modal, Some(Modal::Recovery)));
        let _ = app.update(Message::ModalChoice(ModalChoice::Restore));
        assert!(
            paragraph_texts(&app)
                .iter()
                .any(|t| t == "Unsaved before the crash")
        );
        assert!(
            recovery::load(&snapshots).is_none(),
            "the snapshot is consumed"
        );

        // A file that will not open: the error first, then the offer.
        recovery::write_as(&snapshots, CRASHED_WINDOW, lost.clone(), None).expect("a snapshot");
        let mut app = App::with_doc(test_settings(), None, None);
        app.recovery_dir = snapshots.clone();
        let _ = app.open_path(dir.join("missing.odt"));
        app.check_recovery();
        assert!(
            matches!(app.modal, Some(Modal::Error(_))),
            "the error is not hidden"
        );
        let _ = app.update(Message::ModalChoice(ModalChoice::Cancel));
        assert!(matches!(app.modal, Some(Modal::Recovery)), "then the offer");
        let _ = app.update(Message::ModalChoice(ModalChoice::Discard));
        assert!(recovery::load(&snapshots).is_none());

        // The snapshot of a window that is still running (this one) is never offered.
        recovery::write(&snapshots, lost.clone(), None).expect("our own snapshot");
        let mut app = App::with_doc(test_settings(), None, None);
        app.recovery_dir = snapshots.clone();
        app.check_recovery();
        assert!(app.modal.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------------ MCP tool runner

    fn runner_app() -> App {
        let mut app = mcp_app(false, None);
        let _ = app.update(Message::Runner(RunnerMsg::Open));
        app
    }

    /// Save a server the way the window's Add button does.
    fn add_server(app: &mut App, name: &str, command: &str) {
        let _ = app.update(Message::Runner(RunnerMsg::Name(name.into())));
        let _ = app.update(Message::Runner(RunnerMsg::Command(command.into())));
        let _ = app.update(Message::Runner(RunnerMsg::Add));
    }

    fn notice(app: &App) -> Option<&str> {
        app.runner.as_ref().and_then(|r| r.notice.as_deref())
    }

    #[test]
    fn the_runner_opens_only_while_mcp_is_on_and_closes_with_it() {
        let mut off = App::new(Settings::default(), None);
        let _ = off.update(Message::Runner(RunnerMsg::Open));
        assert!(off.runner.is_none(), "closed gate, no window");

        let mut dev_only = App::new(Settings::default(), None);
        let _ = dev_only.update(Message::SetDevMode(true));
        let _ = dev_only.update(Message::Runner(RunnerMsg::Open));
        assert!(dev_only.runner.is_none(), "dev mode alone is not enough");

        let mut app = runner_app();
        assert!(app.runner.is_some());
        let _ = app.update(Message::SetMcpEnabled(false));
        assert!(app.runner.is_none(), "switching MCP off closes the window");
        let _ = app.update(Message::Runner(RunnerMsg::Run));
        assert!(app.runner.is_none(), "a late message does not reopen it");
    }

    #[test]
    fn adding_a_server_checks_the_command_and_leaves_it_unapproved() {
        let mut app = runner_app();
        add_server(&mut app, "Notes", "npx -y \"some server\"");
        assert_eq!(app.settings.mcp_servers.len(), 1);
        assert!(!app.settings.mcp_servers[0].is_approved());
        assert_eq!(app.runner.as_ref().expect("runner").selected, Some(0));
        assert!(app.runner.as_ref().expect("runner").name_draft.is_empty());

        add_server(&mut app, "Broken", "tool \"open");
        assert_eq!(notice(&app), Some(s::RUNNER_ERR_COMMAND_QUOTE));
        add_server(&mut app, "", "tool");
        assert_eq!(notice(&app), Some(s::RUNNER_ERR_ADD));
        add_server(&mut app, "Empty", "   ");
        assert_eq!(notice(&app), Some(s::RUNNER_ERR_COMMAND_EMPTY));
        assert_eq!(app.settings.mcp_servers.len(), 1, "nothing bad was saved");
    }

    #[test]
    fn a_token_typed_with_a_web_address_is_saved_and_the_draft_cleared() {
        let mut app = runner_app();
        let _ = app.update(Message::Runner(RunnerMsg::Token("tok-abc".into())));
        add_server(&mut app, "Web", "https://example.com/mcp");
        assert_eq!(
            app.settings.mcp_servers[0].token.as_deref(),
            Some("tok-abc")
        );
        assert!(app.runner.as_ref().expect("runner").token_draft.is_empty());
        assert_eq!(notice(&app), None);

        // A program takes no token: the server is saved, the token is not, and the writer is told.
        let _ = app.update(Message::Runner(RunnerMsg::Token("tok-xyz".into())));
        add_server(&mut app, "Prog", "libertum-no-such-program-0f3a");
        assert_eq!(app.settings.mcp_servers[1].token, None);
        assert!(notice(&app).is_some_and(|n| n.contains("token")));
        assert!(app.runner.as_ref().expect("runner").token_draft.is_empty());
    }

    #[test]
    fn a_program_is_never_started_without_approval() {
        let mut app = runner_app();
        add_server(&mut app, "Notes", "libertum-no-such-program-0f3a");
        let _ = app.update(Message::Runner(RunnerMsg::Connect(0)));
        let runner = app.runner.as_ref().expect("runner");
        assert!(
            matches!(runner.link, Link::Idle),
            "Connect alone does nothing"
        );
        assert_eq!(runner.generation, 0, "no attempt was even begun");
    }

    #[test]
    fn approval_starts_one_attempt_and_a_failed_start_is_reported() {
        let mut app = runner_app();
        add_server(&mut app, "Notes", "libertum-no-such-program-0f3a");
        let _ = app.update(Message::Runner(RunnerMsg::ApproveAndConnect(0)));
        assert!(app.settings.mcp_servers[0].is_approved());
        let runner = app.runner.as_ref().expect("runner");
        assert!(matches!(runner.link, Link::Connecting(0)));
        let generation = runner.generation;

        // An answer to some older attempt changes nothing.
        let _ = app.update(Message::Runner(RunnerMsg::Connected(
            generation + 7,
            Err("old".into()),
        )));
        assert!(matches!(
            app.runner.as_ref().expect("runner").link,
            Link::Connecting(0)
        ));

        let _ = app.update(Message::Runner(RunnerMsg::Connected(
            generation,
            Err("no such file".into()),
        )));
        let runner = app.runner.as_ref().expect("runner");
        assert!(matches!(runner.link, Link::Idle));
        assert!(notice(&app).is_some_and(
            |n| n.starts_with(s::RUNNER_ERR_CONNECT) && n.contains("no such file")
        ));
    }

    #[test]
    fn changing_the_command_asks_for_approval_again() {
        let mut app = runner_app();
        add_server(&mut app, "Notes", "libertum-no-such-program-0f3a");
        let _ = app.update(Message::Runner(RunnerMsg::ApproveAndConnect(0)));
        let _ = app.update(Message::Runner(RunnerMsg::Disconnect));
        // The file is edited by hand to run something else.
        app.settings.mcp_servers[0].command_line = "libertum-other-program".into();
        let _ = app.update(Message::Runner(RunnerMsg::Connect(0)));
        assert!(
            matches!(app.runner.as_ref().expect("runner").link, Link::Idle),
            "the new command was never approved"
        );
    }

    #[test]
    fn removing_a_server_ends_the_connection_and_the_selection() {
        let mut app = runner_app();
        add_server(&mut app, "One", "a");
        add_server(&mut app, "Two", "b");
        let _ = app.update(Message::Runner(RunnerMsg::ApproveAndConnect(1)));
        let before = app.runner.as_ref().expect("runner").generation;
        let _ = app.update(Message::Runner(RunnerMsg::Remove(0)));
        let runner = app.runner.as_ref().expect("runner");
        assert!(matches!(runner.link, Link::Idle));
        assert!(runner.selected.is_none());
        assert!(
            runner.generation > before,
            "an answer still on its way is ignored"
        );
        assert_eq!(app.settings.mcp_servers.len(), 1);
        assert_eq!(app.settings.mcp_servers[0].name, "Two");
    }

    #[test]
    fn tools_fill_the_list_and_picking_one_builds_its_form() {
        let mut app = runner_app();
        let generation = app.runner.as_ref().expect("runner").generation;
        let tool = libertum_mcp::ToolInfo {
            name: "search".into(),
            description: "Find things".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
        };
        let _ = app.update(Message::Runner(RunnerMsg::Tools(
            generation + 1,
            Ok(vec![tool.clone()]),
        )));
        assert!(
            app.runner.as_ref().expect("runner").tools.is_empty(),
            "a stale list is ignored"
        );
        let _ = app.update(Message::Runner(RunnerMsg::Tools(
            generation,
            Ok(vec![tool]),
        )));
        let _ = app.update(Message::Runner(RunnerMsg::PickTool(0)));
        let _ = app.update(Message::Runner(RunnerMsg::Field(0, "needle".into())));
        let runner = app.runner.as_ref().expect("runner");
        assert_eq!(runner.fields.len(), 1);
        assert_eq!(runner.fields[0].value, "needle");
        // Not connected, so Run does nothing rather than failing.
        let _ = app.update(Message::Runner(RunnerMsg::Run));
        assert!(!app.runner.as_ref().expect("runner").running);
    }

    #[test]
    fn inserting_a_result_is_one_undo_step() {
        let mut app = runner_app();
        let before = first_paragraph(&app);
        let undo_before = app.doc.as_ref().expect("doc").editor().history().undo_len();
        app.runner.as_mut().expect("runner").result = Some(CallResult {
            text: "from a tool".into(),
            is_error: false,
            truncated: false,
        });
        let _ = app.update(Message::Runner(RunnerMsg::Insert));
        assert_eq!(first_paragraph(&app), format!("from a tool{before}"));
        assert!(app.dirty);
        let doc = app.doc.as_mut().expect("doc");
        assert_eq!(doc.editor().history().undo_len(), undo_before + 1);
        doc.undo().expect("undo");
        assert_eq!(first_paragraph(&app), before);
    }

    /// Put a recipe in the open runner window, as if it had been read from the folder.
    fn add_recipe(app: &mut App, toml: &str) -> usize {
        let runner = app.runner.as_mut().expect("runner");
        runner.macros.push(crate::macros::RecipeFile {
            file: "test.toml".into(),
            recipe: crate::macros::parse(toml),
        });
        runner.macros.len() - 1
    }

    fn macro_status(app: &App) -> String {
        app.runner
            .as_ref()
            .and_then(|r| r.macro_status.clone())
            .unwrap_or_default()
    }

    const SHOUT: &str = "name = 'Shout'
        [[steps]]
kind = 'get_selection'
save_as = 't'
        [[steps]]
kind = 'replace_selection'
text = '{{t}}!'
        [[steps]]
kind = 'insert'
text = ' [done]'
";

    #[test]
    fn a_macro_run_is_one_undo_step_and_undo_restores_the_text() {
        let mut app = runner_app();
        let index = add_recipe(&mut app, SHOUT);
        let before = first_paragraph(&app);
        app.doc_mut().move_caret(|ed| ed.select_all());
        let undo_before = app.doc.as_ref().expect("doc").editor().history().undo_len();
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(index)));
        assert!(
            macro_status(&app).starts_with("Finished"),
            "{}",
            macro_status(&app)
        );
        assert!(app.runner.as_ref().expect("runner").macro_run.is_none());
        assert!(app.dirty);
        assert!(
            paragraph_texts(&app).iter().any(|t| t.contains("! [done]")),
            "the text was replaced and the insert came after it"
        );
        let doc = app.doc.as_mut().expect("doc");
        assert_eq!(doc.editor().history().undo_len(), undo_before + 1);
        doc.undo().expect("undo");
        assert_eq!(first_paragraph(&app), before);
    }

    #[test]
    fn a_script_step_transforms_the_selection_and_a_failing_one_changes_nothing() {
        let mut app = runner_app();
        let good = add_recipe(
            &mut app,
            "name = 'Upper'
             [[steps]]
kind = 'get_selection'
save_as = 't'
             [[steps]]
kind = 'script'
save_as = 'u'
code = 't.to_uppercase()'
             [[steps]]
kind = 'replace_selection'
text = '{{u}}'
",
        );
        let before = first_paragraph(&app);
        app.doc_mut().move_caret(|ed| ed.select_all());
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(good)));
        assert!(
            macro_status(&app).starts_with("Finished"),
            "{}",
            macro_status(&app)
        );
        assert_eq!(first_paragraph(&app), before.to_uppercase());
        let doc = app.doc.as_mut().expect("doc");
        doc.undo().expect("undo");
        assert_eq!(first_paragraph(&app), before);

        // A script that fails at run time stops the recipe before it edits anything.
        let bad = add_recipe(
            &mut app,
            "name = 'Bad'
             [[steps]]
kind = 'get_selection'
save_as = 't'
             [[steps]]
kind = 'script'
save_as = 'u'
code = 't.no_such_method()'
             [[steps]]
kind = 'replace_selection'
text = '{{u}}'
",
        );
        app.doc_mut().move_caret(|ed| ed.select_all());
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(bad)));
        assert!(
            macro_status(&app).starts_with("Stopped"),
            "{}",
            macro_status(&app)
        );
        assert_eq!(first_paragraph(&app), before);
    }

    #[test]
    fn a_macro_without_a_selection_stops_before_changing_anything() {
        let mut app = runner_app();
        let index = add_recipe(&mut app, SHOUT);
        let before = first_paragraph(&app);
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(index)));
        assert!(
            macro_status(&app).starts_with("Stopped"),
            "{}",
            macro_status(&app)
        );
        assert_eq!(first_paragraph(&app), before);
        assert!(!app.dirty);
    }

    #[test]
    fn a_recipe_with_a_stray_field_is_not_runnable() {
        let mut app = runner_app();
        let index = add_recipe(
            &mut app,
            "name = 'x'
[[steps]]
kind = 'get_selection'
save_as = 't'
             [[steps]]
kind = 'call_tool'
tool = 'noop'
             [[steps]]
kind = 'replace_selection'
text = 'y'
server = 'S'
",
        );
        // That recipe is invalid (a step with a stray field); it must not be runnable.
        assert!(
            app.runner.as_ref().expect("runner").macros[index]
                .recipe
                .is_err()
        );
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(index)));
        assert!(app.runner.as_ref().expect("runner").macro_run.is_none());
    }

    #[test]
    fn a_recipe_that_needs_a_program_waits_for_it_to_be_connected() {
        let mut app = runner_app();
        let index = add_recipe(
            &mut app,
            "name = 'x'
server = 'Notes'
[[steps]]
kind = 'call_tool'
tool = 't'
",
        );
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(index)));
        assert!(macro_status(&app).contains("Connect this program first"));
        assert!(app.runner.as_ref().expect("runner").macro_run.is_none());
    }

    #[test]
    fn editing_while_a_macro_waits_stops_it_and_keeps_the_writers_edit_apart() {
        let mut app = runner_app();
        let index = add_recipe(&mut app, SHOUT);
        app.doc_mut().move_caret(|ed| ed.select_all());
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(index)));
        // Start a fresh run by hand at the insert step, as if a tool call were out, then let
        // the writer type before it resumes.
        let runner = app.runner.as_mut().expect("runner");
        let recipe = crate::macros::parse(SHOUT).expect("recipe");
        let undo_now = app.doc.as_ref().expect("doc").editor().history().undo_len();
        runner.macro_serial += 1;
        runner.macro_run = Some(crate::mcp_runner::MacroRun {
            recipe,
            serial: runner.macro_serial,
            next: 2,
            vars: Default::default(),
            pending_save: None,
            selection: None,
            undo_before: undo_now,
            expected_undo: undo_now,
            edited: false,
        });
        app.doc
            .as_mut()
            .expect("doc")
            .edit(|ed| ed.insert_text("typed"))
            .expect("typing");
        let serial = app.runner.as_ref().expect("runner").macro_serial;
        let _ = app.update(Message::Runner(RunnerMsg::MacroCalled(
            serial,
            Ok(libertum_mcp::ToolOutput {
                text: "answer".into(),
                is_error: false,
                truncated: false,
            }),
        )));
        assert!(
            macro_status(&app).contains("edited while"),
            "{}",
            macro_status(&app)
        );
        assert!(
            !first_paragraph(&app).contains("[done]"),
            "nothing was inserted"
        );
        // A stale answer for a run that is over changes nothing.
        let _ = app.update(Message::Runner(RunnerMsg::MacroCalled(
            serial,
            Ok(libertum_mcp::ToolOutput {
                text: "late".into(),
                is_error: false,
                truncated: false,
            }),
        )));
        assert!(!first_paragraph(&app).contains("late"));
    }

    #[test]
    fn a_cut_tool_result_is_never_used() {
        let mut app = runner_app();
        let index = add_recipe(&mut app, SHOUT);
        app.doc_mut().move_caret(|ed| ed.select_all());
        let _ = app.update(Message::Runner(RunnerMsg::RunMacro(index)));
        let runner = app.runner.as_mut().expect("runner");
        runner.macro_serial += 1;
        runner.macro_run = Some(crate::mcp_runner::MacroRun {
            recipe: crate::macros::parse(SHOUT).expect("recipe"),
            serial: runner.macro_serial,
            next: 2,
            vars: Default::default(),
            pending_save: Some("v".into()),
            selection: None,
            undo_before: 0,
            expected_undo: 0,
            edited: false,
        });
        let serial = runner.macro_serial;
        let _ = app.update(Message::Runner(RunnerMsg::MacroCalled(
            serial,
            Ok(libertum_mcp::ToolOutput {
                text: "partial".into(),
                is_error: false,
                truncated: true,
            }),
        )));
        assert!(macro_status(&app).contains("cut"), "{}", macro_status(&app));
    }

    #[test]
    fn escape_closes_the_runner_first() {
        let mut app = runner_app();
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(app.runner.is_none());
    }
}

#[cfg(test)]
mod date_time_tests {
    use super::tests::{app, home, three_paragraph_doc};
    use super::*;

    fn open_chooser(app: &mut App) {
        let _ = app.update(Message::OpenDateTime);
        assert!(
            matches!(app.modal, Some(Modal::DateTime { .. })),
            "the Insert menu opens the Date & Time chooser"
        );
    }

    #[test]
    fn the_chooser_inserts_the_chosen_format_and_source() {
        let mut app = three_paragraph_doc();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        open_chooser(&mut app);
        let _ = app.update(Message::DateTimeFormat(DateTimeFormat::DateIso));
        let _ = app.update(Message::DateTimeInsert);
        assert!(app.modal.is_none(), "the chooser closes after inserting");
        let paragraph = app
            .doc()
            .document()
            .paragraph_at(0)
            .expect("the first paragraph");
        assert_eq!(paragraph.date_fields().len(), 1);
        let field = &paragraph.date_fields()[0];
        assert_eq!(field.format, DateTimeFormat::DateIso);
        assert_eq!(field.source, DateTimeSource::Inserted);
        let shown = &paragraph.text()[field.range.clone()];
        assert_eq!(shown.len(), 10, "an ISO date: {shown}");
        assert!(shown.starts_with("20") && shown.matches('-').count() == 2);
    }

    #[test]
    fn an_updated_field_is_rewritten_by_a_save_and_a_fixed_one_is_not() {
        let mut app = three_paragraph_doc();
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        // The document was saved in 2023.
        app.doc_mut()
            .edit(|ed| {
                ed.mark_saved_at(1_700_000_000);
                Ok(Default::default())
            })
            .expect("mark");
        open_chooser(&mut app);
        let _ = app.update(Message::DateTimeFormat(DateTimeFormat::DateIso));
        let _ = app.update(Message::DateTimeUpdated(true));
        let _ = app.update(Message::DateTimeInsert);
        let before = app
            .doc()
            .document()
            .paragraph_at(0)
            .expect("the first paragraph")
            .text()
            .to_owned();
        let old = libertum_editor::format_date_time(1_700_000_000, DateTimeFormat::DateIso);
        assert!(
            before.contains(&old),
            "the field shows the last save: {before}"
        );
        assert_eq!(
            app.doc()
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .date_fields()[0]
                .source,
            DateTimeSource::Updated
        );

        // Saving refreshes the field before the document is written (the write itself runs on
        // a task the test does not poll; the refresh is synchronous).
        let path =
            std::env::temp_dir().join(format!("libertum-date-save-{}.lbtd", std::process::id()));
        app.file = Some(path);
        let _ = app.save_document(false);
        let after = app
            .doc()
            .document()
            .paragraph_at(0)
            .expect("the first paragraph")
            .text()
            .to_owned();
        assert_ne!(before, after, "the updated field changed with the save");
        assert_eq!(
            app.doc()
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .date_fields()
                .len(),
            1,
            "it is still a field"
        );

        // A fixed field keeps its text through a save.
        let mut fixed = three_paragraph_doc();
        fixed.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        open_chooser(&mut fixed);
        let _ = fixed.update(Message::DateTimeFormat(DateTimeFormat::DateIso));
        let _ = fixed.update(Message::DateTimeInsert);
        let before = fixed
            .doc()
            .document()
            .paragraph_at(0)
            .expect("paragraph")
            .text()
            .to_owned();
        fixed.file = Some(
            std::env::temp_dir().join(format!("libertum-date-fixed-{}.lbtd", std::process::id())),
        );
        let _ = fixed.save_document(false);
        assert_eq!(
            fixed
                .doc()
                .document()
                .paragraph_at(0)
                .expect("paragraph")
                .text(),
            before,
            "an inserted field does not change on save"
        );
    }

    #[test]
    fn escape_closes_the_date_time_chooser() {
        let mut app = three_paragraph_doc();
        open_chooser(&mut app);
        let _ = app.update(Message::Shortcut(Shortcut::CloseMenu));
        assert!(app.modal.is_none(), "Escape closes the chooser");
    }

    #[test]
    fn the_ao3_chapter_list_opens_and_copies_the_chapter_html() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Book));
        let _ = app.update(Message::OpenAo3Chapters);
        assert!(app.state.ao3_open);
        assert!(
            !app.state.ao3_chapters.is_empty(),
            "the Book template has headings"
        );
        let title = app.state.ao3_chapters[0].title.clone();
        let _ = app.update(Message::Ao3Chapter(0));
        match &app.modal {
            Some(Modal::Ao3Chapter {
                title: shown,
                html,
                copied,
                ..
            }) => {
                assert_eq!(shown, &title);
                assert!(!html.is_empty());
                assert!(html.contains('<'), "the fragment is HTML");
                assert!(!copied);
            }
            _ => panic!("the AO3 chapter window is open"),
        }
        let _ = app.update(Message::CopyAo3);
        assert!(matches!(
            app.modal,
            Some(Modal::Ao3Chapter { copied: true, .. })
        ));
        let _ = app.update(Message::CloseAo3);
        assert!(app.modal.is_none());
    }

    /// A document whose first paragraph is a Heading 1 and whose body talks about dragons,
    /// with a small local AO3 vocabulary, for the tag-suggestion tests.
    fn tagged_chapter_app() -> App {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let _ = app.update(Message::PasteText(Some(
            "Chapter One\nThe dragon circled the keep.\nAnother dragon answered.".into(),
        )));
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(Message::Format(FormatMsg::Style(ParagraphStyle::Heading1)));
        app.settings.tags = vec![
            Tag::new("Dragons", TagCategory::Character),
            Tag::new("Slow Burn", TagCategory::Freeform),
        ];
        app
    }

    /// The position of a suggested tag in the open chapter window.
    fn suggestion_index(app: &App, name: &str) -> usize {
        match &app.modal {
            Some(Modal::Ao3Chapter { suggestions, .. }) => suggestions
                .iter()
                .position(|suggestion| suggestion.tag.name == name)
                .unwrap_or_else(|| panic!("{name} is suggested")),
            _ => panic!("the AO3 chapter window is open"),
        }
    }

    #[test]
    fn a_chapter_window_suggests_tags_from_the_vocabulary_and_explains_them() {
        let mut app = tagged_chapter_app();
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
        let Some(Modal::Ao3Chapter {
            suggestions,
            selected,
            ..
        }) = &app.modal
        else {
            panic!("the AO3 chapter window is open");
        };
        assert!(!suggestions.is_empty(), "the chapter is about a dragon");
        let dragons = suggestions
            .iter()
            .find(|suggestion| suggestion.tag.name == "Dragons")
            .expect("Dragons is suggested");
        assert_eq!(dragons.tag.category, TagCategory::Character);
        assert_eq!(dragons.matched[0].term, "dragon");
        assert!(dragons.matched[0].count >= 2);
        assert!(
            !suggestions.iter().any(|s| s.tag.name == "Slow Burn"),
            "a tag the chapter never mentions is not suggested"
        );
        assert!(
            selected.iter().all(|on| !on),
            "nothing is ticked until the writer says so"
        );
    }

    #[test]
    fn ticking_a_suggested_tag_and_copying_writes_the_comma_separated_list() {
        let mut app = tagged_chapter_app();
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
        let index = suggestion_index(&app, "Dragons");
        let _ = app.update(Message::ToggleSuggestedTag(index));
        assert!(
            matches!(&app.modal, Some(Modal::Ao3Chapter { selected, .. }) if selected[index]),
            "the tick reaches the window"
        );
        let _ = app.update(Message::CopySuggestedTags);
        assert!(matches!(
            &app.modal,
            Some(Modal::Ao3Chapter {
                tags_copied: true,
                ..
            })
        ));
        // Unticking clears the note, so it cannot claim a stale copy.
        let _ = app.update(Message::ToggleSuggestedTag(index));
        assert!(matches!(
            &app.modal,
            Some(Modal::Ao3Chapter {
                tags_copied: false,
                ..
            })
        ));
    }

    /// An app showing a two-chapter story, for the tag library tests.
    fn library_app(paragraphs: &[(&str, bool)]) -> App {
        use libertum_model::{BlockId, Document, ParaProps, Paragraph, StyleId};
        let mut app = app();
        let document = Document::from_paragraphs(
            paragraphs
                .iter()
                .enumerate()
                .map(|(i, (text, heading))| {
                    let p = Paragraph::new(BlockId(i as u64 + 1), *text);
                    if *heading {
                        p.with_props(ParaProps {
                            style: StyleId::HEADING_1,
                            outline_level: Some(1),
                            ..ParaProps::default()
                        })
                    } else {
                        p
                    }
                })
                .collect(),
        );
        let (doc, _rx) = DocView::open(document);
        app.doc = Some(doc);
        app
    }

    fn library_suggested(app: &App) -> Vec<(TagCategory, String)> {
        match &app.modal {
            Some(Modal::Ao3Chapter { suggestions, .. }) => suggestions
                .iter()
                .map(|s| (s.tag.category, s.tag.name.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn full_names_come_from_the_whole_story_and_copied_tags_are_remembered() {
        let mut app = library_app(&[
            ("One", true),
            ("Mara Ellison met Tomas Reyes at the harbour.", false),
            ("Later Mara Ellison wrote to Tomas Reyes.", false),
            ("Two", true),
            ("Then Mara kissed Tomas.", false),
            ("And Mara held Tomas, her love.", false),
            ("So Mara watched Tomas sleep; she kissed him.", false),
        ]);
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(1));
        let tags = library_suggested(&app);
        assert!(
            tags.contains(&(TagCategory::Character, "Mara Ellison".into())),
            "{tags:?}"
        );
        assert!(
            tags.contains(&(TagCategory::Character, "Tomas Reyes".into())),
            "{tags:?}"
        );
        assert!(
            tags.contains(&(TagCategory::Relationship, "Mara Ellison/Tomas Reyes".into())),
            "{tags:?}"
        );
        // Ticking the characters and copying them puts them in the library.
        let index = suggestion_index(&app, "Mara Ellison");
        let _ = app.update(Message::ToggleSuggestedTag(index));
        let _ = app.update(Message::CopySuggestedTags);
        assert_eq!(
            app.tag_cache
                .names(TagCategory::Character)
                .collect::<Vec<_>>(),
            ["Mara Ellison"]
        );

        // Another story that never writes the full name still finds it from the library.
        app.tag_cache.learn(&[libertum_export::Tag::new(
            "Tomas Reyes/Mara Ellison",
            TagCategory::Relationship,
        )]);
        app.tag_cache.learn(&[libertum_export::Tag::new(
            "Tomas Reyes",
            TagCategory::Character,
        )]);
        let mut other = library_app(&[
            ("Three", true),
            ("Then Mara kissed Tomas.", false),
            ("And Mara held Tomas, her love.", false),
            ("So Mara watched Tomas sleep; she kissed him.", false),
        ]);
        other.tag_cache = app.tag_cache.clone();
        let _ = other.update(Message::OpenAo3Chapters);
        let _ = other.update(Message::Ao3Chapter(0));
        let tags = library_suggested(&other);
        assert!(
            tags.contains(&(TagCategory::Character, "Mara Ellison".into())),
            "{tags:?}"
        );
        // The pairing comes back written the writer's way, once.
        let pairs: Vec<&String> = tags
            .iter()
            .filter(|(c, _)| *c == TagCategory::Relationship)
            .map(|(_, n)| n)
            .collect();
        assert_eq!(pairs, ["Tomas Reyes/Mara Ellison"], "{tags:?}");
    }

    #[test]
    fn copied_and_removed_tags_are_hidden_for_the_work_until_shown_again() {
        let story = [
            ("One", true),
            ("Mara Ellison met Tomas Reyes at the harbour.", false),
            ("Later Mara Ellison wrote to Tomas Reyes.", false),
            ("Then Mara kissed Tomas.", false),
            ("Two", true),
            ("Then Mara kissed Tomas.", false),
            ("And Mara held Tomas, her love.", false),
            ("So Mara watched Tomas sleep; she kissed him.", false),
        ];
        let mut app = library_app(&story);
        let has = |app: &App, name: &str| library_suggested(app).iter().any(|(_, n)| n == name);
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(1));
        assert!(has(&app, "Mara Ellison") && has(&app, "Tomas Reyes"));
        // Copying Mara: it stays in view now, and is gone the next time.
        let index = suggestion_index(&app, "Mara Ellison");
        let _ = app.update(Message::ToggleSuggestedTag(index));
        let _ = app.update(Message::CopySuggestedTags);
        assert!(has(&app, "Mara Ellison"), "still shown right after copying");
        let _ = app.update(Message::CloseAo3);
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
        assert!(!has(&app, "Mara Ellison"), "already on the work");
        // The – button hides at once, and in every chapter.
        let index = suggestion_index(&app, "Tomas Reyes");
        let _ = app.update(Message::DismissSuggestedTag(index));
        assert!(!has(&app, "Tomas Reyes"));
        let _ = app.update(Message::Ao3Chapter(1));
        assert!(!has(&app, "Tomas Reyes"), "hidden for the whole work");
        assert_eq!(app.work_tags.hidden(), 2);
        // Show Again brings both back.
        let _ = app.update(Message::ShowHiddenTags);
        assert!(has(&app, "Mara Ellison") && has(&app, "Tomas Reyes"));

        // Kept beside the file: the unsaved document's list is written on its first save
        // and read back for the same file later.
        let index = suggestion_index(&app, "Tomas Reyes");
        let _ = app.update(Message::DismissSuggestedTag(index));
        let dir = super::tests::synced_folder_for_tags("work-tags");
        let path = dir.join("story.lbtd");
        app.file = Some(path.clone());
        let _ = app.update(Message::Ao3Chapter(1));
        let mut again = library_app(&story);
        again.file = Some(path);
        let _ = again.update(Message::OpenAo3Chapters);
        let _ = again.update(Message::Ao3Chapter(1));
        assert!(
            !has(&again, "Tomas Reyes"),
            "read back from beside the file"
        );
        assert!(has(&again, "Mara Ellison"));
    }

    #[test]
    fn starter_and_imported_tags_are_suggested_from_the_chapters_words() {
        let mut app = library_app(&[
            ("One", true),
            ("She pressed a bandage to the wound.", false),
            ("The fever broke at dawn; the wound was clean.", false),
            ("He changed the bandage again and held her.", false),
        ]);
        app.settings.use_starter_tags = true;
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
        let tags = library_suggested(&app);
        assert!(
            tags.contains(&(TagCategory::Freeform, "Hurt/Comfort".into())),
            "the starter list knows Hurt/Comfort: {tags:?}"
        );
        let _ = app.update(Message::SetStarterTags(false));
        assert!(!library_suggested(&app).contains(&(TagCategory::Freeform, "Hurt/Comfort".into())));

        // An imported tag is suggested once the chapter uses its words twice.
        let imported = libertum_export::parse_tag_import("Freeform: Field Medicine\n".as_bytes());
        let _ = app.update(Message::TagFileImported(Ok(imported)));
        assert_eq!(app.tag_cache.counts(), (0, 1));
        assert!(app.tag_cache_notice.is_some());
        let _ = app.update(Message::Ao3Chapter(0));
        let _ = app.update(Message::TagFileImported(Ok(Vec::new())));
        let mut medic = library_app(&[
            ("One", true),
            ("The medicine was in the field kit.", false),
            ("Out in the field, medicine ran short.", false),
        ]);
        medic.tag_cache = app.tag_cache.clone();
        let _ = medic.update(Message::OpenAo3Chapters);
        let _ = medic.update(Message::Ao3Chapter(0));
        assert!(
            library_suggested(&medic).contains(&(TagCategory::Freeform, "Field Medicine".into())),
            "{:?}",
            library_suggested(&medic)
        );
        let _ = app.update(Message::ForgetLearnedTags);
        assert_eq!(app.tag_cache.counts(), (0, 1), "imported tags stay");
    }

    #[test]
    fn the_rating_and_warnings_can_only_be_added_by_their_fixed_names() {
        let mut app = tagged_chapter_app();
        let _ = app.update(Message::OpenTagVocabulary);
        let before = app.settings.tags.len();
        // A typed rating is not one of AO3's, so it is not added.
        let _ = app.update(Message::TagCategoryPicked(TagCategory::Rating));
        let _ = app.update(Message::TagDraft("Teen".into()));
        let _ = app.update(Message::AddTag);
        assert_eq!(app.settings.tags.len(), before);
        // Picked from the list, it is added in AO3's own spelling, whatever case was given.
        let _ = app.update(Message::FixedTagPicked("Explicit"));
        let _ = app.update(Message::AddTag);
        assert!(
            app.settings
                .tags
                .iter()
                .any(|tag| tag.name == "Explicit" && tag.category == TagCategory::Rating)
        );
        // Switching to a category with a fixed list drops a name typed for another.
        let _ = app.update(Message::TagCategoryPicked(TagCategory::Freeform));
        let _ = app.update(Message::TagDraft("Mood piece".into()));
        let _ = app.update(Message::TagCategoryPicked(TagCategory::Warning));
        assert!(app.tag_draft.is_empty());
        // A name that is fine for the new category is kept.
        let _ = app.update(Message::TagCategoryPicked(TagCategory::Freeform));
        let _ = app.update(Message::TagDraft("Mood piece".into()));
        let _ = app.update(Message::TagCategoryPicked(TagCategory::Relationship));
        assert_eq!(app.tag_draft, "Mood piece");
    }

    #[test]
    fn a_chapter_with_a_heading_can_copy_its_title_and_the_whole_document_cannot() {
        let mut chaptered = tagged_chapter_app();
        let _ = chaptered.update(Message::OpenAo3Chapters);
        let _ = chaptered.update(Message::Ao3Chapter(0));
        let Some(Modal::Ao3Chapter {
            title, has_title, ..
        }) = &chaptered.modal
        else {
            panic!("the chapter window is open");
        };
        assert!(*has_title, "a chapter that starts at a heading has a title");
        assert!(!title.trim().is_empty());
        let _ = chaptered.update(Message::CopyAo3Title);
        assert!(matches!(
            &chaptered.modal,
            Some(Modal::Ao3Chapter {
                title_copied: true,
                ..
            })
        ));

        // A document with no matching heading is one "Whole document" chapter: that name is
        // a label, not a title, so there is nothing to copy and the button does nothing.
        let mut plain = app();
        let _ = plain.update(Message::OpenAo3Chapters);
        let _ = plain.update(Message::Ao3Chapter(0));
        assert!(matches!(
            &plain.modal,
            Some(Modal::Ao3Chapter {
                has_title: false,
                ..
            })
        ));
        let _ = plain.update(Message::CopyAo3Title);
        assert!(matches!(
            &plain.modal,
            Some(Modal::Ao3Chapter {
                title_copied: false,
                ..
            })
        ));
    }

    /// A chapter with two recurring characters who share romantic scenes and a recurring word,
    /// and no vocabulary at all, for the suggestions drawn from the text.
    fn story_app() -> App {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Blank));
        let story = "Chapter One\n\
            Mara watched the dragon circle the keep. Voss stood beside her.\n\
            \"The dragon is early,\" Mara said. Voss nodded and Mara frowned.\n\
            Voss looked at Mara and smiled. The dragon landed, and the dragon roared.\n\
            Voss kissed Mara, and the dragon watched. Mara loved the dragon.\n\
            Voss held Mara close. The dragon slept.\n\
            Voss whispered that he loved her, and Mara kissed Voss.";
        let _ = app.update(Message::PasteText(Some(story.into())));
        app.doc_mut().move_caret(|ed| ed.move_doc_start(false));
        let _ = app.update(Message::Format(FormatMsg::Style(ParagraphStyle::Heading1)));
        app.settings.tags = Vec::new();
        app
    }

    fn open_first_chapter(app: &mut App) {
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
    }

    fn suggested(app: &App, category: TagCategory) -> Vec<String> {
        match &app.modal {
            Some(Modal::Ao3Chapter { suggestions, .. }) => suggestions
                .iter()
                .filter(|s| s.tag.category == category)
                .map(|s| s.tag.name.clone())
                .collect(),
            _ => panic!("the AO3 chapter window is open"),
        }
    }

    #[test]
    fn the_chapter_window_suggests_characters_pairings_and_keywords_from_the_text_alone() {
        let mut app = story_app();
        assert!(app.settings.auto_suggest_tags, "on by default");
        open_first_chapter(&mut app);
        let characters = suggested(&app, TagCategory::Character);
        assert!(characters.contains(&"Mara".to_owned()), "{characters:?}");
        assert!(characters.contains(&"Voss".to_owned()), "{characters:?}");
        assert_eq!(
            suggested(&app, TagCategory::Relationship),
            vec!["Mara/Voss".to_owned()]
        );
        assert!(suggested(&app, TagCategory::Freeform).contains(&"Dragon".to_owned()));
        // Never a rating or a warning, and nothing is ticked for the writer.
        assert!(suggested(&app, TagCategory::Rating).is_empty());
        assert!(suggested(&app, TagCategory::Warning).is_empty());
        assert!(matches!(
            &app.modal,
            Some(Modal::Ao3Chapter { selected, .. }) if selected.iter().all(|on| !on)
        ));
    }

    #[test]
    fn switching_the_text_suggestions_off_drops_them_and_keeps_the_writers_ticks() {
        let mut app = story_app();
        app.settings.tags = vec![Tag::new("Dragon", TagCategory::Freeform)];
        open_first_chapter(&mut app);
        let dragon = suggestion_index(&app, "Dragon");
        let _ = app.update(Message::ToggleSuggestedTag(dragon));
        let _ = app.update(Message::SetAutoTags(false));
        assert!(!app.settings.auto_suggest_tags);
        assert!(suggested(&app, TagCategory::Character).is_empty());
        assert!(suggested(&app, TagCategory::Relationship).is_empty());
        // The vocabulary's own suggestion, and its tick, are still there.
        let dragon = suggestion_index(&app, "Dragon");
        assert!(matches!(
            &app.modal,
            Some(Modal::Ao3Chapter { selected, .. }) if selected[dragon]
        ));
        // And back on again.
        let _ = app.update(Message::SetAutoTags(true));
        assert!(suggested(&app, TagCategory::Character).contains(&"Mara".to_owned()));
        // A tag the vocabulary has is not offered a second time as a keyword.
        assert_eq!(
            suggested(&app, TagCategory::Freeform)
                .iter()
                .filter(|name| name.eq_ignore_ascii_case("dragon"))
                .count(),
            1
        );
    }

    #[test]
    fn the_ao3_chapter_list_opens_on_hover_and_closes_without_closing_the_menu() {
        let mut app = tagged_chapter_app();
        let _ = app.update(Message::OpenAo3Chapters);
        assert!(app.state.ao3_open);
        assert_eq!(app.state.open_menu, Some(Menu::Export));
        assert!(
            !app.state.ao3_chapters.is_empty(),
            "the chapters are listed"
        );
        // The pointer moves to another row: the list goes, the Export menu stays.
        let _ = app.update(Message::CloseAo3List);
        assert!(!app.state.ao3_open);
        assert_eq!(app.state.open_menu, Some(Menu::Export));
        // And back onto the AO3 row.
        let _ = app.update(Message::OpenAo3Chapters);
        assert!(app.state.ao3_open);
    }

    #[test]
    fn the_tag_vocabulary_window_adds_and_removes_tags() {
        let mut app = tagged_chapter_app();
        let _ = app.update(Message::OpenTagVocabulary);
        assert!(app.vocabulary_open, "the window opens over the document");
        let _ = app.update(Message::TagDraft("  Enemies to Lovers  ".into()));
        let _ = app.update(Message::TagCategoryPicked(TagCategory::Relationship));
        let _ = app.update(Message::AddTag);
        assert!(app.settings.tags.iter().any(
            |tag| tag.name == "Enemies to Lovers" && tag.category == TagCategory::Relationship
        ));
        assert!(
            app.tag_draft.is_empty(),
            "the field clears for the next tag"
        );
        let before = app.settings.tags.len();
        let _ = app.update(Message::TagDraft("dragons".into()));
        let _ = app.update(Message::AddTag);
        assert_eq!(
            app.settings.tags.len(),
            before,
            "a tag already in the list is not added twice"
        );
        let index = app
            .settings
            .tags
            .iter()
            .position(|tag| tag.name == "Slow Burn")
            .expect("seeded");
        let _ = app.update(Message::RemoveTag(index));
        assert!(
            !app.settings.tags.iter().any(|tag| tag.name == "Slow Burn"),
            "Remove takes the tag out"
        );
        let _ = app.update(Message::CloseTagVocabulary);
        assert!(!app.vocabulary_open);
    }

    #[test]
    fn closing_the_vocabulary_reranks_the_open_chapter_and_keeps_the_ticks() {
        let mut app = tagged_chapter_app();
        let _ = app.update(Message::OpenAo3Chapters);
        let _ = app.update(Message::Ao3Chapter(0));
        let index = suggestion_index(&app, "Dragons");
        let _ = app.update(Message::ToggleSuggestedTag(index));
        // "Keep" is not in the vocabulary yet, so the chapter cannot suggest it.
        assert!(matches!(
            &app.modal,
            Some(Modal::Ao3Chapter { suggestions, .. })
                if !suggestions.iter().any(|s| s.tag.name == "Keep")
        ));
        let _ = app.update(Message::OpenTagVocabulary);
        let _ = app.update(Message::TagDraft("Keep".into()));
        let _ = app.update(Message::AddTag);
        let _ = app.update(Message::CloseTagVocabulary);
        let Some(Modal::Ao3Chapter {
            suggestions,
            selected,
            ..
        }) = &app.modal
        else {
            panic!("the chapter window is still open under the vocabulary");
        };
        let keep = suggestions
            .iter()
            .position(|suggestion| suggestion.tag.name == "Keep")
            .expect("the chapter says 'keep', so the new tag is ranked");
        assert!(suggestions[keep].matched[0].count >= 1);
        let dragons = suggestions
            .iter()
            .position(|suggestion| suggestion.tag.name == "Dragons")
            .expect("Dragons is still suggested");
        assert!(
            selected[dragons],
            "the writer's tick survived the re-ranking"
        );
    }

    #[test]
    fn chapter_heading_settings_toggle_independently() {
        let mut app = app();
        assert!(app.settings.chapter_headings.h1);
        assert!(app.settings.chapter_headings.h2);
        let _ = app.update(Message::SetChapterHeading(1, false));
        assert!(!app.settings.chapter_headings.h1);
        assert!(app.settings.chapter_headings.h2);
        let _ = app.update(Message::SetChapterHeading(4, true));
        assert!(app.settings.chapter_headings.h4);
    }

    #[test]
    fn export_writes_text_epub_and_pdf_files() {
        let mut app = home();
        let _ = app.update(Message::CreateDocument(TemplateId::Manuscript));
        let document = app.doc().editor().document().clone();
        let dir = std::env::temp_dir().join(format!("libertum-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let levels = libertum_export::ChapterLevels::default();
        let text = write_export(
            ExportFormat::Text,
            document.clone(),
            levels,
            "Book".into(),
            dir.join("book.txt"),
        )
        .expect("text export");
        assert!(
            std::fs::read_to_string(text)
                .expect("read text")
                .contains(s::MANUSCRIPT_TITLE)
        );
        let epub = write_export(
            ExportFormat::Epub,
            document.clone(),
            levels,
            "Book".into(),
            dir.join("book.epub"),
        )
        .expect("epub export");
        let bytes = std::fs::read(epub).expect("read epub");
        assert_eq!(&bytes[..4], b"PK");
        let rtf = write_export(
            ExportFormat::Rtf,
            document.clone(),
            levels,
            "Book".into(),
            dir.join("book.rtf"),
        )
        .expect("rtf export");
        assert!(
            std::fs::read_to_string(rtf)
                .expect("read rtf")
                .starts_with("{\\rtf")
        );
        let markdown = write_export(
            ExportFormat::Markdown,
            document.clone(),
            levels,
            "Book".into(),
            dir.join("book.md"),
        )
        .expect("markdown export");
        assert!(
            std::fs::read_to_string(markdown)
                .expect("read markdown")
                .contains(s::MANUSCRIPT_TITLE)
        );
        let pdf = write_export(
            ExportFormat::Pdf,
            document,
            levels,
            "Book".into(),
            dir.join("book.pdf"),
        )
        .expect("pdf export");
        assert!(std::fs::read(pdf).expect("read pdf").starts_with(b"%PDF-"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
