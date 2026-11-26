//! Shared building blocks for the inspector: popups, checkboxes, section headings, labels.

use std::borrow::Borrow;

use iced::font::Weight;
use iced::widget::overlay::menu;
use iced::widget::{button, checkbox, column, container, pick_list, rule, space, text};
use iced::{Border, Color, Element, Font, Length, Padding, Shadow, Vector};

use crate::theme::Tokens;

/// Horizontal padding of the inspector, logical pixels.
pub const PAD_X: f32 = 16.0;

pub fn bold() -> Font {
    Font {
        weight: Weight::Semibold,
        ..Font::DEFAULT
    }
}

/// Transparent button with a rounded hover / pressed background.
pub fn flat(t: Tokens, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Hovered => Some(t.hover.into()),
        button::Status::Pressed => Some(t.pressed.into()),
        _ => None,
    };
    button::Style {
        background,
        text_color: if status == button::Status::Disabled {
            t.text_disabled
        } else {
            t.text
        },
        border: Border {
            radius: 6.0.into(),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// A popup button (Iced `pick_list`) styled like a Pages popup.
pub fn popup<'a, T, L, V, M>(
    t: Tokens,
    options: L,
    selected: Option<V>,
    placeholder: &str,
    on_select: impl Fn(T) -> M + 'a,
) -> pick_list::PickList<'a, T, L, V, M>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
    M: Clone + 'a,
{
    pick_list(options, selected, on_select)
        .placeholder(placeholder)
        .width(Length::Fill)
        .text_size(13)
        .padding(Padding {
            top: 3.0,
            bottom: 3.0,
            left: 8.0,
            right: 8.0,
        })
        .style(move |_, status| {
            let hovered = !matches!(status, pick_list::Status::Active);
            pick_list::Style {
                text_color: t.text,
                placeholder_color: t.text_secondary,
                handle_color: t.text_secondary,
                background: if hovered { t.control_hover } else { t.control }.into(),
                border: Border {
                    color: t.control_border,
                    width: 1.0,
                    radius: 5.0.into(),
                },
            }
        })
        .menu_style(move |_| menu::Style {
            background: t.menu.into(),
            border: Border {
                color: t.menu_border,
                width: 1.0,
                radius: 6.0.into(),
            },
            text_color: t.text,
            selected_text_color: t.on_accent,
            selected_background: t.menu_hover.into(),
            shadow: Shadow {
                color: t.shadow,
                offset: Vector::new(0.0, 4.0),
                blur_radius: 12.0,
            },
        })
}

pub fn checkbox_row<'a, M: Clone + 'a>(
    t: Tokens,
    label: &str,
    checked: bool,
    mixed: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Element<'a, M> {
    let text = if mixed {
        format!("{label}{}", crate::strings::MIXED_SUFFIX)
    } else {
        label.to_owned()
    };
    checkbox(checked)
        .label(text)
        .size(15)
        .text_size(13)
        .spacing(8)
        .on_toggle(on_toggle)
        .style(move |_, status| {
            let on = matches!(
                status,
                checkbox::Status::Active { is_checked: true }
                    | checkbox::Status::Hovered { is_checked: true }
            );
            let hovered = matches!(status, checkbox::Status::Hovered { .. });
            checkbox::Style {
                background: if on {
                    t.accent.into()
                } else if hovered {
                    t.control_hover.into()
                } else {
                    t.control.into()
                },
                icon_color: t.on_accent,
                border: Border {
                    color: if on { t.accent } else { t.control_border },
                    width: 1.0,
                    radius: 4.0.into(),
                },
                text_color: Some(t.text),
            }
        })
        .into()
}

/// A secondary-colour caption above or beside a control.
pub fn caption<'a, M: 'a>(t: Tokens, label: impl text::IntoFragment<'a>) -> Element<'a, M> {
    text(label)
        .size(12)
        .color(t.text_secondary)
        .width(Length::Fill)
        .into()
}

/// A caption and a control stacked.
pub fn labelled<'a, M: 'a>(
    t: Tokens,
    label: &'a str,
    control: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    column![caption(t, label), control.into()]
        .spacing(4)
        .width(Length::Fill)
        .into()
}

/// An inspector section: bold 13 px heading, body, hairline divider below.
pub fn section<'a, M: 'a>(
    t: Tokens,
    title: &'a str,
    body: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    column![
        container(
            column![text(title).size(13).font(bold()).color(t.text), body.into()].spacing(10)
        )
        .padding(Padding {
            top: 12.0,
            bottom: 14.0,
            left: PAD_X,
            right: PAD_X,
        })
        .width(Length::Fill),
        divider(t),
    ]
    .into()
}

pub fn divider<'a, M: 'a>(t: Tokens) -> Element<'a, M> {
    hairline(t.divider)
}

/// A one-pixel horizontal line.
pub fn hairline<'a, M: 'a>(colour: Color) -> Element<'a, M> {
    rule::horizontal(1)
        .style(move |_| rule::Style {
            color: colour,
            radius: 0.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        })
        .into()
}

/// A colour swatch chip of `size` logical pixels.
pub fn chip<'a, M: 'a>(t: Tokens, colour: Color, size: f32) -> Element<'a, M> {
    container(
        space()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .style(move |_| container::Style {
        background: Some(colour.into()),
        border: Border {
            color: t.control_border,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    })
    .into()
}
