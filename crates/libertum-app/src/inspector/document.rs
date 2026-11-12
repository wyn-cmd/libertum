//! Document inspector: Document (paper, header and footer, margins) and Section panes.

use iced::widget::{column, container, radio, row, space, text_input};
use iced::{Border, Center, Element, Length, Padding};

use crate::app::{DocumentMsg, Message};
use crate::icons::Icon;
use crate::inspector::field;
use crate::state::{
    AppState, DocumentPane, Field, MarginSide, Mixed, NumberingHorizontal, NumberingVertical,
    Orientation, PageSize, Units,
};
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{PAD_X, caption, checkbox_row, divider, labelled, popup, section};
use crate::widgets::segmented::{self, Content, Look};

const STEPPER_W: f32 = 84.0;

pub fn header<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let panes = segmented::select(
        t,
        Look::Raised,
        &[
            (DocumentPane::Document, Content::Label(s::PANE_DOCUMENT)),
            (DocumentPane::Section, Content::Label(s::PANE_SECTION)),
        ],
        Some(state.document_pane),
        Message::DocumentPane,
    );
    column![
        container(panes).padding(Padding {
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
        // A Markdown file has no page setup, headers, footers or sections to edit.
        return crate::inspector::format::note(t, s::MARKDOWN_DOCUMENT_NOTE);
    }
    match state.document_pane {
        DocumentPane::Document => document_pane(state, t),
        DocumentPane::Section => section_pane(state, t),
    }
}

fn document_pane<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let d = &state.document;
    let dimensions = d.page_millimetres().map_or_else(
        || s::MIXED.to_owned(),
        |(w, h)| s::paper_dimensions(w, h, state.units == Units::Inches),
    );
    let orientation = segmented::select(
        t,
        Look::Accent,
        &[
            (Orientation::Portrait, Content::Icon(Icon::Portrait)),
            (Orientation::Landscape, Content::Icon(Icon::Landscape)),
        ],
        d.orientation.value(),
        |o| Message::Document(DocumentMsg::Orientation(o)),
    );
    let paper = column![
        popup(t, PageSize::ALL, d.page_size.value(), s::MIXED, |v| {
            Message::Document(DocumentMsg::PageSize(v))
        }),
        row![
            caption(t, dimensions),
            space::horizontal(),
            container(orientation).width(Length::Fixed(72.0)),
        ]
        .align_y(Center),
    ]
    .spacing(8);

    let margin =
        |side: MarginSide| labelled(t, side_label(side), field(state, t, Field::Margin(side)));

    let footnote = if let Some(number) = d.footnote_number {
        column![
            caption(t, s::footnote_number(number)),
            text_input(s::FOOTNOTE_PLACEHOLDER, &d.footnote_text)
                .on_input(|text| Message::Document(DocumentMsg::FootnoteText(text)))
                .size(12)
                .padding(6)
                .width(Length::Fill),
        ]
        .spacing(6)
    } else {
        column![caption(t, s::FOOTNOTE_NONE)].spacing(6)
    };

    column![
        section(t, s::SECTION_PAPER, paper),
        section(t, s::SECTION_HEADER_FOOTER, header_footer(state, t)),
        section(t, s::SECTION_WATERMARK, watermark(state, t)),
        section(
            t,
            s::SECTION_MARGINS,
            column![
                row![margin(MarginSide::Top), margin(MarginSide::Bottom)].spacing(8),
                row![margin(MarginSide::Left), margin(MarginSide::Right)].spacing(8),
            ]
            .spacing(8)
        ),
        section(
            t,
            s::SECTION_TYPOGRAPHY,
            column![
                check(
                    t,
                    s::DOC_HYPHENATION,
                    d.hyphenation,
                    DocumentMsg::Hyphenation
                ),
                check(t, s::DOC_LIGATURES, d.ligatures, DocumentMsg::Ligatures),
            ]
            .spacing(9)
        ),
        section(t, s::SECTION_FOOTNOTE, footnote),
    ]
    .into()
}

/// Header & Footer: the switches, their distances and the text of each variant the section
/// uses. `{page}` and `{pages}` in the text are fields (see `HeaderFooter::resolve`).
fn header_footer<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    use libertum_model::HfVariant;
    let d = &state.document;
    let header_on = d.header == Mixed::Value(true);
    let footer_on = d.footer == Mixed::Value(true);
    let first = d.different_first_page == Mixed::Value(true);
    let even = d.different_even_odd == Mixed::Value(true);

    let field = |variant: HfVariant, value: &'a str, header: bool| -> Element<'a, Message> {
        let (label, msg): (_, fn(HfVariant, String) -> DocumentMsg) = if header {
            (
                match variant {
                    HfVariant::Default => s::HEADER_TEXT,
                    HfVariant::First => s::FIRST_PAGE_HEADER,
                    HfVariant::Even => s::EVEN_PAGE_HEADER,
                },
                DocumentMsg::HeaderText,
            )
        } else {
            (
                match variant {
                    HfVariant::Default => s::FOOTER_TEXT,
                    HfVariant::First => s::FIRST_PAGE_FOOTER,
                    HfVariant::Even => s::EVEN_PAGE_FOOTER,
                },
                DocumentMsg::FooterText,
            )
        };
        let input = text_line(t, s::HF_TEXT_PLACEHOLDER, value, move |text| {
            msg(variant, text)
        });
        labelled(t, label, input)
    };

    let mut body = column![check_with_distance(
        state,
        t,
        s::HEADER,
        d.header,
        DocumentMsg::Header,
        Field::HeaderDistance
    ),]
    .spacing(9);
    if header_on {
        body = body.push(field(HfVariant::Default, &d.header_text, true));
    }
    body = body.push(check_with_distance(
        state,
        t,
        s::FOOTER,
        d.footer,
        DocumentMsg::Footer,
        Field::FooterDistance,
    ));
    if footer_on {
        body = body.push(field(HfVariant::Default, &d.footer_text, false));
    }
    body = body.push(check(
        t,
        s::DIFFERENT_FIRST_PAGE,
        d.different_first_page,
        DocumentMsg::DifferentFirstPage,
    ));
    if first {
        if header_on {
            body = body.push(field(HfVariant::First, &d.first_header_text, true));
        }
        if footer_on {
            body = body.push(field(HfVariant::First, &d.first_footer_text, false));
        }
    }
    body = body.push(check(
        t,
        s::DIFFERENT_EVEN_ODD,
        d.different_even_odd,
        DocumentMsg::DifferentEvenOdd,
    ));
    if even {
        if header_on {
            body = body.push(field(HfVariant::Even, &d.even_header_text, true));
        }
        if footer_on {
            body = body.push(field(HfVariant::Even, &d.even_footer_text, false));
        }
    }
    body = body.push(caption(t, s::HF_FIELD_HINT));
    body.into()
}

/// The watermark text (applied on Return or Apply), its direction and Remove.
fn watermark<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let d = &state.document;
    let input = text_input(s::WATERMARK_PLACEHOLDER, &d.watermark_text)
        .on_input(|text| Message::Document(DocumentMsg::WatermarkText(text)))
        .on_submit(Message::Document(DocumentMsg::WatermarkApply))
        .size(13)
        .padding(Padding {
            top: 4.0,
            bottom: 4.0,
            left: 8.0,
            right: 8.0,
        })
        .width(Length::Fill);
    let button = |label: &'static str, message: Option<DocumentMsg>| {
        iced::widget::button(iced::widget::text(label).size(12))
            .padding([4, 10])
            .style(move |_, status| crate::widgets::controls::flat(t, status))
            .on_press_maybe(message.map(Message::Document))
    };
    let can_apply = !d.watermark_text.trim().is_empty();
    column![
        input,
        row![
            container(check(
                t,
                s::WATERMARK_DIAGONAL,
                Mixed::Value(d.watermark_diagonal),
                DocumentMsg::WatermarkDiagonal
            ))
            .width(Length::Fill),
            button(
                s::WATERMARK_APPLY,
                can_apply.then_some(DocumentMsg::WatermarkApply)
            ),
            button(
                s::WATERMARK_REMOVE,
                d.watermark_on.then_some(DocumentMsg::WatermarkRemove)
            ),
        ]
        .spacing(6)
        .align_y(Center),
        caption(t, s::WATERMARK_HINT),
    ]
    .spacing(8)
    .into()
}

/// A one-line text input styled like the other inspector controls.
fn text_line<'a>(
    t: Tokens,
    placeholder: &'a str,
    value: &'a str,
    on_input: impl Fn(String) -> DocumentMsg + 'a,
) -> Element<'a, Message> {
    text_input(placeholder, value)
        .on_input(move |text| Message::Document(on_input(text)))
        .size(13)
        .padding(Padding {
            top: 4.0,
            bottom: 4.0,
            left: 8.0,
            right: 8.0,
        })
        .width(Length::Fill)
        .style(move |_, _| text_input::Style {
            background: t.control.into(),
            border: Border {
                color: t.control_border,
                width: 1.0,
                radius: 5.0.into(),
            },
            icon: t.text_secondary,
            placeholder: t.text_secondary,
            value: t.text,
            selection: iced::Color {
                a: 0.35,
                ..t.accent
            },
        })
        .into()
}

fn side_label(side: MarginSide) -> &'static str {
    match side {
        MarginSide::Top => s::MARGIN_TOP,
        MarginSide::Bottom => s::MARGIN_BOTTOM,
        MarginSide::Left => s::MARGIN_LEFT,
        MarginSide::Right => s::MARGIN_RIGHT,
    }
}

fn check<'a>(
    t: Tokens,
    label: &'static str,
    value: Mixed<bool>,
    msg: fn(bool) -> DocumentMsg,
) -> Element<'a, Message> {
    checkbox_row(
        t,
        label,
        value == Mixed::Value(true),
        value.is_mixed(),
        move |on| Message::Document(msg(on)),
    )
}

/// A checkbox with its distance stepper on the same row; the stepper shows only when on.
fn check_with_distance<'a>(
    state: &'a AppState,
    t: Tokens,
    label: &'static str,
    value: Mixed<bool>,
    msg: fn(bool) -> DocumentMsg,
    distance: Field,
) -> Element<'a, Message> {
    let right: Element<'a, Message> = if value == Mixed::Value(true) {
        container(field(state, t, distance))
            .width(Length::Fixed(STEPPER_W))
            .into()
    } else {
        space().width(Length::Fixed(STEPPER_W)).into()
    };
    row![
        container(check(t, label, value, msg)).width(Length::Fill),
        right
    ]
    .align_y(Center)
    .spacing(8)
    .into()
}

// ---------------------------------------------------------------- Section pane

fn section_pane<'a>(state: &'a AppState, t: Tokens) -> Element<'a, Message> {
    let d = &state.document;
    let continuing = d.numbering_start.is_none();
    let style = move |_: &iced::Theme, status: radio::Status| {
        let selected = matches!(
            status,
            radio::Status::Active { is_selected: true }
                | radio::Status::Hovered { is_selected: true }
        );
        radio::Style {
            background: t.control.into(),
            dot_color: t.accent,
            border_width: 1.0,
            border_color: if selected { t.accent } else { t.control_border },
            text_color: Some(t.text),
        }
    };
    let continue_radio = radio(s::NUMBERING_CONTINUE, true, Some(continuing), |_| {
        Message::Document(DocumentMsg::NumberingContinue(true))
    })
    .size(15)
    .text_size(13)
    .style(style);
    let start_radio = radio(s::NUMBERING_START, false, Some(continuing), |_| {
        Message::Document(DocumentMsg::NumberingContinue(false))
    })
    .size(15)
    .text_size(13)
    .style(style);

    let start_row: Element<'a, Message> = if continuing {
        row![start_radio].into()
    } else {
        row![
            container(start_radio).width(Length::Fill),
            container(field(state, t, Field::NumberingStart)).width(Length::Fixed(STEPPER_W)),
        ]
        .align_y(Center)
        .spacing(8)
        .into()
    };

    let vertical = segmented::select(
        t,
        Look::Accent,
        &[
            (NumberingVertical::Top, Content::Label(s::NUMBERING_TOP)),
            (
                NumberingVertical::Bottom,
                Content::Label(s::NUMBERING_BOTTOM),
            ),
        ],
        d.numbering_vertical.value(),
        |v| Message::Document(DocumentMsg::NumberingVertical(v)),
    );
    let horizontal = segmented::select(
        t,
        Look::Accent,
        &[
            (NumberingHorizontal::Left, Content::Label(s::POSITION_LEFT)),
            (
                NumberingHorizontal::Centre,
                Content::Label(s::POSITION_CENTRE),
            ),
            (
                NumberingHorizontal::Right,
                Content::Label(s::POSITION_RIGHT),
            ),
        ],
        d.numbering_horizontal.value(),
        |v| Message::Document(DocumentMsg::NumberingHorizontal(v)),
    );

    column![
        section(
            t,
            s::SECTION_PAGE_NUMBERING,
            column![
                checkbox_row(
                    t,
                    s::NUMBERING_SHOW,
                    d.numbering_on == Mixed::Value(true),
                    d.numbering_on.is_mixed(),
                    |on| Message::Document(DocumentMsg::NumberingOn(on))
                ),
                continue_radio,
                start_row,
                check(
                    t,
                    s::HIDE_ON_FIRST_PAGE,
                    d.hide_on_first_page,
                    DocumentMsg::HideOnFirstPage
                ),
            ]
            .spacing(9)
        ),
        section(
            t,
            s::SECTION_POSITION,
            column![
                row![
                    container(vertical).width(Length::FillPortion(2)),
                    container(horizontal).width(Length::FillPortion(3)),
                ]
                .spacing(8)
                .align_y(Center),
                checkbox_row(
                    t,
                    s::NUMBERING_MIRROR,
                    d.numbering_mirror == Mixed::Value(true),
                    d.numbering_mirror.is_mixed(),
                    |on| Message::Document(DocumentMsg::NumberingMirror(on))
                ),
            ]
            .spacing(9)
        ),
        section(
            t,
            s::SECTION_LINKS,
            column![
                checkbox_row(
                    t,
                    s::LINK_HEADERS,
                    d.link_headers == Mixed::Value(true),
                    d.link_headers.is_mixed(),
                    |on| Message::Document(DocumentMsg::LinkHeaders(on))
                ),
                checkbox_row(
                    t,
                    s::LINK_FOOTERS,
                    d.link_footers == Mixed::Value(true),
                    d.link_footers.is_mixed(),
                    |on| Message::Document(DocumentMsg::LinkFooters(on))
                ),
            ]
            .spacing(9)
        ),
    ]
    .into()
}
