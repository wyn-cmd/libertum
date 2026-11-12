//! The Pages-style toolbar and its dropdown menus.
//!
//! The toolbar is a fixed-height strip of flat icon buttons with a small caption underneath.
//! On macOS it is merged into the (transparent, full-size) title bar, so the left edge leaves
//! room for the traffic-light buttons and empty space drags the window. Dropdowns are drawn
//! by the app as a layer above everything else; [`menu_x`] tells it where to anchor them from
//! the same fixed widths the toolbar lays out with.

use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Border, Center, Color, Element, Length, Padding, Shadow, Vector};

use crate::app::Message;
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
const ZOOM_STEP_W: f32 = 20.0;
const ZOOM_NUMBER_W: f32 = 30.0;
const ZOOM_UNIT_W: f32 = 12.0;
const ZOOM_CHEVRON_W: f32 = 16.0;
const ZOOM_INSET: f32 = 2.0;
/// The combined Zoom control: inset, two step buttons, the number, the unit and the chevron.
const W_ZOOM: f32 =
    ZOOM_INSET * 2.0 + ZOOM_STEP_W * 2.0 + ZOOM_NUMBER_W + ZOOM_UNIT_W + ZOOM_CHEVRON_W;
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
        Menu::Zoom => zoom_x(),
        Menu::Insert => centre_x(window_width),
        Menu::Table => centre_x(window_width) + CENTRE_WIDTHS[0] + BUTTON_SPACING,
    }
}

/// The zoom percentage, the 1% step buttons and the menu chevron in one control, so the
/// arrows are part of the Zoom button rather than a second control beside it. The percentage
/// stays editable for an exact value.
fn zoom_control<'a>(state: &AppState, percent: u16, t: Tokens) -> Element<'a, Message> {
    let open = state.open_menu == Some(Menu::Zoom);
    let shown = state
        .zoom_edit
        .clone()
        .unwrap_or_else(|| percent.to_string());
    let input = text_input("", &shown)
        .on_input(Message::ZoomText)
        .on_submit(Message::ZoomCommit)
        .size(13.0)
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: 0.0,
            right: 0.0,
        })
        .width(Length::Fixed(ZOOM_NUMBER_W))
        .align_x(Center)
        .style(move |_, _| text_input::Style {
            background: Color::TRANSPARENT.into(),
            border: Border::default(),
            icon: t.text_secondary,
            placeholder: t.text_secondary,
            value: t.text,
            selection: Color {
                a: 0.35,
                ..t.accent
            },
        });
    let chevron = button(
        container(icon(Icon::ChevronDown, 9.0, t.text_secondary))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(ZOOM_CHEVRON_W))
    .height(Length::Fixed(24.0))
    .padding(0)
    .style(move |_, status| zoom_flat(t, status))
    .on_press(Message::ToggleMenu(Menu::Zoom));
    let unit = container(text("%").size(11.0).color(t.text_secondary))
        .center_x(Length::Fixed(ZOOM_UNIT_W));
    let body = row![
        zoom_step(t, Icon::Minus, -1),
        input,
        unit,
        chevron,
        zoom_step(t, Icon::Plus, 1),
    ]
    .align_y(Center)
    .spacing(0);
    let control = container(body)
        .width(Length::Fixed(W_ZOOM))
        .height(Length::Fixed(28.0))
        .padding(Padding {
            top: 0.0,
            bottom: 0.0,
            left: ZOOM_INSET,
            right: ZOOM_INSET,
        })
        .style(move |_| container::Style {
            background: Some(t.control.into()),
            border: Border {
                color: if open { t.accent } else { t.control_border },
                width: 1.0,
                radius: 5.0.into(),
            },
            ..container::Style::default()
        });
    column![control, text(s::TB_ZOOM).size(10.0).color(t.text_secondary),]
        .spacing(2.0)
        .align_x(Center)
        .width(Length::Fixed(W_ZOOM))
        .into()
}

fn zoom_step<'a>(t: Tokens, glyph: Icon, direction: i8) -> Element<'a, Message> {
    button(
        container(icon(glyph, 12.0, t.text_secondary))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .width(Length::Fixed(ZOOM_STEP_W))
    .height(Length::Fixed(24.0))
    .padding(0)
    .style(move |_, status| zoom_flat(t, status))
    .on_press(Message::ZoomStep(i16::from(direction)))
    .into()
}

fn zoom_flat(t: Tokens, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(t.hover.into()),
        button::Status::Pressed => Some(t.pressed.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: t.text,
        border: Border {
            radius: 4.0.into(),
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
        .style(move |_, status| {
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
        });
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
        Btn::icon(Icon::Table, s::TB_TABLE, CENTRE_WIDTHS[1])
            .chevron()
            .active(state.open_menu == Some(Menu::Table))
            .on_press(Message::ToggleMenu(Menu::Table)),
        Btn::icon(Icon::Image, s::TB_IMAGE, CENTRE_WIDTHS[2])
            .on_press(Message::Insert(InsertAction::Image)),
        Btn::icon(Icon::Asterisk, s::TB_FOOTNOTE, CENTRE_WIDTHS[3])
            .on_press(Message::Insert(InsertAction::Footnote)),
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
            on_press: Some(on_press),
        }
    }

    fn disabled(label: impl Into<String>) -> Self {
        Self::Item {
            label: label.into(),
            shortcut: None,
            checked: false,
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
            Entry::item(s::VIEW_HOME, Message::GoHome)
                .shortcut(shortcuts::label(Shortcut::CloseDocument)),
            Entry::item(s::VIEW_STITCH, Message::OpenStitch),
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
            Entry::item(s::VIEW_INVISIBLES, Message::View(ViewAction::Invisibles))
                .checked(state.show_invisibles),
            Entry::item(s::VIEW_BOOKLET, Message::View(ViewAction::Booklet)).checked(state.booklet),
            Entry::item(s::VIEW_PAGELESS, Message::View(ViewAction::Pageless))
                .checked(state.pageless),
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
        Menu::Table => {
            let enabled = state.in_table;
            let item = |action: TableAction| {
                if enabled {
                    Entry::item(action.to_string(), Message::Table(action))
                } else {
                    Entry::disabled(action.to_string())
                }
            };
            let when = |possible: bool, action: TableAction| {
                if possible {
                    Entry::item(action.to_string(), Message::Table(action))
                } else {
                    Entry::disabled(action.to_string())
                }
            };
            let mut list = vec![
                Entry::item(s::TABLE_INSERT, Message::Table(TableAction::InsertTable)),
                Entry::Separator,
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
            ];
            // Table styles apply to the caret's table; the current one is ticked.
            if enabled {
                list.push(Entry::Separator);
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
                    list.push(
                        Entry::item(style.name, Message::TableStyle(Some(index))).checked(current),
                    );
                }
            }
            list
        }
        Menu::Insert => {
            let item = |a: InsertAction| Entry::item(a.to_string(), Message::Insert(a));
            vec![
                item(InsertAction::PageBreak),
                item(InsertAction::SectionBreak),
                Entry::Separator,
                item(InsertAction::Table),
                item(InsertAction::Image),
                Entry::Separator,
                item(InsertAction::Footnote),
                item(InsertAction::Endnote),
                item(InsertAction::TableOfContents),
                item(InsertAction::PageNumber),
                Entry::Separator,
                Entry::item(s::INSERT_DATE_TIME, Message::OpenDateTime),
            ]
        }
    }
}

const MENU_WIDTH: f32 = 232.0;

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

/// What a right-click menu offers: the thing under the pointer decides.
pub enum ContextMenuTarget {
    Text,
    Table {
        merge_right: bool,
        merge_below: bool,
        split: bool,
    },
    Image {
        block: libertum_model::BlockId,
        index: usize,
    },
}

/// The right-click menu, anchored at `x`/`y` in window logical pixels.
pub fn context_layer<'a>(
    target: &ContextMenuTarget,
    x: f32,
    y: f32,
    t: Tokens,
) -> Element<'a, Message> {
    let panel = panel(context_entries(target), t);
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
        ContextMenuTarget::Text => vec![
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
            Entry::item(s::VIEW_FIND, Message::OpenFind).shortcut(shortcuts::label(Shortcut::Find)),
        ],
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
    }
}

/// The menu panel: its rows, paper, border and shadow.
fn panel<'a>(entries: Vec<Entry>, t: Tokens) -> Element<'a, Message> {
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
                on_press,
            } => {
                let enabled = on_press.is_some();
                let mut b = button(menu_row(t, label, shortcut, checked, enabled))
                    .width(Length::Fill)
                    .padding([3, 10])
                    .style(move |_, status| {
                        let hot =
                            status == button::Status::Hovered || status == button::Status::Pressed;
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
                b.into()
            }
        })
        .collect();
    container(column(rows).spacing(0).padding(5))
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
    r.into()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let parts =
            ZOOM_INSET * 2.0 + ZOOM_STEP_W * 2.0 + ZOOM_NUMBER_W + ZOOM_UNIT_W + ZOOM_CHEVRON_W;
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
        assert_eq!(menu_x(Menu::Zoom, 1200.0), zoom_x());
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
}
