//! Format inspector: paragraph style on top, then Style / Layout / More panes.

use iced::font::{Style as FontStyle, Weight};
use iced::widget::{button, column, container, row, space, text, tooltip};
use iced::{Border, Center, Color, Element, Length, Padding};

use crate::app::{FormatMsg, Message};
use crate::icons::{Icon, icon};
use crate::inspector::field;
use crate::state::{
    Alignment, AppState, Baseline, BorderStyle, Field, FontWeight, FormatPane, FormatState,
    Ligature, LineSpacingMode, ListKind, Mixed, ParagraphStyle, Swatch, SwatchTarget, TableAction,
};
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{
    PAD_X, caption, checkbox_row, chip, divider, flat, labelled, popup, section,
};
use crate::widgets::segmented::{self, Content, Look, Segment};
use libertum_model::BorderSides;

const STEPPER_W: f32 = 80.0;

/// Fixed top part: paragraph style popup with its Update button, and the pane switcher.
pub fn header<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    let style = popup(
        t,
        ParagraphStyle::ALL,
        f.paragraph_style.value(),
        s::MIXED,
        |v| Message::Format(FormatMsg::Style(v)),
    )
    .text_size(14)
    .padding(Padding {
        top: 5.0,
        bottom: 5.0,
        left: 10.0,
        right: 10.0,
    });
    // Enabled once the selection's paragraph has formatting of its own on top of its style.
    let update = tip(
        t,
        button(text(s::UPDATE).size(12))
            .padding([4, 10])
            .style(move |_, status| flat(t, status))
            .on_press_maybe(
                f.style_differs
                    .then_some(Message::Format(FormatMsg::UpdateStyle)),
            ),
        s::UPDATE_STYLE_TIP,
    );

    let panes = segmented::select(
        t,
        Look::Raised,
        &[
            (FormatPane::Style, Content::Label(s::PANE_STYLE)),
            (FormatPane::Layout, Content::Label(s::PANE_LAYOUT)),
            (FormatPane::More, Content::Label(s::PANE_MORE)),
        ],
        Some(state.format_pane),
        Message::FormatPane,
    );

    column![
        container(column![row![style, update].spacing(6).align_y(Center), panes].spacing(10))
            .padding(Padding {
                top: 12.0,
                bottom: 12.0,
                left: PAD_X,
                right: PAD_X,
            }),
        divider(t),
    ]
    .into()
}

pub fn body<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    if state.markdown {
        // Markdown mode (PLAN §7): only what a Markdown file can store is offered.
        return match state.format_pane {
            FormatPane::Style => markdown_style_pane(state, t),
            FormatPane::Layout | FormatPane::More => note(t, s::MARKDOWN_LAYOUT_NOTE),
        };
    }
    match state.format_pane {
        FormatPane::Style => with_images(state, t, style_pane(state, t)),
        FormatPane::Layout => layout_pane(state, t),
        FormatPane::More => more_pane(state, t),
    }
}

/// A paragraph of explanation in place of controls that do not apply.
pub fn note<'a>(t: Tokens, message: &'a str) -> Element<'a, Message> {
    container(text(message).size(12).color(t.text_secondary))
        .padding(Padding {
            top: 14.0,
            bottom: 14.0,
            left: PAD_X,
            right: PAD_X,
        })
        .width(Length::Fill)
        .into()
}

/// The Style pane for a Markdown file: bold, italic and strikethrough, lists, and the images
/// of the caret's paragraph. Everything else has no Markdown form.
fn markdown_style_pane<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    let toggles = segmented::view(
        t,
        Look::Accent,
        vec![
            toggle(
                "B",
                f.bold,
                FormatMsg::ToggleBold,
                Weight::Bold,
                FontStyle::Normal,
            ),
            toggle(
                "I",
                f.italic,
                FormatMsg::ToggleItalic,
                Weight::Normal,
                FontStyle::Italic,
            ),
            toggle(
                "S",
                f.strikethrough,
                FormatMsg::ToggleStrike,
                Weight::Normal,
                FontStyle::Normal,
            ),
        ],
    );
    let pane: Element<'a, Message> = column![
        section(t, s::SECTION_FONT, toggles),
        section(
            t,
            s::SECTION_LISTS,
            popup(t, ListKind::ALL, f.list_kind.value(), s::MIXED, |v| {
                Message::Format(FormatMsg::ListKind(v))
            })
        ),
    ]
    .into();
    column![with_images(state, t, pane), note(t, s::MARKDOWN_STYLE_NOTE)].into()
}

/// `pane` with the Image group on top when the caret's paragraph holds images.
fn with_images<'a>(
    state: &'a AppState,
    t: Tokens,
    pane: Element<'a, Message>,
) -> Element<'a, Message> {
    if state.images.is_empty() {
        return pane;
    }
    column![section(t, s::SECTION_IMAGE, image_section(state, t)), pane].into()
}

/// One block per image: its width with Smaller/Larger buttons, its alternative text and a
/// Remove button.
fn image_section<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let inches = state.units == crate::state::Units::Inches;
    let mut list = column![].spacing(12);
    for image in &state.images {
        let (block, index) = (image.block, image.index);
        let step = |label: &'static str, direction: i8| {
            button(text(label).size(12))
                .padding([4, 10])
                .style(move |_, status| flat(t, status))
                .on_press(Message::ImageStep(block, index, direction))
        };
        list = list.push(
            column![
                row![
                    text(s::image_width(image.width_pt, inches))
                        .size(12)
                        .color(t.text),
                    space().width(Length::Fill),
                    step(s::IMAGE_SMALLER, -1),
                    step(s::IMAGE_LARGER, 1),
                ]
                .spacing(6)
                .align_y(Center),
                iced::widget::text_input(s::IMAGE_ALT, &image.alt)
                    .on_input(move |value| Message::ImageAlt(block, index, value))
                    .size(12)
                    .padding(6)
                    .width(Length::Fill),
                button(text(s::IMAGE_REMOVE).size(12).color(t.accent))
                    .padding(0)
                    .style(button::text)
                    .on_press(Message::ImageRemove(block, index)),
            ]
            .spacing(6),
        );
    }
    list.into()
}

// ------------------------------------------------------------------ Style pane

fn style_pane<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    column![
        section(t, s::SECTION_FONT, font_section(state, t)),
        section(t, s::SECTION_ALIGNMENT, alignment_section(f, t)),
        section(t, s::SECTION_CHARACTER_COLOUR, colour_section(state, t)),
        section(
            t,
            s::SECTION_LISTS,
            popup(t, ListKind::ALL, f.list_kind.value(), s::MIXED, |v| {
                Message::Format(FormatMsg::ListKind(v))
            })
        ),
        section(t, s::SECTION_SPACING, spacing_section(state, t)),
    ]
    .into()
}

/// A tooltip in the app's pill style.
fn tip<'a>(
    t: Tokens,
    content: impl Into<Element<'a, Message>>,
    label: &'a str,
) -> Element<'a, Message> {
    tooltip(
        content,
        container(text(label).size(12).color(t.pill_text))
            .padding(Padding::from([6.0, 10.0]))
            .style(move |_| container::Style {
                background: Some(t.pill.into()),
                border: Border {
                    radius: 6.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }),
        tooltip::Position::Top,
    )
    .gap(6)
    .into()
}

/// The gear beside B/I/U/S: shows or hides the advanced font options.
fn gear<'a>(t: Tokens, open: bool) -> Element<'a, Message> {
    let colour = if open { t.accent } else { t.text };
    tip(
        t,
        button(
            container(icon(Icon::Settings, 15.0, colour))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(24.0))
        .padding(0)
        .style(move |_, status| {
            let mut style = flat(t, status);
            if open && style.background.is_none() {
                style.background = Some(t.segment_track.into());
            }
            style
        })
        .on_press(Message::Format(FormatMsg::ToggleFontAdvanced)),
        s::FONT_OPTIONS_TIP,
    )
}

/// Advanced font options: baseline (superscript, subscript) and character spacing.
fn font_advanced<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    row![
        labelled(
            t,
            s::BASELINE,
            popup(t, Baseline::ALL, f.baseline.value(), s::MIXED, |v| {
                Message::Format(FormatMsg::Baseline(v))
            })
        ),
        labelled(t, s::CHAR_SPACING, field(state, t, Field::CharSpacing)),
    ]
    .spacing(8)
    .into()
}

fn font_section<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    let toggles = segmented::view(
        t,
        Look::Accent,
        vec![
            toggle(
                "B",
                f.bold,
                FormatMsg::ToggleBold,
                Weight::Bold,
                FontStyle::Normal,
            ),
            toggle(
                "I",
                f.italic,
                FormatMsg::ToggleItalic,
                Weight::Normal,
                FontStyle::Italic,
            ),
            toggle(
                "U",
                f.underline,
                FormatMsg::ToggleUnderline,
                Weight::Normal,
                FontStyle::Normal,
            ),
            toggle(
                "S",
                f.strikethrough,
                FormatMsg::ToggleStrike,
                Weight::Normal,
                FontStyle::Normal,
            ),
        ],
    );
    let mut col = column![
        popup(t, &state.fonts[..], f.font_family.as_ref(), s::MIXED, |v| {
            Message::Format(FormatMsg::Family(v))
        }),
        row![
            popup(t, FontWeight::ALL, f.font_weight.value(), s::MIXED, |v| {
                Message::Format(FormatMsg::Weight(v))
            }),
            container(field(state, t, Field::FontSize)).width(Length::Fixed(STEPPER_W)),
        ]
        .spacing(8),
        row![toggles, gear(t, state.font_advanced)]
            .spacing(6)
            .align_y(Center),
    ]
    .spacing(8);
    if state.font_advanced {
        col = col.push(font_advanced(state, t));
    }
    col.into()
}

fn toggle<'a>(
    label: &'static str,
    value: Mixed<bool>,
    msg: FormatMsg,
    weight: Weight,
    style: FontStyle,
) -> Segment<'a, Message> {
    Segment::new(
        Content::Styled(label, weight, style),
        value == Mixed::Value(true),
        Message::Format(msg),
    )
    .mixed(value.is_mixed())
}

pub fn swatch_colour(swatch: Swatch) -> Color {
    match swatch {
        Swatch::Black => Color::from_rgb8(0x1d, 0x1d, 0x1f),
        Swatch::Grey => Color::from_rgb8(0x8e, 0x8e, 0x93),
        Swatch::Red => Color::from_rgb8(0xff, 0x3b, 0x30),
        Swatch::Orange => Color::from_rgb8(0xff, 0x95, 0x00),
        Swatch::Yellow => Color::from_rgb8(0xff, 0xd6, 0x0a),
        Swatch::Green => Color::from_rgb8(0x34, 0xc7, 0x59),
        Swatch::Blue => Color::from_rgb8(0x00, 0x7a, 0xff),
        Swatch::Purple => Color::from_rgb8(0xaf, 0x52, 0xde),
    }
}

fn colour_section<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    let text_colour = f.colour.value().map(swatch_colour);
    let (highlight_colour, highlight_name) = match f.highlight {
        Mixed::Value(Some(sw)) => (Some(swatch_colour(sw)), sw.to_string()),
        Mixed::Value(None) => (None, s::NO_HIGHLIGHT.to_owned()),
        Mixed::Mixed => (None, s::MIXED.to_owned()),
    };
    let text_name = f
        .colour
        .value()
        .map_or_else(|| s::MIXED.to_owned(), |c| c.to_string());

    let mut col = column![
        row![
            labelled(
                t,
                s::COLOUR,
                swatch_button(
                    t,
                    text_name,
                    text_colour,
                    state.open_swatch == Some(SwatchTarget::Character),
                    SwatchTarget::Character
                )
            ),
            labelled(
                t,
                s::HIGHLIGHT,
                swatch_button(
                    t,
                    highlight_name,
                    highlight_colour,
                    state.open_swatch == Some(SwatchTarget::Highlight),
                    SwatchTarget::Highlight
                )
            ),
        ]
        .spacing(8),
    ]
    .spacing(8);

    if let Some(target) = state.open_swatch {
        let selected = match target {
            SwatchTarget::Character => f.colour.value(),
            SwatchTarget::Highlight => f.highlight.value().flatten(),
        };
        let mut chips = row![].spacing(6).align_y(Center);
        if target == SwatchTarget::Highlight {
            chips = chips.push(
                button(icon(Icon::Strikethrough, 12.0, t.text_secondary))
                    .padding(3)
                    .style(move |_, status| flat(t, status))
                    .on_press(Message::Format(FormatMsg::Highlight(None))),
            );
        }
        for &sw in Swatch::ALL {
            let message = match target {
                SwatchTarget::Character => FormatMsg::Colour(sw),
                SwatchTarget::Highlight => FormatMsg::Highlight(Some(sw)),
            };
            let is_selected = selected == Some(sw);
            chips = chips.push(
                button(chip(t, swatch_colour(sw), 16.0))
                    .padding(2)
                    .style(move |_, _| button::Style {
                        background: None,
                        text_color: t.text,
                        border: Border {
                            color: if is_selected {
                                t.accent
                            } else {
                                Color::TRANSPARENT
                            },
                            width: 2.0,
                            radius: 6.0.into(),
                        },
                        ..button::Style::default()
                    })
                    .on_press(Message::Format(message)),
            );
        }
        col = col.push(chips);
    }
    col.into()
}

fn swatch_button<'a>(
    t: Tokens,
    name: String,
    colour: Option<Color>,
    open: bool,
    target: SwatchTarget,
) -> Element<'a, Message> {
    let chip_el: Element<'a, Message> = chip(t, colour.unwrap_or(Color::TRANSPARENT), 14.0);
    button(
        row![
            chip_el,
            text(name).size(13).color(t.text),
            space::horizontal(),
            icon(Icon::ChevronDown, 11.0, t.text_secondary),
        ]
        .spacing(7)
        .align_y(Center),
    )
    .width(Length::Fill)
    .padding([3, 8])
    .style(move |_, status| {
        let hot = open || !matches!(status, button::Status::Active);
        button::Style {
            background: Some(if hot { t.control_hover } else { t.control }.into()),
            text_color: t.text,
            border: Border {
                color: t.control_border,
                width: 1.0,
                radius: 5.0.into(),
            },
            ..button::Style::default()
        }
    })
    .on_press(Message::ToggleSwatch(target))
    .into()
}

fn alignment_section<'a>(f: &FormatState, t: Tokens) -> Element<'a, Message> {
    let icons = [
        (Alignment::Left, Icon::AlignLeft),
        (Alignment::Centre, Icon::AlignCentre),
        (Alignment::Right, Icon::AlignRight),
        (Alignment::Justify, Icon::AlignJustify),
    ];
    let items: Vec<_> = icons
        .iter()
        .map(|&(a, glyph)| (a, Content::Icon(glyph)))
        .collect();
    let align = segmented::select(t, Look::Accent, &items, f.alignment.value(), |a| {
        Message::Format(FormatMsg::Align(a))
    });
    let indent = |glyph: Icon, direction: i8| {
        button(
            container(icon(glyph, 15.0, t.text))
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(24.0))
        .padding(0)
        .style(move |_, status| {
            let mut style = flat(t, status);
            if style.background.is_none() {
                style.background = Some(t.segment_track.into());
            }
            style.border.radius = 5.0.into();
            style
        })
        .on_press(Message::Format(FormatMsg::Indent(direction)))
    };
    row![
        align,
        indent(Icon::IndentDecrease, -1),
        indent(Icon::IndentIncrease, 1)
    ]
    .spacing(6)
    .align_y(Center)
    .into()
}

fn spacing_section<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    column![
        row![
            popup(
                t,
                LineSpacingMode::ALL,
                f.line_spacing_mode.value(),
                s::MIXED,
                |v| Message::Format(FormatMsg::SpacingMode(v))
            ),
            container(field(
                state,
                t,
                if matches!(f.line_spacing_mode, Mixed::Value(LineSpacingMode::Lines)) {
                    Field::LineSpacing
                } else {
                    Field::LineSpacingPoints
                }
            ))
            .width(Length::Fixed(STEPPER_W)),
        ]
        .spacing(8),
        row![
            labelled(t, s::SPACE_BEFORE, field(state, t, Field::SpaceBefore)),
            labelled(t, s::SPACE_AFTER, field(state, t, Field::SpaceAfter)),
        ]
        .spacing(8),
    ]
    .spacing(8)
    .into()
}

// ----------------------------------------------------------------- Layout pane

fn layout_pane<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let mut sections: Vec<Element<'a, Message>> = Vec::new();
    // Column widths belong to the table the caret is in, so the group only appears there.
    if state.in_table {
        sections.push(section(t, s::SECTION_TABLE, table_section(state, t)));
    }
    sections.push(section(
        t,
        s::SECTION_COLUMNS,
        row![
            labelled(t, s::COLUMNS_COUNT, field(state, t, Field::Columns)),
            labelled(t, s::COLUMNS_GUTTER, field(state, t, Field::Gutter)),
        ]
        .spacing(8),
    ));
    sections.push(section(
        t,
        s::SECTION_INDENTS,
        column![
            row![
                labelled(t, s::INDENT_FIRST, field(state, t, Field::IndentFirst)),
                labelled(t, s::INDENT_LEFT, field(state, t, Field::IndentLeft)),
            ]
            .spacing(8),
            row![
                labelled(t, s::INDENT_RIGHT, field(state, t, Field::IndentRight)),
                space::horizontal().width(Length::Fill),
            ]
            .spacing(8),
        ]
        .spacing(8),
    ));
    sections.push(section(t, s::SECTION_TABS, caption(t, s::TABS_DEFAULT)));
    sections.push(section(
        t,
        s::SECTION_BORDERS,
        column![
            popup(
                t,
                BorderStyle::ALL,
                state.format.border_style.value(),
                s::MIXED,
                |v| Message::Format(FormatMsg::BorderStyle(v))
            ),
            {
                let sides = state.format.border_sides;
                row![
                    checkbox_row(t, s::BORDER_TOP, sides.top, false, move |on| {
                        Message::Format(FormatMsg::BorderSides(BorderSides { top: on, ..sides }))
                    }),
                    checkbox_row(t, s::BORDER_RIGHT, sides.right, false, move |on| {
                        Message::Format(FormatMsg::BorderSides(BorderSides { right: on, ..sides }))
                    }),
                    checkbox_row(t, s::BORDER_BOTTOM, sides.bottom, false, move |on| {
                        Message::Format(FormatMsg::BorderSides(BorderSides {
                            bottom: on,
                            ..sides
                        }))
                    }),
                    checkbox_row(t, s::BORDER_LEFT, sides.left, false, move |on| {
                        Message::Format(FormatMsg::BorderSides(BorderSides { left: on, ..sides }))
                    }),
                ]
                .spacing(4)
            },
            {
                let mut swatches = row![].spacing(6);
                for colour in [
                    [0u8, 0, 0, 255],
                    [200, 30, 30, 255],
                    [30, 80, 200, 255],
                    [30, 120, 60, 255],
                    [180, 120, 20, 255],
                    [110, 110, 110, 255],
                ] {
                    let selected = state.format.border_colour == colour;
                    swatches = swatches.push(
                        button(space().width(18.0).height(18.0))
                            .padding(0)
                            .style(move |_, _| button::Style {
                                background: Some(
                                    Color::from_rgba8(colour[0], colour[1], colour[2], 1.0).into(),
                                ),
                                border: Border {
                                    color: if selected { t.accent } else { t.chrome_border },
                                    width: if selected { 2.0 } else { 1.0 },
                                    radius: 3.0.into(),
                                },
                                ..button::Style::default()
                            })
                            .on_press(Message::Format(FormatMsg::BorderColour(colour))),
                    );
                }
                swatches
            },
            caption(t, s::BORDERS_HINT),
        ]
        .spacing(6),
    ));
    column(sections).spacing(0).width(Length::Fill).into()
}

/// The caret table's column width: the current width, one step either way, and one press to
/// give every column the same width. The table keeps its overall width either way.
fn table_section<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let inches = state.units == crate::state::Units::Inches;
    let width = state
        .column_width_pt
        .map(|width| s::column_width(width, inches))
        .unwrap_or_else(|| s::MIXED.to_owned());
    let action = |label: &'static str, action: TableAction| {
        button(text(label).size(12))
            .padding([4, 10])
            .style(move |_, status| flat(t, status))
            .on_press(Message::Table(action))
    };
    column![
        caption(t, s::TABLE_COLUMN_WIDTH),
        row![
            text(width).size(12).color(t.text),
            space().width(Length::Fill),
            action(s::TABLE_COLUMN_NARROWER, TableAction::ColumnNarrower),
            action(s::TABLE_COLUMN_WIDER, TableAction::ColumnWider),
        ]
        .spacing(6)
        .align_y(Center),
        row![action(
            s::TABLE_COLUMN_EQUALISE,
            TableAction::EqualiseColumns
        )],
    ]
    .spacing(8)
    .into()
}

// ------------------------------------------------------------------- More pane

fn more_pane<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let f = &state.format;
    let check = |label: &'static str, value: Mixed<bool>, msg: fn(bool) -> FormatMsg| {
        checkbox_row(
            t,
            label,
            value == Mixed::Value(true),
            value.is_mixed(),
            move |on| Message::Format(msg(on)),
        )
    };
    column![
        section(
            t,
            s::SECTION_PAGINATION,
            column![
                check(
                    s::KEEP_LINES_TOGETHER,
                    f.keep_lines_together,
                    FormatMsg::KeepLines
                ),
                check(s::KEEP_WITH_NEXT, f.keep_with_next, FormatMsg::KeepWithNext),
                check(
                    s::PAGE_BREAK_BEFORE,
                    f.page_break_before,
                    FormatMsg::PageBreakBefore
                ),
                check(s::WIDOW_ORPHAN, f.widow_orphan, FormatMsg::WidowOrphan),
            ]
            .spacing(9)
        ),
        section(
            t,
            s::SECTION_HYPHENATION,
            check(
                s::REMOVE_HYPHENATION,
                f.remove_hyphenation,
                FormatMsg::RemoveHyphenation
            )
        ),
        section(
            t,
            s::SECTION_LIGATURES,
            popup(t, Ligature::ALL, f.ligatures.value(), s::MIXED, |v| {
                Message::Format(FormatMsg::Ligatures(v))
            })
        ),
    ]
    .into()
}
