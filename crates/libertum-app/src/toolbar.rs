//! The Pages-style toolbar and its dropdown menus.
//!
//! The toolbar is a fixed-height strip of flat icon buttons with a small caption underneath.
//! On macOS it is merged into the (transparent, full-size) title bar, so the left edge leaves
//! room for the traffic-light buttons and empty space drags the window. Dropdowns are drawn
//! by the app as a layer above everything else; [`menu_x`] tells it where to anchor them from
//! the same fixed widths the toolbar lays out with.

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, stack, text, text_input,
};
use iced::{Border, Center, Color, Element, Length, Padding, Shadow, Vector};

use crate::app::{ExportFormat, Message};
use crate::icons::{Icon, icon};
use crate::shortcuts::{self, Shortcut};
use crate::state::{AppState, InsertAction, Inspector, Menu, TableAction, ViewAction, Zoom};
use crate::strings as s;
use crate::theme::Tokens;

pub const HEIGHT: f32 = 52.0;
/// Room for the macOS traffic-light buttons (logical pixels).
#[cfg(target_os = "macos")]
pub const LEFT_INSET: f32 = 78.0;
#[cfg(not(target_os = "macos"))]
pub const LEFT_INSET: f32 = 12.0;
const RIGHT_INSET: f32 = 12.0;
const BUTTON_SPACING: f32 = 2.0;
const GLYPH_SIZE: f32 = 18.0;

const W_VIEW: f32 = 58.0;
/// The Zoom control is three toolbar buttons side by side: minus, the percentage with its
/// menu, and plus. All three are as tall as every other button and share one style.
const ZOOM_STEP_W: f32 = 22.0;
const ZOOM_MAIN_W: f32 = 56.0;
const ZOOM_NUMBER_W: f32 = 28.0;
/// Space left of the figure; the room on the right belongs to the chevron.
const ZOOM_FIGURE_INSET: f32 = 6.0;
const ZOOM_STEP_GLYPH: f32 = 12.0;
const W_ZOOM: f32 = ZOOM_STEP_W * 2.0 + ZOOM_MAIN_W + BUTTON_SPACING * 2.0;
const W_FULL_SCREEN: f32 = 68.0;
const CENTRE_WIDTHS: [f32; 4] = [58.0, 52.0, 52.0, 64.0];
/// The four inspector buttons (Format, Document, Headers, Notes) share one width and one gap,
/// so the group reads as an even strip whatever the active pane is or how long a caption is.
const W_INSPECTOR: f32 = 76.0;
const RIGHT_WIDTHS: [f32; 4] = [W_INSPECTOR; 4];

fn group_width(widths: &[f32]) -> f32 {
    widths.iter().sum::<f32>() + BUTTON_SPACING * widths.len().saturating_sub(1) as f32
}

/// Width of the left group: View, Full Screen and the combined Zoom control.
fn left_width() -> f32 {
    group_width(&[W_VIEW, W_FULL_SCREEN, W_ZOOM])
}

/// Left edge of the View button, in window coordinates.
fn view_x() -> f32 {
    LEFT_INSET
}

/// Left edge of the Full Screen button, which follows View.
fn full_screen_x() -> f32 {
    view_x() + W_VIEW + BUTTON_SPACING
}

/// Left edge of the Zoom button, which follows Full Screen.
fn zoom_x() -> f32 {
    full_screen_x() + W_FULL_SCREEN + BUTTON_SPACING
}

/// Left edge of the Insert button, which sits at the start of the centred group.
fn centre_x(window_width: f32) -> f32 {
    let free = window_width
        - LEFT_INSET
        - RIGHT_INSET
        - left_width()
        - group_width(&CENTRE_WIDTHS)
        - group_width(&RIGHT_WIDTHS);
    LEFT_INSET + left_width() + free.max(0.0) / 2.0
}

/// Left edge of a menu's toolbar button, in window coordinates.
pub fn menu_x(menu: Menu, window_width: f32) -> f32 {
    match menu {
        Menu::View => view_x(),
        // Under the middle button, the one that opens it.
        Menu::Zoom => zoom_x() + ZOOM_STEP_W + BUTTON_SPACING,
        Menu::Insert => centre_x(window_width),
        Menu::Export => {
            centre_x(window_width)
                + CENTRE_WIDTHS[0]
                + BUTTON_SPACING
                + CENTRE_WIDTHS[1]
                + BUTTON_SPACING
                + CENTRE_WIDTHS[2]
                + BUTTON_SPACING
        }
    }
}

/// The Zoom control: a minus button, the percentage with its dropdown, and a plus button, all as
/// tall as every other toolbar button and sharing their style. The middle one has no caption:
/// the figure fills the height, close to the left edge, and the chevron sits in its top-right corner. The glyphs of the
/// steppers are centred in the height. The percentage stays editable for an exact value.
fn zoom_control<'a>(state: &AppState, percent: u16, t: Tokens) -> Element<'a, Message> {
    let open = state.open_menu == Some(Menu::Zoom);
    let colour = if open { t.accent } else { t.text };
    let label_colour = if open { t.accent } else { t.text_secondary };
    let shown = state
        .zoom_edit
        .clone()
        .unwrap_or_else(|| percent.to_string());
    let input = text_input("", &shown)
        .on_input(Message::ZoomText)
        .on_submit(Message::ZoomCommit)
        .size(15.0)
        .padding(0)
        .width(Length::Fixed(ZOOM_NUMBER_W))
        .align_x(Center)
        .style(move |_, _| text_input::Style {
            background: Color::TRANSPARENT.into(),
            border: Border::default(),
            icon: t.text_secondary,
            placeholder: t.text_secondary,
            value: colour,
            selection: Color {
                a: 0.35,
                ..t.accent
            },
        });
    let figure = container(
        row![input, text("%").size(12.0).color(label_colour)]
            .spacing(1)
            .align_y(Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(iced::alignment::Vertical::Center)
    .padding(Padding {
        left: ZOOM_FIGURE_INSET,
        ..Padding::ZERO
    });
    let corner = container(icon(Icon::ChevronDown, 9.0, label_colour))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Top)
        .padding(Padding {
            top: 5.0,
            right: 5.0,
            bottom: 0.0,
            left: 0.0,
        });
    let main = zoom_button(
        stack![figure, corner].into(),
        ZOOM_MAIN_W,
        open,
        t,
        Message::ToggleMenu(Menu::Zoom),
    );
    row![
        zoom_step(t, Icon::Minus, -1),
        main,
        zoom_step(t, Icon::Plus, 1)
    ]
    .spacing(BUTTON_SPACING)
    .align_y(Center)
    .into()
}

/// A stepper: a small glyph centred in the button.
fn zoom_step<'a>(t: Tokens, glyph: Icon, direction: i8) -> Element<'a, Message> {
    zoom_button(
        icon(glyph, ZOOM_STEP_GLYPH, t.text).into(),
        ZOOM_STEP_W,
        false,
        t,
        Message::ZoomStep(i16::from(direction)),
    )
}

fn zoom_button<'a>(
    content: Element<'a, Message>,
    width: f32,
    active: bool,
    t: Tokens,
    on_press: Message,
) -> Element<'a, Message> {
    button(
        container(content)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(width))
    .height(Length::Fixed(HEIGHT - 8.0))
    .padding(0)
    .style(move |_, status| tool_style(t, active, status))
    .on_press(on_press)
    .into()
}

/// The look of every toolbar button: clear at rest, tinted while its menu is open, shaded on
/// hover and press.
fn tool_style(t: Tokens, active: bool, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Pressed => Some(t.pressed.into()),
        button::Status::Hovered => Some(t.hover.into()),
        _ if active => Some(
            Color {
                a: 0.12,
                ..t.accent
            }
            .into(),
        ),
        _ => None,
    };
    button::Style {
        background,
        text_color: t.text,
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

enum Top {
    Icon(Icon),
}

struct Btn {
    top: Top,
    label: &'static str,
    width: f32,
    chevron: bool,
    active: bool,
    on_press: Option<Message>,
}

impl Btn {
    fn new(top: Top, label: &'static str, width: f32) -> Self {
        Self {
            top,
            label,
            width,
            chevron: false,
            active: false,
            on_press: None,
        }
    }

    fn icon(glyph: Icon, label: &'static str, width: f32) -> Self {
        Self::new(Top::Icon(glyph), label, width)
    }

    fn chevron(mut self) -> Self {
        self.chevron = true;
        self
    }

    fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    fn on_press(mut self, message: Message) -> Self {
        self.on_press = Some(message);
        self
    }

    fn view<'a>(self, t: Tokens) -> Element<'a, Message> {
        let enabled = self.on_press.is_some();
        let colour = if !enabled {
            t.text_disabled
        } else if self.active {
            t.accent
        } else {
            t.text
        };
        let label_colour = if !enabled {
            t.text_disabled
        } else if self.active {
            t.accent
        } else {
            t.text_secondary
        };
        let mut top = row![].spacing(2).align_y(Center);
        top = match self.top {
            Top::Icon(glyph) => top.push(icon(glyph, GLYPH_SIZE, colour)),
        };
        if self.chevron {
            top = top.push(icon(Icon::ChevronDown, 9.0, label_colour));
        }
        let content = column![
            container(top)
                .height(Length::Fixed(GLYPH_SIZE + 2.0))
                .center_y(Length::Fill),
            text(self.label).size(11).color(label_colour),
        ]
        .spacing(3)
        .align_x(Center);

        let active = self.active;
        let mut b = button(
            container(content)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fixed(self.width))
        .height(Length::Fixed(HEIGHT - 8.0))
        .padding(0)
        .style(move |_, status| tool_style(t, active, status));
        if let Some(message) = self.on_press {
            b = b.on_press(message);
        }
        b.into()
    }
}

pub fn view<'a>(state: &'a AppState, zoom_percent: u16, t: Tokens) -> Element<'a, Message> {
    let sidebar_on = state.sidebar_visible;
    let format_on = sidebar_on && state.inspector == Inspector::Format;
    let document_on = sidebar_on && state.inspector == Inspector::Document;
    let headers_on = sidebar_on && state.inspector == Inspector::Headers;
    let notes_on = sidebar_on && state.inspector == Inspector::Notes;

    let left = row![
        Btn::icon(Icon::PanelRight, s::TB_VIEW, W_VIEW)
            .chevron()
            .active(state.open_menu == Some(Menu::View))
            .on_press(Message::ToggleMenu(Menu::View))
            .view(t),
        // Page Break lives in the Insert menu and on Ctrl/Cmd+Enter.
        Btn::icon(
            if state.fullscreen {
                Icon::Minimize
            } else {
                Icon::Maximize
            },
            s::TB_FULL_SCREEN,
            W_FULL_SCREEN,
        )
        .active(state.fullscreen)
        .on_press(Message::ToggleFullScreen)
        .view(t),
        zoom_control(state, zoom_percent, t),
    ]
    .spacing(BUTTON_SPACING)
    .align_y(Center);

    let centre = row([
        Btn::icon(Icon::CirclePlus, s::TB_INSERT, CENTRE_WIDTHS[0])
            .chevron()
            .active(state.open_menu == Some(Menu::Insert))
            .on_press(Message::ToggleMenu(Menu::Insert)),
        Btn::icon(Icon::Image, s::TB_IMAGE, CENTRE_WIDTHS[1])
            .on_press(Message::Insert(InsertAction::Image)),
        Btn::icon(Icon::Asterisk, s::TB_FOOTNOTE, CENTRE_WIDTHS[2])
            .on_press(Message::Insert(InsertAction::Footnote)),
        Btn::icon(Icon::Upload, s::TB_EXPORT, CENTRE_WIDTHS[3])
            .chevron()
            .active(state.open_menu == Some(Menu::Export))
            .on_press(Message::ToggleMenu(Menu::Export)),
    ]
    .into_iter()
    .map(|b: Btn| b.view(t)))
    .spacing(BUTTON_SPACING);

    let right = row([
        Btn::icon(Icon::Paintbrush, s::TB_FORMAT, RIGHT_WIDTHS[0])
            .active(format_on)
            .on_press(Message::ToggleInspector(Inspector::Format)),
        Btn::icon(Icon::Document, s::TB_DOCUMENT, RIGHT_WIDTHS[1])
            .active(document_on)
            .on_press(Message::ToggleInspector(Inspector::Document)),
        Btn::icon(Icon::List, s::TB_HEADERS, RIGHT_WIDTHS[2])
            .active(headers_on)
            .on_press(Message::ToggleInspector(Inspector::Headers)),
        Btn::icon(Icon::Asterisk, s::TB_NOTES, RIGHT_WIDTHS[3])
            .active(notes_on)
            .on_press(Message::ToggleInspector(Inspector::Notes)),
    ]
    .into_iter()
    .map(|b: Btn| b.view(t)))
    .spacing(BUTTON_SPACING);

    let bar = container(
        row![
            left,
            space::horizontal(),
            centre,
            space::horizontal(),
            right,
        ]
        .align_y(Center),
    )
    .padding(Padding {
        top: 0.0,
        bottom: 0.0,
        left: LEFT_INSET,
        right: RIGHT_INSET,
    })
    .width(Length::Fill)
    .height(Length::Fixed(HEIGHT))
    .center_y(Length::Fixed(HEIGHT))
    .style(move |_| container::Style {
        background: Some(t.chrome.into()),
        border: Border {
            color: t.chrome_border,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..container::Style::default()
    });

    let bar: Element<'a, Message> = bar.into();
    // With the title bar merged into the toolbar, empty toolbar space is the drag handle.
    #[cfg(target_os = "macos")]
    let bar: Element<'a, Message> = iced::widget::mouse_area(bar)
        .on_press(Message::DragWindow)
        .on_double_click(Message::ToggleMaximise)
        .into();

    column![bar, crate::widgets::controls::hairline(t.chrome_border)].into()
}

// ------------------------------------------------------------------------- menus

enum Entry {
    Item {
        label: String,
        shortcut: Option<&'static str>,
        checked: bool,
        /// The row opens a submenu beside the menu, so it ends in an arrow.
        submenu: bool,
        /// That submenu is showing: the row stays highlighted until it goes.
        open: bool,
        /// Sent when the pointer moves onto the row, without a click.
        hover: Option<Box<Message>>,
        on_press: Option<Message>,
    },
    Separator,
}

impl Entry {
    fn item(label: impl Into<String>, on_press: Message) -> Self {
        Self::Item {
            label: label.into(),
            shortcut: None,
            checked: false,
            submenu: false,
            open: false,
            hover: None,
            on_press: Some(on_press),
        }
    }

    /// A row that opens a submenu beside the menu; `open` says whether it is showing now.
    fn submenu(label: impl Into<String>, on_press: Message, open: bool) -> Self {
        Self::Item {
            label: label.into(),
            shortcut: None,
            checked: false,
            submenu: true,
            open,
            hover: None,
            on_press: Some(on_press),
        }
    }

    fn disabled(label: impl Into<String>) -> Self {
        Self::Item {
            label: label.into(),
            shortcut: None,
            checked: false,
            submenu: false,
            open: false,
            hover: None,
            on_press: None,
        }
    }

    fn checked(mut self, value: bool) -> Self {
        if let Self::Item { checked, .. } = &mut self {
            *checked = value;
        }
        self
    }

    fn shortcut(mut self, value: &'static str) -> Self {
        if let Self::Item { shortcut, .. } = &mut self {
            *shortcut = Some(value);
        }
        self
    }
}

fn entries(menu: Menu, state: &AppState) -> Vec<Entry> {
    match menu {
        Menu::View => vec![
            Entry::item(
                if state.sidebar_visible {
                    s::VIEW_HIDE_INSPECTOR
                } else {
                    s::VIEW_SHOW_INSPECTOR
                },
                Message::View(ViewAction::Inspector),
            )
            .shortcut(shortcuts::label(Shortcut::ToggleSidebar)),
            Entry::item(s::VIEW_FIND, Message::OpenFind).shortcut(shortcuts::label(Shortcut::Find)),
            Entry::item(s::VIEW_SETTINGS, Message::OpenSettings)
                .shortcut(shortcuts::label(Shortcut::Settings)),
            Entry::item(s::VIEW_NEW, Message::GoHome)
                .shortcut(shortcuts::label(Shortcut::NewDocument)),
            Entry::item(s::VIEW_HOME, Message::GoHome)
                .shortcut(shortcuts::label(Shortcut::CloseDocument)),
            Entry::item(s::VIEW_STITCH, Message::OpenStitch),
            Entry::item(s::VIEW_LIBRARY, Message::OpenLibrary),
            Entry::item(s::VIEW_HISTORY, Message::OpenHistory(None)),
            Entry::item(
                if state.fullscreen {
                    s::VIEW_EXIT_FULL_SCREEN
                } else {
                    s::VIEW_ENTER_FULL_SCREEN
                },
                Message::ToggleFullScreen,
            )
            .shortcut(shortcuts::label(Shortcut::FullScreen)),
            Entry::item(s::VIEW_WORD_COUNT, Message::View(ViewAction::WordCount))
                .checked(state.show_word_count),
            Entry::Separator,
            Entry::item(s::VIEW_BOOKLET, Message::View(ViewAction::Booklet)).checked(state.booklet),
            Entry::item(s::VIEW_PAGELESS, Message::View(ViewAction::Pageless))
                .checked(state.pageless),
            Entry::item(
                s::VIEW_COMPARE,
                Message::Compare(crate::app::CompareMsg::Toggle),
            )
            .checked(state.compare),
            Entry::item(
                s::VIEW_BOOKMARKS,
                Message::ToggleInspector(Inspector::Bookmarks),
            )
            .checked(state.sidebar_visible && state.inspector == Inspector::Bookmarks),
            // Thumbnails show pages, so they are off in the pageless view.
            if state.pageless {
                Entry::disabled(s::VIEW_THUMBNAILS)
            } else {
                Entry::item(s::VIEW_THUMBNAILS, Message::ToggleThumbnails).checked(state.thumbnails)
            },
        ],
        Menu::Zoom => {
            let mut list: Vec<Entry> = vec![
                Entry::item(s::ZOOM_IN, Message::ZoomStep(10)),
                Entry::item(s::ZOOM_OUT, Message::ZoomStep(-10)),
                Entry::item(s::ZOOM_ACTUAL, Message::ZoomSet(100)),
                Entry::Separator,
            ];
            list.extend(Zoom::PRESETS.iter().map(|&p| {
                Entry::item(s::zoom_percent(p), Message::ZoomSelected(Zoom::Percent(p)))
                    .checked(state.zoom == Zoom::Percent(p))
            }));
            list.push(Entry::Separator);
            list.push(
                Entry::item(s::ZOOM_FIT_WIDTH, Message::ZoomSelected(Zoom::FitWidth))
                    .checked(state.zoom == Zoom::FitWidth),
            );
            list.push(
                Entry::item(s::ZOOM_FIT_PAGE, Message::ZoomSelected(Zoom::FitPage))
                    .checked(state.zoom == Zoom::FitPage),
            );
            list
        }
        Menu::Export => {
            let mut list = vec![
                Entry::item(s::EXPORT_PDF, Message::Export(ExportFormat::Pdf)),
                Entry::item(
                    s::EXPORT_PDF_ARCHIVAL,
                    Message::Export(ExportFormat::PdfArchive),
                ),
                Entry::item(s::EXPORT_EPUB, Message::Export(ExportFormat::Epub)),
                Entry::item(s::EXPORT_RTF, Message::Export(ExportFormat::Rtf)),
                Entry::item(s::EXPORT_TEXT, Message::Export(ExportFormat::Text)),
                Entry::item(s::EXPORT_MARKDOWN, Message::Export(ExportFormat::Markdown)),
                Entry::item(
                    s::EXPORT_PLAIN_HTML,
                    Message::Export(ExportFormat::PlainHtml),
                ),
                Entry::Separator,
                Entry::submenu(s::EXPORT_AO3, Message::OpenAo3Chapters, state.ao3_open),
            ];
            // Resting the pointer on the AO3 row shows its chapters, with no click; moving to
            // another row puts them away again.
            for entry in &mut list {
                if let Entry::Item { submenu, hover, .. } = entry {
                    *hover = if *submenu {
                        Some(Box::new(Message::OpenAo3Chapters))
                    } else if state.ao3_open {
                        Some(Box::new(Message::CloseAo3List))
                    } else {
                        None
                    };
                }
            }
            list
        }
        Menu::Insert => {
            let item = |a: InsertAction| Entry::item(a.to_string(), Message::Insert(a));
            let mut list = vec![
                item(InsertAction::PageBreak),
                item(InsertAction::SectionBreak),
                Entry::Separator,
                item(InsertAction::Table),
                item(InsertAction::Image),
                Entry::item(s::INSERT_LINKED_MEDIA, Message::OpenLinkedMedia),
                Entry::item(s::INSERT_MUSIC, Message::MusicAdd),
                Entry::Separator,
                item(InsertAction::Footnote),
                item(InsertAction::Endnote),
                item(InsertAction::TableOfContents),
                item(InsertAction::PageNumber),
                Entry::Separator,
                Entry::item(s::INSERT_DATE_TIME, Message::OpenDateTime),
                Entry::Separator,
                Entry::item(s::INSERT_CHAPTER_FILE, Message::ImportChapterFile),
                Entry::item(s::INSERT_CHAPTER_HTML, Message::ImportChapterClipboard),
            ];
            // The Table button is gone from the toolbar; the caret's table commands live
            // here while the caret is in one.
            if state.in_table {
                list.push(Entry::Separator);
                list.extend(table_entries(state));
            }
            list
        }
    }
}

/// The Table menu's caret commands, shown at the end of the Insert menu while the caret
/// is inside a table (the toolbar Table button was replaced by Export).
fn table_entries(state: &AppState) -> Vec<Entry> {
    let item = |action: TableAction| Entry::item(action.to_string(), Message::Table(action));
    let when = |possible: bool, action: TableAction| {
        if possible {
            Entry::item(action.to_string(), Message::Table(action))
        } else {
            Entry::disabled(action.to_string())
        }
    };
    let mut list = vec![
        item(TableAction::InsertRowAbove),
        item(TableAction::InsertRowBelow),
        Entry::Separator,
        item(TableAction::InsertColumnLeft),
        item(TableAction::InsertColumnRight),
        Entry::Separator,
        when(state.can_merge_right, TableAction::MergeRight),
        when(state.can_merge_below, TableAction::MergeBelow),
        when(state.can_split_cell, TableAction::SplitCell),
        Entry::Separator,
        item(TableAction::DeleteRow),
        item(TableAction::DeleteColumn),
        item(TableAction::DeleteTable),
        Entry::Separator,
    ];
    list.push(
        Entry::item(s::TABLE_STYLE_NONE, Message::TableStyle(None))
            .checked(state.table_style.is_none()),
    );
    for (index, (style, _)) in libertum_model::TableStyle::builtins()
        .into_iter()
        .enumerate()
    {
        let current = state
            .table_style
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(&style.name));
        list.push(Entry::item(style.name, Message::TableStyle(Some(index))).checked(current));
    }
    list
}

const MENU_WIDTH: f32 = 232.0;
/// A menu taller than this scrolls (the Insert menu grows when the caret is in a table).
const MENU_MAX_HEIGHT: f32 = 420.0;

/// The open dropdown, positioned under its toolbar button. Fills the window so the caller can
/// stack it directly.
pub fn menu_layer<'a>(
    menu: Menu,
    state: &AppState,
    window_width: f32,
    t: Tokens,
) -> Element<'a, Message> {
    let panel = panel(entries(menu, state), t);
    let x = menu_x(menu, window_width).min((window_width - MENU_WIDTH - 8.0).max(0.0));
    container(panel)
        .padding(Padding {
            top: HEIGHT - 4.0,
            left: x,
            right: 0.0,
            bottom: 0.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Width of the chapter list beside the Export menu.
const AO3_WIDTH: f32 = 260.0;

/// The AO3 chapter list, shown beside the Export menu. Each row opens that chapter's HTML.
pub fn ao3_layer<'a>(state: &AppState, window_width: f32, t: Tokens) -> Element<'a, Message> {
    let x = (menu_x(Menu::Export, window_width) + MENU_WIDTH + 8.0)
        .min((window_width - AO3_WIDTH - 8.0).max(0.0));
    let rows: Vec<Element<'a, Message>> = if state.ao3_chapters.is_empty() {
        vec![
            container(text(s::AO3_EMPTY).size(12).color(t.text_secondary))
                .padding(6)
                .into(),
        ]
    } else {
        state
            .ao3_chapters
            .iter()
            .enumerate()
            .map(|(index, chapter)| {
                button(text(chapter.title.clone()).size(12))
                    .width(Length::Fill)
                    .padding([5, 8])
                    .style(move |_, status| {
                        let hot =
                            matches!(status, button::Status::Hovered | button::Status::Pressed);
                        button::Style {
                            background: hot.then(|| t.menu_hover.into()),
                            text_color: if hot { t.on_accent } else { t.text },
                            border: Border {
                                radius: 4.0.into(),
                                ..Border::default()
                            },
                            ..button::Style::default()
                        }
                    })
                    .on_press(Message::Ao3Chapter(index))
                    .into()
            })
            .collect()
    };
    let list = scrollable(column(rows).spacing(2).padding(2)).height(Length::Fixed(320.0));
    let card = container(
        column![
            text(s::AO3_TITLE).size(13).color(t.text),
            text(s::AO3_HINT).size(11).color(t.text_secondary),
            list,
        ]
        .spacing(6),
    )
    .padding(8)
    .width(Length::Fixed(AO3_WIDTH))
    .style(move |_| container::Style {
        background: Some(t.menu.into()),
        border: Border {
            color: t.menu_border,
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: Shadow {
            color: t.shadow,
            offset: Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    });
    container(card)
        .padding(Padding {
            top: HEIGHT - 4.0,
            left: x,
            right: 0.0,
            bottom: 0.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A misspelt word under the pointer and the suggestions offered for it.
#[derive(Clone, Debug, PartialEq)]
pub struct SpellingMenu {
    pub span: crate::spell::Span,
    pub suggestions: Vec<String>,
}

/// What a right-click menu offers: the thing under the pointer decides.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextMenuTarget {
    Text {
        spelling: Option<SpellingMenu>,
    },
    Table {
        merge_right: bool,
        merge_below: bool,
        split: bool,
    },
    Image {
        block: libertum_model::BlockId,
        index: usize,
    },
    /// A story checklist point in the Notes pane.
    ChecklistPoint {
        id: u64,
    },
}

/// The right-click menu, anchored at `x`/`y` in window logical pixels.
pub fn context_layer<'a>(
    target: &ContextMenuTarget,
    x: f32,
    y: f32,
    t: Tokens,
) -> Element<'a, Message> {
    let panel = match target {
        // One command: a menu just wide enough for its word, not the full-width panel.
        ContextMenuTarget::ChecklistPoint { id } => compact_panel(
            vec![
                (s::CHECKLIST_EDIT, Message::ChecklistEditOpen(*id)),
                (s::CHECKLIST_REMOVE, Message::ChecklistRemove(*id)),
            ],
            t,
        ),
        _ => panel(context_entries(target), t),
    };
    container(panel)
        .padding(Padding {
            top: y.max(8.0),
            left: x.max(8.0),
            right: 0.0,
            bottom: 0.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The commands for one right-click target. Each is also on the menu bar, in an inspector
/// pane, or on a keyboard shortcut: no action lives only here.
fn context_entries(target: &ContextMenuTarget) -> Vec<Entry> {
    let cmd = shortcuts::command_key_label;
    match target {
        ContextMenuTarget::Text { spelling } => {
            let mut entries = Vec::new();
            if let Some(menu) = spelling {
                entries.push(Entry::disabled(s::SPELL_SUGGESTIONS));
                for suggestion in &menu.suggestions {
                    entries.push(Entry::item(
                        suggestion.clone(),
                        Message::SpellReplace {
                            span: menu.span.clone(),
                            replacement: suggestion.clone(),
                        },
                    ));
                }
                if menu.suggestions.is_empty() {
                    entries.push(Entry::disabled(s::SPELL_NO_SUGGESTIONS));
                }
                entries.push(Entry::Separator);
                entries.push(Entry::item(
                    s::SPELL_ADD_DICTIONARY,
                    Message::SpellAddWord(menu.span.word.clone()),
                ));
                entries.push(Entry::item(
                    s::SPELL_IGNORE_ONCE,
                    Message::SpellIgnoreOnce(menu.span.clone()),
                ));
                entries.push(Entry::item(
                    s::SPELL_IGNORE_DOCUMENT,
                    Message::SpellIgnoreDocument(menu.span.word.clone()),
                ));
                entries.push(Entry::Separator);
            }
            entries.extend([
                Entry::item(s::CONTEXT_CUT, Message::Cut).shortcut(cmd("X")),
                Entry::item(s::CONTEXT_COPY, Message::Copy).shortcut(cmd("C")),
                Entry::item(s::CONTEXT_PASTE, Message::Paste).shortcut(cmd("V")),
                Entry::Separator,
                Entry::item(s::CONTEXT_SELECT_ALL, Message::SelectAll).shortcut(cmd("A")),
                Entry::Separator,
                Entry::item(s::CONTEXT_BOLD, Message::Shortcut(Shortcut::Bold))
                    .shortcut(shortcuts::label(Shortcut::Bold)),
                Entry::item(s::CONTEXT_ITALIC, Message::Shortcut(Shortcut::Italic))
                    .shortcut(shortcuts::label(Shortcut::Italic)),
                Entry::item(s::CONTEXT_UNDERLINE, Message::Shortcut(Shortcut::Underline))
                    .shortcut(shortcuts::label(Shortcut::Underline)),
                Entry::Separator,
                Entry::item(s::VIEW_FIND, Message::OpenFind)
                    .shortcut(shortcuts::label(Shortcut::Find)),
            ]);
            entries
        }
        ContextMenuTarget::Table {
            merge_right,
            merge_below,
            split,
        } => {
            let when = |possible: bool, action: TableAction| {
                if possible {
                    Entry::item(action.to_string(), Message::Table(action))
                } else {
                    Entry::disabled(action.to_string())
                }
            };
            vec![
                Entry::item(
                    s::TABLE_ROW_ABOVE,
                    Message::Table(TableAction::InsertRowAbove),
                ),
                Entry::item(
                    s::TABLE_ROW_BELOW,
                    Message::Table(TableAction::InsertRowBelow),
                ),
                Entry::item(
                    s::TABLE_COLUMN_LEFT,
                    Message::Table(TableAction::InsertColumnLeft),
                ),
                Entry::item(
                    s::TABLE_COLUMN_RIGHT,
                    Message::Table(TableAction::InsertColumnRight),
                ),
                Entry::Separator,
                when(*merge_right, TableAction::MergeRight),
                when(*merge_below, TableAction::MergeBelow),
                when(*split, TableAction::SplitCell),
                Entry::Separator,
                Entry::item(s::TABLE_DELETE_ROW, Message::Table(TableAction::DeleteRow)),
                Entry::item(
                    s::TABLE_DELETE_COLUMN,
                    Message::Table(TableAction::DeleteColumn),
                ),
                Entry::item(s::TABLE_DELETE, Message::Table(TableAction::DeleteTable)),
            ]
        }
        ContextMenuTarget::Image { block, index } => vec![
            Entry::item(s::IMAGE_LARGER, Message::ImageStep(*block, *index, 1)),
            Entry::item(s::IMAGE_SMALLER, Message::ImageStep(*block, *index, -1)),
            Entry::Separator,
            Entry::item(s::IMAGE_REMOVE, Message::ImageRemove(*block, *index)),
        ],
        ContextMenuTarget::ChecklistPoint { id } => vec![
            Entry::item(s::CHECKLIST_EDIT, Message::ChecklistEditOpen(*id)),
            Entry::item(s::CHECKLIST_REMOVE, Message::ChecklistRemove(*id)),
        ],
    }
}

/// An upper bound on the width of the one-command menu [`compact_panel`] draws, so the app can
/// keep it on screen near the window's right edge.
pub const CONTEXT_COMPACT_WIDTH: f32 = 96.0;

/// A short menu of a few commands, each row as wide as the longest word plus a little
/// padding rather than the full-width panel the menu bar uses.
fn compact_panel<'a>(entries: Vec<(&'static str, Message)>, t: Tokens) -> Element<'a, Message> {
    // About 7.5 logical pixels a letter at 13 px, plus the row's padding.
    let widest = entries
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0) as f32;
    let width = widest * 7.5 + 24.0;
    let rows = entries.into_iter().map(|(label, message)| {
        button(text(label).size(13))
            .width(Length::Fixed(width))
            .padding([4, 12])
            .style(move |_, status| {
                let hot = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: hot.then(|| t.menu_hover.into()),
                    text_color: if hot { t.on_accent } else { t.text },
                    border: Border {
                        radius: 4.0.into(),
                        ..Border::default()
                    },
                    ..button::Style::default()
                }
            })
            .on_press(message)
            .into()
    });
    container(column(rows.collect::<Vec<Element<'a, Message>>>()))
        .padding(3)
        .style(move |_| container::Style {
            background: Some(t.menu.into()),
            border: Border {
                color: t.menu_border,
                width: 1.0,
                radius: 8.0.into(),
            },
            shadow: Shadow {
                color: t.shadow,
                offset: Vector::new(0.0, 6.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
        .into()
}

/// Roughly how tall the menu's contents are. A row is about 24 logical pixels (13 px text
/// with 3 px of vertical padding) and a separator about 9. This only decides whether the
/// list scrolls; counting every row as 30 px pushed the View menu over the limit, so it
/// reserved 420 px and left a quarter of the panel empty below the last item.
fn natural_height(entries: &[Entry]) -> f32 {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Separator => 9.0,
            Entry::Item { .. } => 24.0,
        })
        .sum::<f32>()
        + 10.0
}

/// The menu panel: its rows, paper, border and shadow.
fn panel<'a>(entries: Vec<Entry>, t: Tokens) -> Element<'a, Message> {
    let natural = natural_height(&entries);
    let rows: Vec<Element<'a, Message>> = entries
        .into_iter()
        .map(|entry| match entry {
            Entry::Separator => container(crate::widgets::controls::hairline(t.menu_border))
                .padding(Padding {
                    top: 4.0,
                    bottom: 4.0,
                    left: 10.0,
                    right: 10.0,
                })
                .into(),
            Entry::Item {
                label,
                shortcut,
                checked,
                submenu,
                open,
                hover,
                on_press,
            } => {
                let enabled = on_press.is_some();
                let mut b = button(menu_row(t, label, shortcut, checked, submenu, enabled))
                    .width(Length::Fill)
                    .padding([3, 10])
                    .style(move |_, status| {
                        // A row whose submenu is showing stays lit, as in a native menu.
                        let hot = open
                            || status == button::Status::Hovered
                            || status == button::Status::Pressed;
                        button::Style {
                            background: hot.then(|| t.menu_hover.into()),
                            text_color: if hot { t.on_accent } else { t.text },
                            border: Border {
                                radius: 4.0.into(),
                                ..Border::default()
                            },
                            ..button::Style::default()
                        }
                    });
                if let Some(message) = on_press {
                    b = b.on_press(message);
                }
                match hover {
                    Some(message) => mouse_area(b).on_enter(*message).into(),
                    None => b.into(),
                }
            }
        })
        .collect();
    // A table adds about fifteen rows to the Insert menu; keep it inside the window.
    let list: Element<'a, Message> = if natural > MENU_MAX_HEIGHT {
        scrollable(column(rows).spacing(0).padding(5))
            .height(Length::Fixed(MENU_MAX_HEIGHT))
            .into()
    } else {
        column(rows).spacing(0).padding(5).into()
    };
    container(list)
        .width(Length::Fixed(MENU_WIDTH))
        .style(move |_| container::Style {
            background: Some(t.menu.into()),
            border: Border {
                color: t.menu_border,
                width: 1.0,
                radius: 8.0.into(),
            },
            shadow: Shadow {
                color: t.shadow,
                offset: Vector::new(0.0, 6.0),
                blur_radius: 18.0,
            },
            ..container::Style::default()
        })
        .into()
}

fn menu_row<'a>(
    t: Tokens,
    label: String,
    shortcut: Option<&'static str>,
    checked: bool,
    submenu: bool,
    enabled: bool,
) -> Element<'a, Message> {
    // A text check mark inherits the row colour, so it stays readable on the hover fill.
    let check: Element<'a, Message> = text(if checked { "\u{2713}" } else { "" })
        .size(13)
        .width(Length::Fixed(13.0))
        .into();
    let label_colour = if enabled { None } else { Some(t.text_disabled) };
    let mut label = text(label).size(13);
    if let Some(c) = label_colour {
        label = label.color(c);
    }
    let mut r = row![check, label, space::horizontal()]
        .spacing(8)
        .align_y(Center);
    if let Some(sc) = shortcut {
        r = r.push(text(sc).size(12).color(t.text_secondary));
    }
    if submenu {
        // A text arrow, like the check mark, takes the row's colour (including the hover fill).
        r = r.push(text("\u{203A}").size(18).width(Length::Fixed(10.0)));
    }
    r.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checklist_points_menu_offers_edit_above_delete() {
        let labels: Vec<String> = context_entries(&ContextMenuTarget::ChecklistPoint { id: 3 })
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Item { label, .. } => Some(label),
                Entry::Separator => None,
            })
            .collect();
        assert_eq!(labels, [s::CHECKLIST_EDIT, s::CHECKLIST_REMOVE]);
    }

    #[test]
    fn left_buttons_are_ordered_view_page_break_zoom() {
        assert!(view_x() < full_screen_x());
        assert!(full_screen_x() < zoom_x());
        // The combined Zoom control ends after its own width, then Insert starts.
        let zoom_end = zoom_x() + W_ZOOM;
        assert!(menu_x(Menu::Insert, 1200.0) >= zoom_end);
    }

    #[test]
    fn zoom_width_matches_its_parts() {
        let parts = ZOOM_STEP_W * 2.0 + ZOOM_MAIN_W + BUTTON_SPACING * 2.0;
        assert!((W_ZOOM - parts).abs() < 0.01);
    }

    #[test]
    fn left_width_matches_the_row_it_describes() {
        let expected = W_VIEW + W_FULL_SCREEN + W_ZOOM + 2.0 * BUTTON_SPACING;
        assert!((left_width() - expected).abs() < 0.01);
    }

    #[test]
    fn menus_anchor_under_their_own_buttons() {
        assert_eq!(menu_x(Menu::View, 1200.0), view_x());
        // The Zoom menu opens under the middle button, past the minus button.
        assert_eq!(
            menu_x(Menu::Zoom, 1200.0),
            zoom_x() + ZOOM_STEP_W + BUTTON_SPACING
        );
        assert!(menu_x(Menu::Insert, 1200.0) > zoom_x());
    }

    #[test]
    fn the_view_menu_checks_pageless() {
        let state = AppState {
            pageless: true,
            ..AppState::default()
        };
        let entries = entries(Menu::View, &state);
        let item = entries
            .iter()
            .find(|entry| matches!(entry, Entry::Item { label, .. } if label == s::VIEW_PAGELESS))
            .expect("a Pageless item");
        assert!(matches!(item, Entry::Item { checked: true, .. }));
    }

    #[test]
    fn inspector_buttons_share_one_width_and_gap() {
        // Format, Document, Headers and Notes: one width each, so the group is an even strip
        // and the active pill is the same size wherever it is.
        let first = RIGHT_WIDTHS[0];
        assert!(RIGHT_WIDTHS.iter().all(|width| *width == first));
        assert!(
            first >= 76.0,
            "the widest caption (Document) must fit its button"
        );
        // The gaps between them are the toolbar's one spacing value, as for every other group.
        assert_eq!(
            group_width(&RIGHT_WIDTHS),
            first * RIGHT_WIDTHS.len() as f32 + BUTTON_SPACING * (RIGHT_WIDTHS.len() - 1) as f32
        );
    }

    #[test]
    fn export_menu_offers_the_formats_then_the_ao3_chapter() {
        let state = AppState::default();
        let labels: Vec<String> = entries(Menu::Export, &state)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Item { label, .. } => Some(label),
                Entry::Separator => None,
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                s::EXPORT_PDF.to_owned(),
                s::EXPORT_PDF_ARCHIVAL.to_owned(),
                s::EXPORT_EPUB.to_owned(),
                s::EXPORT_RTF.to_owned(),
                s::EXPORT_TEXT.to_owned(),
                s::EXPORT_MARKDOWN.to_owned(),
                s::EXPORT_PLAIN_HTML.to_owned(),
                s::EXPORT_AO3.to_owned(),
            ]
        );
        assert!(matches!(
            entries(Menu::Export, &state)
                .into_iter()
                .find(|entry| matches!(entry, Entry::Item { label, .. } if label == s::EXPORT_AO3)),
            Some(Entry::Item {
                on_press: Some(Message::OpenAo3Chapters),
                ..
            })
        ));
    }

    #[test]
    fn the_ao3_row_has_an_arrow_and_stays_lit_while_its_list_is_open() {
        let mut state = AppState::default();
        let ao3 = |state: &AppState| {
            entries(Menu::Export, state)
                .into_iter()
                .find(|entry| matches!(entry, Entry::Item { label, .. } if label == s::EXPORT_AO3))
        };
        assert!(matches!(
            ao3(&state),
            Some(Entry::Item {
                submenu: true,
                open: false,
                ..
            })
        ));
        state.ao3_open = true;
        assert!(matches!(
            ao3(&state),
            Some(Entry::Item {
                submenu: true,
                open: true,
                ..
            })
        ));
        // No other Export row opens a submenu.
        assert!(
            entries(Menu::Export, &state)
                .iter()
                .filter(|entry| matches!(entry, Entry::Item { submenu: true, .. }))
                .count()
                == 1
        );
    }

    #[test]
    fn resting_on_the_ao3_row_opens_its_chapters_and_other_rows_put_them_away() {
        let hovers = |state: &AppState| -> Vec<(String, Option<Box<Message>>)> {
            entries(Menu::Export, state)
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Item { label, hover, .. } => Some((label, hover)),
                    Entry::Separator => None,
                })
                .collect()
        };
        let mut state = AppState::default();
        // Closed: only the AO3 row reacts to the pointer, and it opens the list.
        for (label, hover) in hovers(&state) {
            if label == s::EXPORT_AO3 {
                assert!(matches!(hover.as_deref(), Some(Message::OpenAo3Chapters)));
            } else {
                assert!(
                    hover.is_none(),
                    "{label} needs no hover while the list is closed"
                );
            }
        }
        // Open: the other rows put it away when the pointer reaches them.
        state.ao3_open = true;
        for (label, hover) in hovers(&state) {
            if label == s::EXPORT_AO3 {
                assert!(matches!(hover.as_deref(), Some(Message::OpenAo3Chapters)));
            } else {
                assert!(
                    matches!(hover.as_deref(), Some(Message::CloseAo3List)),
                    "{label} should close the list"
                );
            }
        }
        // Other menus never have hover behaviour.
        for menu in [Menu::View, Menu::Zoom, Menu::Insert] {
            assert!(
                entries(menu, &state)
                    .iter()
                    .all(|entry| !matches!(entry, Entry::Item { hover: Some(_), .. }))
            );
        }
    }

    #[test]
    fn the_export_button_is_right_of_every_other_centre_button() {
        assert!(menu_x(Menu::Export, 1200.0) > menu_x(Menu::Insert, 1200.0));
    }

    #[test]
    fn table_commands_live_in_the_insert_menu_when_the_caret_is_in_a_table() {
        let state = AppState {
            in_table: true,
            ..AppState::default()
        };
        let labels: Vec<String> = entries(Menu::Insert, &state)
            .into_iter()
            .filter_map(|entry| match entry {
                Entry::Item { label, .. } => Some(label),
                Entry::Separator => None,
            })
            .collect();
        assert!(
            labels.contains(&s::TABLE_ROW_ABOVE.to_owned()),
            "{labels:?}"
        );
        assert!(
            labels.contains(&s::TABLE_STYLE_NONE.to_owned()),
            "{labels:?}"
        );
        let outside = AppState::default();
        assert!(
            !entries(Menu::Insert, &outside).iter().any(
                |entry| matches!(entry, Entry::Item { label, .. } if label == s::TABLE_ROW_ABOVE)
            ),
            "no table commands without a table"
        );
    }

    #[test]
    fn view_menu_offers_bookmarks_and_checks_the_open_pane() {
        let state = AppState {
            inspector: Inspector::Bookmarks,
            sidebar_visible: true,
            ..AppState::default()
        };
        let entries = entries(Menu::View, &state);
        let bookmark = entries
            .iter()
            .find(|entry| matches!(entry, Entry::Item { label, .. } if label == s::VIEW_BOOKMARKS))
            .expect("a Bookmarks item");
        assert!(matches!(bookmark, Entry::Item { checked: true, .. }));
    }

    #[test]
    fn only_a_long_menu_scrolls() {
        // The View menu used to be treated as a 420 px scroll area and showed a large
        // blank strip under its last item; its contents are shorter than that.
        let state = AppState::default();
        assert!(natural_height(&entries(Menu::View, &state)) < MENU_MAX_HEIGHT);

        // The Insert menu with a table's commands is the case the limit exists for.
        let in_table = AppState {
            in_table: true,
            ..AppState::default()
        };
        assert!(natural_height(&entries(Menu::Insert, &in_table)) > MENU_MAX_HEIGHT);
    }
}
