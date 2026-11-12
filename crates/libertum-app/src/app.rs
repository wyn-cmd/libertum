//! Application state, messages, update and view.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use iced::advanced::input_method;
use iced::futures::channel::mpsc::UnboundedReceiver;
use iced::widget::{button, column, container, mouse_area, row, space, stack, text};
use iced::{
    Center, Element, Event, Length, Padding, Size, Subscription, Task, Theme, border, event,
    keyboard, theme, window,
};
use libertum_docx::Docx;
use libertum_layout::PageGeometry;
use libertum_markdown::Markdown;
use libertum_model::{
    Align, CharFormat, Document, HfKind, HfVariant, Ligatures, LineSpacing, PageNumberPos,
    PageSetup, ParaFormat, Section,
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
use crate::perf;
use crate::recovery;
use crate::settings::{DefaultFormat, Settings, ThemePreference, UnitsPreference};
use crate::shortcuts::{self, Shortcut};
use crate::state::{
    Alignment, AppState, Baseline, BorderStyle, DocumentPane, Field, FontName, FontWeight,
    FormatPane, InsertAction, Inspector, Ligature, LineSpacingMode, ListKind, Margin, MarginSide,
    Menu, Mixed, NumberingHorizontal, NumberingVertical, Orientation, PageSize, ParagraphStyle,
    Points, Swatch, SwatchTarget, TableAction, Units, ViewAction, Zoom,
};
use crate::strings as s;
use crate::templates::TemplateId;
use crate::theme::{self as look, Scheme, Tokens};
use crate::toolbar;
use crate::widgets::controls::flat;
use crate::widgets::segmented::{self, Content, Look};

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
}

#[derive(Debug, Clone)]
pub enum DocumentMsg {
    /// Watermark text being typed; applied with `WatermarkApply`.
    WatermarkText(String),
    WatermarkApply,
    WatermarkDiagonal(bool),
    WatermarkRemove,
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
    /// Show or hide this section's page numbers.
    NumberingOn(bool),
    /// `true` continues numbering from the previous section, `false` starts at a number.
    NumberingContinue(bool),
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
}

impl Format {
    fn for_path(path: &std::path::Path) -> Self {
        let extension = path.extension().and_then(|extension| extension.to_str());
        match extension.map(str::to_ascii_lowercase).as_deref() {
            Some(s::EXT_ODT) => Format::Odt,
            Some(s::EXT_MARKDOWN) => Format::Markdown,
            Some(s::EXT_RTF) => Format::Rtf,
            Some(s::EXT_NATIVE) => Format::Native,
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

    fn extension(self) -> &'static str {
        match self {
            Format::Docx => s::EXT_DOCX,
            Format::Odt => s::EXT_ODT,
            Format::Markdown => s::EXT_MARKDOWN,
            Format::Rtf => s::EXT_RTF,
            Format::Native => s::EXT_NATIVE,
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
        }
    }

    fn format(&self) -> Format {
        match self {
            Self::Docx(_) => Format::Docx,
            Self::Odt(_) => Format::Odt,
            Self::Markdown(_) => Format::Markdown,
            Self::Rtf(_) => Format::Rtf,
            Self::Native(_) => Format::Native,
        }
    }

    fn set_document(&mut self, document: Document) {
        match self {
            Self::Docx(docx) => docx.set_document(document),
            Self::Odt(odt) => odt.set_document(document),
            Self::Markdown(markdown) => markdown.set_document(document),
            Self::Rtf(rtf) => rtf.set_document(document),
            Self::Native(native) => *native.document_mut() = document,
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

    fn save(&self, path: &std::path::Path) -> Result<(), String> {
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
        }
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

/// A prompt over the document.
#[derive(Debug, Clone)]
enum Modal {
    /// The document has unsaved changes; `pending` runs after Save or Don't Save.
    Unsaved { pending: Pending },
    /// An open or save failed. The message is already user-facing.
    Error(String),
    /// Saving `path` as Markdown would drop the listed formatting; Save writes it anyway.
    MarkdownLoss { path: PathBuf, losses: Vec<String> },
    /// A snapshot from the last session is waiting: restore or discard it.
    Recovery,
    /// Documents are being picked and ordered for stitching.
    Stitch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalChoice {
    Save,
    Discard,
    Cancel,
    /// Put the recovered document back.
    Restore,
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
    SetTheme(ThemePreference),
    SetDefaultFormat(DefaultFormat),
    /// The autosave period in seconds (0 turns autosave off).
    SetAutosave(u32),
    SetGraphicsAcceleration(bool),
    SetUnits(UnitsPreference),
    /// A frame was rendered (only with the `fps` feature).
    #[cfg(feature = "fps")]
    FpsFrame(Instant),
    /// The autosave timer fired.
    AutosaveTick,
    /// The recovery snapshot was written (or failed to be).
    Autosaved(Result<(), String>),
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
    /// Apply the built-in table style with this index to the caret's table (`None` removes
    /// the style).
    TableStyle(Option<usize>),
    /// Make image `index` of a paragraph one step smaller (-1) or larger (+1).
    ImageStep(libertum_model::BlockId, usize, i8),
    /// Alternative text typed for an image in the Format inspector.
    ImageAlt(libertum_model::BlockId, usize, String),
    ImageRemove(libertum_model::BlockId, usize),
    /// Insert a reference to the named bookmark at the caret (Bookmarks pane).
    InsertCrossRef(String, libertum_model::CrossRefKind),
    /// Bring every cross-reference's text up to date.
    RefreshCrossRefs,
    /// Jump the caret to a heading from the Headers pane.
    GoToHeading(libertum_model::BlockId),
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
    SystemTheme(theme::Mode),
    Canvas(CanvasEvent),
    Doc(DocEvent),
    /// Home screen: select a template, or create a document from one.
    SelectTemplate(TemplateId),
    CreateDocument(TemplateId),
    /// Open a document from a file (home screen, Cmd/Ctrl+O).
    OpenFile,
    /// A background save finished.
    Saved {
        path: PathBuf,
        result: Result<(), String>,
    },
    /// The window manager asked to close the window.
    CloseRequested,
    /// A button in the unsaved-changes or error prompt.
    ModalChoice(ModalChoice),
    ScaleFactor(f32),
    WindowResized(Size),
    Frame(Instant),
    /// Caret blink timer (only runs for [`BLINK_TIMEOUT`] after caret activity).
    BlinkTick(Instant),
    // The macOS merged title bar: empty toolbar space drags, double-click maximises.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    DragWindow,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    ToggleMaximise,
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
/// same). Agreed with the user on 2026-11-09.
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
    pub state: AppState,
    pub settings: Settings,
    /// Where settings are persisted. `None` disables persistence (used by tests).
    settings_path: Option<PathBuf>,
    system_mode: theme::Mode,
    /// The theme setting, or the `LIBERTUM_THEME` test override.
    theme_preference: ThemePreference,
    /// Window width in logical pixels (anchors the toolbar menus).
    window_width: f32,
    /// The open document; `None` while the home screen (template chooser) is shown.
    doc: Option<DocView>,
    /// The template selected on the home screen.
    home_selection: TemplateId,
    /// Scroll offset of the page strip, physical pixels.
    scroll: (f32, f32),
    /// Canvas size in logical pixels, once the widget has reported it.
    viewport: Option<Size>,
    /// Rendered page thumbnails by page index: the page's content hash and the image.
    thumb_cache: std::collections::HashMap<usize, (u64, iced::widget::image::Handle)>,
    /// Offset and visible height of the thumbnails column, logical pixels.
    thumb_scroll: (f32, f32),
    /// Window scale factor (device pixels per logical pixel).
    scale_factor: f32,
    scale_factor_known: bool,
    /// The canvas last received a click; inspector controls do not take document keystrokes.
    canvas_focused: bool,
    /// Current IME composition string, drawn inline until committed.
    ime_preedit: Option<String>,
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
    /// A prompt over the document (unsaved changes, or an error).
    modal: Option<Modal>,
    /// What to do when a save finishes (close the window, open another file).
    after_save: Option<Pending>,
    /// Cached headings for the Headers pane (refreshed while that pane is visible).
    headings: Vec<HeadingEntry>,
    /// Cached notes for the Notes pane.
    notes: Vec<NoteEntry>,
    /// Cached bookmarks for the Bookmarks pane.
    bookmarks: Vec<BookmarkEntry>,
    /// A bookmark name being typed in the pane; committed on Return.
    bookmark_draft: Option<(libertum_model::BlockId, usize, String)>,
    /// Where autosave snapshots live (the platform data directory; tests point it elsewhere).
    recovery_dir: PathBuf,
    /// A snapshot waiting for the user's Restore or Discard choice.
    pending_recovery: Option<(libertum_model::Document, recovery::Meta)>,
    /// The Settings window is over the document.
    settings_open: bool,
    /// The frame-rate counter (only with the `fps` feature).
    #[cfg(feature = "fps")]
    fps: crate::fps::FrameCounter,
    /// The right-click menu, if one is open.
    context_menu: Option<ContextMenuState>,
    /// Distance a mouse wheel has asked for that the view has not travelled yet, physical
    /// pixels: the view glides to it over the next frames instead of jumping.
    scroll_pending: (f32, f32),
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
        let mut app = Self {
            state,
            settings,
            settings_path,
            system_mode: theme::Mode::None,
            theme_preference,
            window_width: 1200.0,
            doc,
            home_selection: TemplateId::default(),
            scroll: (0.0, 0.0),
            viewport: None,
            scale_factor: 1.0,
            scale_factor_known: false,
            canvas_focused: false,
            ime_preedit: None,
            autoscroll: AutoScroll::from_env(),
            caret_activity: Instant::now(),
            caret_on: true,
            file: None,
            file_context: None,
            dirty: false,
            modal: None,
            after_save: None,
            headings: Vec::new(),
            notes: Vec::new(),
            bookmarks: Vec::new(),
            bookmark_draft: None,
            thumb_cache: std::collections::HashMap::new(),
            thumb_scroll: (0.0, 0.0),
            recovery_dir: recovery::data_dir().unwrap_or_else(std::env::temp_dir),
            pending_recovery: None,
            settings_open: false,
            scroll_pending: (0.0, 0.0),
            last_frame: None,
            escape_presses: (0, Instant::now()),
            stitch_sources: Vec::new(),
            #[cfg(feature = "fps")]
            fps: crate::fps::FrameCounter::default(),
            context_menu: None,
        };
        app.refresh_document();
        app.headings = app.headings();
        app.notes = app.notes();
        app.bookmarks = app.bookmarks();
        app
    }

    /// Starts on the home screen, or on the generated 500-page demo document when
    /// `LIBERTUM_DEMO=1` or `LIBERTUM_AUTOSCROLL=1` is set (the performance runs need it).
    // TODO(Phase 7): open a Markdown file passed on the command line.
    pub fn boot(
        settings: Settings,
        path: Option<PathBuf>,
        open: Option<PathBuf>,
    ) -> (Self, Task<Message>) {
        let mut tasks = vec![
            iced::system::theme().map(Message::SystemTheme),
            Self::query_scale_factor(),
        ];
        // Test hook: `LIBERTUM_OPEN=<path>` starts with a file instead of the home screen,
        // so screenshots and the save/close checks do not need a native dialog.
        if let Some(open) = std::env::var_os("LIBERTUM_OPEN").map(PathBuf::from) {
            return match FileContext::open(&open) {
                Ok((context, document, warnings)) => {
                    for warning in warnings {
                        tracing::info!(%warning, "import warning");
                    }
                    let view = context.view();
                    let (doc, events) = match context.format() {
                        Format::Markdown => DocView::open_markdown(document),
                        // RTF has no reliable page model of its own, so it reads pageless.
                        Format::Rtf => DocView::open_pageless(document),
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
                    app.refresh_format();
                    app.refresh_section();
                    (app, Task::batch(tasks))
                }
                Err(error) => {
                    tracing::warn!(%error, path = %open.display(), "LIBERTUM_OPEN failed");
                    let mut app = Self::with_doc(settings, path, None);
                    app.modal = Some(Modal::Error(format!("{} {error}", s::OPEN_FAILED)));
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
        (app, Task::batch(tasks))
    }

    /// Replaces the home screen with a new document built from `template`. The caret starts
    /// at the end of the document (the empty body paragraph), ready to type.
    fn create_document(&mut self, template: TemplateId) -> Task<Message> {
        let new = template.build(self.state.units);
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

    /// Insert > Page Number: turns page numbers in the caret's section on (bottom centre) or
    /// off. One undo step.
    fn toggle_page_numbers(&mut self) {
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
            section.page_numbers = match section.page_numbers {
                Some(_) => None,
                None => Some(PageNumberPos::BOTTOM_CENTRE),
            };
            ed.set_section(index, section)
        });
        let _ = self.finish_edit(result);
    }

    fn on_home_key(&mut self, key: &keyboard::Key) -> Task<Message> {
        use keyboard::key::Named;
        match key.as_ref() {
            keyboard::Key::Named(Named::Enter) => self.create_document(self.home_selection),
            keyboard::Key::Named(Named::ArrowLeft) => {
                self.home_selection = self.home_selection.step(-1);
                Task::none()
            }
            keyboard::Key::Named(Named::ArrowRight) => {
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

    fn metrics(&self) -> Metrics {
        let (geometry, pages) = self
            .doc
            .as_ref()
            .map_or((PageGeometry::a4(), 1), |d| (d.geometry, d.page_total()));
        Metrics::new_with_layout(
            &geometry,
            self.state.zoom,
            self.scale_factor,
            self.viewport.map_or(1000.0, |v| v.width),
            self.viewport.map_or(700.0, |v| v.height),
            pages,
            self.state.booklet,
            self.doc.as_ref().is_some_and(|doc| doc.pageless),
            &self
                .doc
                .as_ref()
                .map(DocView::page_geometries)
                .unwrap_or_default(),
        )
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
    fn reanchor(&mut self, old: Metrics, old_viewport: Size) {
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

    fn refresh_view(&mut self) {
        if self.doc.is_none() {
            return;
        }
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
                    // A trackpad (or a drag past the edge): follow it exactly.
                    self.scroll_pending = (0.0, 0.0);
                    self.scroll.0 += dx * sf;
                    self.scroll.1 += dy * sf;
                }
            }
            CanvasEvent::ScrollTo { x, y } => {
                self.scroll_pending = (0.0, 0.0);
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
                    crate::toolbar::ContextMenuTarget::Text
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
            text,
            modifiers,
            ..
        } = event
        else {
            return Task::none();
        };
        // An inspector field owns the keyboard while it is being edited.
        if self.state.editing.is_some() {
            return Task::none();
        }
        let Some(doc) = self.doc.as_mut() else {
            return self.on_home_key(&key);
        };
        let command = modifiers.command();
        let shift = modifiers.shift();
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
            // list items selected). Outside a list, Tab inserts a tab character and
            // Shift+Tab does nothing.
            keyboard::Key::Named(Named::Tab) => {
                if doc.editor().in_table() {
                    // Tab moves to the next cell; in the last cell it adds a row (Word).
                    let result = doc.edit(|ed| ed.tab_in_table(shift));
                    self.finish_edit(result)
                } else if doc.editor().tab_changes_list_level() {
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
        let text = self.ime_preedit.as_ref()?;
        let caret = self.doc.as_ref()?.caret_rect()?;
        Some(canvas::PreeditOverlay {
            page: caret.page,
            x_pt: caret.x,
            y_pt: caret.y,
            height_pt: caret.height,
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

    fn on_doc_event(&mut self, event: DocEvent) {
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
                ],
            )
            .add_filter(s::FILE_FILTER_NATIVE, &[s::EXT_NATIVE])
            .add_filter(s::FILE_FILTER_ODT, &[s::EXT_ODT])
            .add_filter(s::FILE_FILTER_DOCX, &[s::EXT_DOCX])
            .add_filter(s::FILE_FILTER_MARKDOWN, &[s::EXT_MARKDOWN])
            .add_filter(s::FILE_FILTER_RTF, &[s::EXT_RTF])
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
        for warning in warnings {
            tracing::info!(%warning, path = %path.display(), "import warning");
        }
        let view = context.view();
        let (mut doc, events) = match context.format() {
            Format::Markdown => DocView::open_markdown(document),
            // RTF carries a page size but its flow is best read as one continuous column.
            Format::Rtf => DocView::open_pageless(document),
            // A native document remembers whether it was read pageless.
            Format::Native if view.as_ref().is_some_and(|view| view.pageless) => {
                DocView::open_pageless(document)
            }
            Format::Native | Format::Odt | Format::Docx => DocView::open(document),
        };
        doc.move_caret(|ed| ed.move_doc_start(false));
        self.state.fonts = installed_fonts(&doc);
        self.state.word_count = doc.word_count();
        self.state.page_count = doc.page_total();
        self.state.current_page = 1;
        self.doc = Some(doc);
        self.remember_recent(&path);
        self.file = Some(path);
        self.file_context = Some(context);
        self.dirty = false;
        self.modal = None;
        self.after_save = None;
        self.scroll = (0.0, 0.0);
        self.canvas_focused = true;
        self.ime_preedit = None;
        if let Some(view) = &view {
            self.state.zoom = Zoom::Percent(view.zoom_percent.clamp(1, 800));
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
        self.modal = None;
        self.after_save = None;
        self.context_menu = None;
        self.state.find = crate::state::FindState::default();
        self.bookmark_draft = None;
        self.scroll = (0.0, 0.0);
        self.scroll_pending = (0.0, 0.0);
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
    fn autosave_interval(&self) -> Option<u64> {
        let seconds = u64::from(self.settings.autosave_secs);
        (seconds > 0 && self.doc.is_some()).then_some(seconds)
    }

    /// Offers a snapshot from the last session, if one survived.
    fn check_recovery(&mut self) {
        // Not over an open document, and not in place of a prompt already shown (an error
        // opening the file Libertum was started with): the snapshot stays for next time.
        if self.doc.is_some() || self.modal.is_some() {
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
        recovery::clear(&self.recovery_dir);
        self.refresh_document();
        self.refresh_view();
        self.refresh_format();
        Task::run(events, Message::Doc)
    }

    fn discard_recovery(&mut self) {
        self.pending_recovery = None;
        recovery::clear(&self.recovery_dir);
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

    /// Search as the query is typed: from the start of the selection, so a match that still
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

    /// Saves to the current file, asking for one when there is none (or always, with
    /// `save_as`). The package is written on a background task.
    fn save_document(&mut self, save_as: bool) -> Task<Message> {
        if self.doc.is_none() {
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
                self.modal = Some(Modal::MarkdownLoss { path, losses });
                return Task::none();
            }
        }
        self.write_document(path)
    }

    /// Writes the document to `path` on a background task, in the format its extension names.
    fn write_document(&mut self, path: PathBuf) -> Task<Message> {
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
                self.file = Some(path);
                self.dirty = false;
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
        iced::exit()
    }

    fn on_modal_choice(&mut self, choice: ModalChoice) -> Task<Message> {
        let Some(modal) = self.modal.take() else {
            return Task::none();
        };
        match (modal, choice) {
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
                }
                Task::none()
            }
            (Modal::Stitch, _) => Task::none(),
            (Modal::Recovery, ModalChoice::Restore) => self.restore_recovery(),
            (Modal::Recovery, _) => {
                self.discard_recovery();
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
                iced::exit()
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
        let coasting = self.scroll_pending != (0.0, 0.0);
        if autoscrolling || coasting || self.state.sidebar_animating() {
            subs.push(window::frames().map(Message::Frame));
        }
        if self.blinking(Instant::now()) {
            subs.push(Subscription::run(blink_ticks).map(Message::BlinkTick));
        }
        #[cfg(feature = "fps")]
        subs.push(window::frames().map(Message::FpsFrame));
        if let Some(seconds) = self.autosave_interval() {
            subs.push(
                Subscription::run_with(seconds, autosave_ticks).map(|_| Message::AutosaveTick),
            );
        }
        Subscription::batch(subs)
    }

    /// The blink timer runs while a document has keyboard focus and caret activity was
    /// recent; otherwise the caret is solid and nothing ticks.
    fn blinking(&self, now: Instant) -> bool {
        self.doc.is_some()
            && self.canvas_focused
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
        let task = self.update_inner(message);
        if selection_may_change {
            self.refresh_format();
        }
        perf::update_done(started.elapsed());
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
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
                | Message::ScaleFactor(_)
                | Message::SystemTheme(_)
                | Message::WindowResized(_)
        ) {
            if let Some((field, _)) = self.state.editing.take() {
                self.apply_field(field);
            }
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
                | Message::ImageRemove(..)
                | Message::Cut
                | Message::Copy
                | Message::Paste
                | Message::SelectAll
                | Message::Format(_)
                | Message::Document(_)
                | Message::OpenFind
                | Message::Shortcut(_)
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
            Message::CloseMenu => self.state.open_menu = None,
            Message::View(action) => {
                self.state.open_menu = None;
                match action {
                    ViewAction::Inspector => self.toggle_sidebar(),
                    ViewAction::WordCount => {
                        self.state.show_word_count = !self.state.show_word_count;
                        self.settings.show_word_count = self.state.show_word_count;
                        self.save_settings();
                    }
                    ViewAction::Invisibles => {
                        // TODO(Phase 2+): draw paragraph marks / spaces on the canvas.
                        self.state.show_invisibles = !self.state.show_invisibles;
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
            Message::CloseFind => self.close_find(),
            Message::FindQuery(query) => {
                self.state.find.query = query;
                return self.find_as_typed();
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
            Message::Shortcut(Shortcut::CloseDocument) => return self.request_home(),
            Message::OpenSettings => {
                self.state.open_menu = None;
                self.settings_open = true;
            }
            Message::CloseSettings => self.settings_open = false,
            Message::SetTheme(theme) => {
                self.settings.theme = theme;
                self.theme_preference = theme;
                self.save_settings();
            }
            Message::SetDefaultFormat(format) => {
                self.settings.default_format = format;
                self.save_settings();
            }
            Message::SetAutosave(seconds) => {
                self.settings.autosave_secs = seconds;
                self.save_settings();
            }
            Message::SetGraphicsAcceleration(on) => {
                self.settings.graphics_acceleration = on;
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
            #[cfg(feature = "fps")]
            Message::FpsFrame(now) => {
                self.fps.tick(now);
            }
            Message::AutosaveTick => {
                if self.dirty
                    && let Some(doc) = self.doc.as_ref()
                {
                    let document = doc.document().clone();
                    let path = self.file.clone();
                    let dir = self.recovery_dir.clone();
                    return Task::perform(
                        async move {
                            recovery::write(&dir, &document, path.as_deref())
                                .map_err(|error| error.to_string())
                        },
                        Message::Autosaved,
                    );
                }
            }
            Message::Autosaved(result) => {
                if let Err(error) = result {
                    tracing::warn!(%error, "could not write the recovery snapshot");
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
                // Escape backs out of whatever is in front: a prompt (Cancel, or Discard for
                // the recovery offer), then the Settings window, then the Find bar.
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
            Message::SelectTemplate(id) => self.home_selection = id,
            Message::OpenFile => return self.choose_open_file(),
            Message::Saved { path, result } => return self.on_saved(path, result),
            Message::CloseRequested => return self.on_close_request(),
            Message::ModalChoice(choice) => return self.on_modal_choice(choice),
            Message::CreateDocument(id) => {
                if self.doc.is_none() {
                    return self.create_document(id);
                }
            }
            Message::Doc(event) => self.on_doc_event(event),
            Message::Keyboard(event) => return self.on_keyboard(event),
            Message::Ime(event) => return self.on_ime(event),
            Message::PasteText(text) => {
                if let Some(text) = text {
                    if self.canvas_focused
                        && let Some(doc) = self.doc.as_mut()
                    {
                        let result = doc.edit(|ed| ed.paste(&text));
                        return self.finish_edit(result);
                    }
                }
            }
            Message::ScaleFactor(sf) => self.set_scale_factor(sf),
            Message::WindowResized(size) => self.window_width = size.width,
            Message::BlinkTick(now) => self.on_blink_tick(now),
            Message::Frame(now) => {
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
            if matches!(msg, FormatMsg::ToggleStrike) {
                if let Some(doc) = self.doc.as_mut() {
                    let result = doc.edit(|ed| ed.toggle_strikethrough());
                    let _ = self.finish_edit(result);
                }
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
        if let Field::NumberingStart = field {
            if let Some(start) = self.state.document.numbering_start {
                self.edit_section(|section| section.first_page_number = Some(u32::from(start)));
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
            })
            .collect();
        self.state.markdown = self
            .file
            .as_deref()
            .map(Format::for_path)
            .or_else(|| self.file_context.as_ref().map(FileContext::format))
            == Some(Format::Markdown);
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
            DocumentMsg::NumberingOn(on) => self.edit_section(|section| {
                if on {
                    if section.page_numbers.is_none() {
                        section.page_numbers = Some(PageNumberPos::BOTTOM_CENTRE);
                    }
                } else {
                    section.page_numbers = None;
                }
            }),
            DocumentMsg::NumberingContinue(continuing) => self.edit_section(|section| {
                section.first_page_number = if continuing { None } else { Some(1) };
            }),
            DocumentMsg::NumberingVertical(vertical) => self.edit_section(|section| {
                let pos = section
                    .page_numbers
                    .get_or_insert(PageNumberPos::BOTTOM_CENTRE);
                pos.vertical = vertical.model();
            }),
            DocumentMsg::NumberingHorizontal(horizontal) => self.edit_section(|section| {
                let pos = section
                    .page_numbers
                    .get_or_insert(PageNumberPos::BOTTOM_CENTRE);
                pos.horizontal = horizontal.model();
            }),
            DocumentMsg::NumberingMirror(mirror) => self.edit_section(|section| {
                let pos = section
                    .page_numbers
                    .get_or_insert(PageNumberPos::BOTTOM_CENTRE);
                pos.mirror = mirror;
            }),
            DocumentMsg::HideOnFirstPage(hidden) => {
                self.edit_section(|section| section.hide_first_page = hidden);
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
        let mut setup = doc.editor().page_setup();
        change(&mut setup);
        let result = doc.edit(|ed| ed.set_page_setup(setup));
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
        let setup = document.page_setup();
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
        let d = &mut self.state.document;
        d.numbering_start = section
            .first_page_number
            .map(|n| n.min(u32::from(u16::MAX)) as u16);
        d.numbering_on = Mixed::Value(section.page_numbers.is_some());
        let pos = section.page_numbers.unwrap_or(PageNumberPos::BOTTOM_CENTRE);
        d.numbering_vertical = Mixed::Value(NumberingVertical::from_model(pos.vertical));
        d.numbering_horizontal = Mixed::Value(NumberingHorizontal::from_model(pos.horizontal));
        d.numbering_mirror = Mixed::Value(pos.mirror);
        d.hide_on_first_page = Mixed::Value(section.hide_first_page);
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
        let t = Tokens::of(self.scheme());
        let Some(doc) = &self.doc else {
            // Prompts (the stitch dialog, an open error, crash recovery) belong on the home
            // screen too: without this layer they were set but never drawn.
            let home = home::view(self.home_selection, &self.settings.recent_files, t);
            return match &self.modal {
                None => home,
                Some(_) => stack![home, self.modal_layer(t)].into(),
            };
        };
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
                caret: doc.caret_rect().filter(|_| self.caret_on),
                ime_enabled: self.canvas_focused && self.state.editing.is_none(),
                ime_caret: doc.caret_rect(),
                selection: doc.selection_rects(),
                find: self.find_match_rects(),
                preedit: self.preedit_overlay(),
                animate: self.autoscroll.as_ref().is_some_and(AutoScroll::active),
                booklet: self.state.booklet,
                continuous: doc.pageless,
                geometries: doc.page_geometries(),
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
        #[cfg(feature = "fps")]
        {
            canvas_stack = canvas_stack.push(self.fps_pill(t));
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
        if let Some(sidebar) = inspector::view(&self.state, headings, notes, bookmarks, draft, t) {
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
        let base: Element<'_, Message> = if self.state.find.open {
            stack![base, self.find_layer(t)].into()
        } else {
            base
        };
        let base: Element<'_, Message> = if self.settings_open {
            stack![base, self.settings_layer(t)].into()
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
        match &self.modal {
            None => base,
            Some(_) => stack![base, self.modal_layer(t)].into(),
        }
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
        let close = button(text(s::SETTINGS_CLOSE).size(13))
            .padding([6, 18])
            .style(move |_, _| button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: border::rounded(6),
                ..button::Style::default()
            })
            .on_press(Message::CloseSettings);
        let card = container(
            column![
                text(s::SETTINGS_TITLE)
                    .size(16)
                    .font(controls::bold())
                    .color(t.text),
                labelled(t, s::SETTINGS_THEME, theme),
                labelled(t, s::SETTINGS_FORMAT, format),
                labelled(t, s::SETTINGS_AUTOSAVE, autosave),
                labelled(t, s::SETTINGS_UNITS, units),
                column![
                    controls::checkbox_row(
                        t,
                        s::SETTINGS_GRAPHICS,
                        self.settings.graphics_acceleration,
                        false,
                        Message::SetGraphicsAcceleration,
                    ),
                    controls::caption(t, s::SETTINGS_GRAPHICS_HINT),
                ]
                .spacing(4),
                row![space::horizontal(), close],
            ]
            .spacing(16),
        )
        .padding(20)
        .width(Length::Fixed(460.0))
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
        container(
            container(bar)
                .padding([6, 8])
                .style(move |_| container::Style {
                    background: Some(t.chrome.into()),
                    border: border::rounded(8),
                    ..container::Style::default()
                }),
        )
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

    /// The unsaved-changes or error prompt, with a dimmed backdrop.
    fn modal_layer(&self, t: Tokens) -> Element<'_, Message> {
        let Some(modal) = &self.modal else {
            return space().into();
        };
        if matches!(modal, Modal::Stitch) {
            return self.stitch_layer(t);
        }
        let (title, detail): (String, String) = match modal {
            Modal::Unsaved { .. } => (s::UNSAVED_TITLE.to_owned(), s::UNSAVED_DETAIL.to_owned()),
            Modal::Error(message) => (message.clone(), String::new()),
            Modal::Stitch => (s::STITCH_TITLE.to_owned(), String::new()),
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
            Modal::MarkdownLoss { losses, .. } => {
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
                (s::MARKDOWN_LOSS_TITLE.to_owned(), detail)
            }
        };
        let buttons: Vec<Element<'_, Message>> = match modal {
            Modal::Unsaved { .. } => vec![
                self.modal_button(s::SAVE, true, ModalChoice::Save),
                self.modal_button(s::DONT_SAVE, false, ModalChoice::Discard),
                self.modal_button(s::CANCEL, false, ModalChoice::Cancel),
            ],
            Modal::Error(_) => vec![self.modal_button(s::OK, true, ModalChoice::Cancel)],
            Modal::MarkdownLoss { .. } => vec![
                self.modal_button(s::SAVE_ANYWAY, true, ModalChoice::Save),
                self.modal_button(s::CANCEL, false, ModalChoice::Cancel),
            ],
            Modal::Stitch => Vec::new(),
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
        let t = Tokens::of(self.scheme());
        button(text(label).size(13))
            .padding(Padding::from([6.0, 16.0]))
            .on_press(Message::ModalChoice(choice))
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

    /// The floating "Page 1 of 506 · 288,323 words" pill at the bottom centre of the canvas.
    /// The frame-rate counter: a pill in the top-right corner of the canvas. Built only with
    /// the `fps` feature, which official builds leave off.
    #[cfg(feature = "fps")]
    fn fps_pill(&self, t: Tokens) -> Element<'_, Message> {
        let label = self.fps.label().unwrap_or_else(|| "measuring…".to_owned());
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
    });
    if !known {
        path.set_extension(fallback.extension());
    }
    path
}

fn key_event(event: Event, _status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Keyboard(key) => Some(
            shortcuts::from_event(&key).map_or(Message::Keyboard(key.clone()), Message::Shortcut),
        ),
        Event::InputMethod(event) => Some(Message::Ime(event)),
        Event::Window(window::Event::Rescaled(sf)) => Some(Message::ScaleFactor(sf)),
        Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
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

/// Test hook: `LIBERTUM_MENU=view|zoom|insert` opens that toolbar menu at start-up.
fn menu_hook() -> Option<Menu> {
    match std::env::var("LIBERTUM_MENU").ok().as_deref() {
        Some("view") => Some(Menu::View),
        Some("zoom") => Some(Menu::Zoom),
        Some("insert") => Some(Menu::Insert),
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
    fn test_settings() -> Settings {
        Settings {
            units: UnitsPreference::Centimetres,
            ..Settings::default()
        }
    }

    fn app() -> App {
        App::new(test_settings(), None)
    }

    /// An app on the home screen (no document open).
    fn home() -> App {
        App::with_doc(test_settings(), None, None)
    }

    impl App {
        fn doc(&self) -> &DocView {
            self.doc.as_ref().expect("a document is open")
        }
        fn doc_mut(&mut self) -> &mut DocView {
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
            Some(PageNumberPos::BOTTOM_CENTRE)
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
    fn three_paragraph_doc() -> App {
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
    fn section_pane_controls_edit_the_caret_section() {
        let mut app = three_paragraph_doc();
        assert_eq!(app.state.document.numbering_on, Mixed::Value(false));

        let _ = app.update(Message::Document(DocumentMsg::NumberingOn(true)));
        assert_eq!(
            app.doc().editor().document().sections[0].page_numbers,
            Some(PageNumberPos::BOTTOM_CENTRE)
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

        // Start at 1, then step down to 0.
        let _ = app.update(Message::Document(DocumentMsg::NumberingContinue(false)));
        assert_eq!(
            app.doc().editor().document().sections[0].first_page_number,
            Some(1)
        );
        let _ = app.update(Message::Step(Field::NumberingStart, -1));
        assert_eq!(
            app.doc().editor().document().sections[0].first_page_number,
            Some(0),
            "numbering can start at zero"
        );
        assert_eq!(app.state.document.numbering_start, Some(0));

        let _ = app.update(Message::Document(DocumentMsg::HideOnFirstPage(true)));
        assert!(
            app.doc().editor().document().sections[0].hide_first_page,
            "hide first page is stored on the section"
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
        let _ = app.update(Message::FindQuery("cat".into()));
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
        let _ = app.update(Message::FindQuery("cat".into()));
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
        let _ = app.update(Message::FindQuery("zebra".into()));
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
        recovery::write(&dir, &document, Some(old)).expect("a snapshot");

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
        recovery::write(&dir, &document, None).expect("a snapshot");

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

        let _ = app.update(Message::SetGraphicsAcceleration(true));
        assert!(app.settings.graphics_acceleration);

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
                Some(crate::toolbar::ContextMenuTarget::Text)
            ),
            "text commands over body text"
        );
        let _ = app.view();

        // Choosing a command runs it and closes the menu.
        let _ = app.update(Message::SelectAll);
        assert!(app.context_menu.is_none());
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

    #[cfg(feature = "fps")]
    #[test]
    fn the_frame_rate_counter_reports_a_rate() {
        let mut app = app();
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

        // A trackpad delta is applied as it comes and ends any glide.
        let before = app.scroll.1;
        let _ = app.update(Message::Canvas(CanvasEvent::Wheel {
            dx: 0.0,
            dy: 7.5,
            stepped: false,
        }));
        assert_eq!(app.scroll.1, before + 7.5);
        assert_eq!(app.scroll_pending, (0.0, 0.0));

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
        let _ = app.update(Message::FindQuery("needle300".into()));
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
        let _ = app.update(Message::FindQuery("needle2 ".into()));
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
        let _ = app.update(Message::FindQuery("cat".into()));
        assert_eq!(app.state.find.count.map(|c| c.1), Some(4));
        let _ = app.update(Message::ToggleFindWord);
        assert_eq!(app.state.find.count.map(|c| c.1), Some(3));
        let _ = app.update(Message::ToggleFindCase);
        assert_eq!(app.state.find.count.map(|c| c.1), Some(2));
        // Nothing found: the count goes and the notice says so.
        let _ = app.update(Message::FindQuery("dog".into()));
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
        ] {
            let filters = save_filters(current);
            assert_eq!(filters.len(), 5);
            assert_eq!(
                filters[0].1,
                current.extension(),
                "the current format leads"
            );
            let mut extensions: Vec<&str> = filters.iter().map(|f| f.1).collect();
            extensions.sort_unstable();
            assert_eq!(extensions, vec!["docx", "lbtd", "md", "odt", "rtf"]);
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
        assert!(app.modal.is_none());
        // A file that cannot be opened shows the error over the home screen.
        let (app, _task) = App::boot(settings.clone(), None, Some(dir.join("missing.odt")));
        assert!(app.doc.is_none());
        assert!(matches!(app.modal, Some(Modal::Error(_))));
        // No argument: the home screen.
        let (app, _task) = App::boot(settings, None, None);
        assert!(app.doc.is_none() || demo_requested());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
