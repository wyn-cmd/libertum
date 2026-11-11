//! Segmented control: a rounded track of equal segments, each a label or an icon.
//!
//! Used for the Style / Layout / More and Document / Section tabs (one selected, raised
//! look), alignment and orientation (accent look), and the B I U S toggles (each segment
//! independent).

use iced::font::{Style as FontStyle, Weight};
use iced::widget::{button, container, row, text};
use iced::{Border, Center, Element, Font, Length, Shadow, Vector};

use crate::icons::{Icon, icon};
use crate::theme::Tokens;

const HEIGHT: f32 = 24.0;

/// What a segment shows.
#[derive(Debug, Clone, Copy)]
pub enum Content<'a> {
    Label(&'a str),
    /// A styled letter (B, I, U, S).
    Styled(&'a str, Weight, FontStyle),
    Icon(Icon),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// Selected segment is raised and light (tab style).
    Raised,
    /// Selected segment is filled with the accent colour.
    Accent,
}

pub struct Segment<'a, M> {
    pub content: Content<'a>,
    pub on: bool,
    /// Indeterminate: drawn outlined, neither on nor off.
    pub mixed: bool,
    pub width: Length,
    pub on_press: M,
}

impl<'a, M> Segment<'a, M> {
    pub fn new(content: Content<'a>, on: bool, on_press: M) -> Self {
        Self {
            content,
            on,
            mixed: false,
            width: Length::Fill,
            on_press,
        }
    }

    pub fn mixed(mut self, mixed: bool) -> Self {
        self.mixed = mixed;
        self
    }
}

pub fn view<'a, M: Clone + 'a>(
    t: Tokens,
    look: Look,
    segments: Vec<Segment<'a, M>>,
) -> Element<'a, M> {
    let buttons = segments.into_iter().map(|seg| {
        let (fg, bg) = match (look, seg.on) {
            (Look::Accent, true) => (t.on_accent, Some(t.accent)),
            (Look::Raised, true) => (t.text, Some(t.segment_selected)),
            (_, false) => (t.text, None),
        };
        let body: Element<'a, M> = match seg.content {
            Content::Label(label) => text(label).size(13).color(fg).into(),
            Content::Styled(label, weight, style) => text(label)
                .size(13)
                .color(fg)
                .font(Font {
                    weight,
                    style,
                    ..Font::DEFAULT
                })
                .into(),
            Content::Icon(glyph) => icon(glyph, 15.0, fg).into(),
        };
        let raised = look == Look::Raised && seg.on;
        let mixed = seg.mixed;
        button(
            container(body)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        )
        .width(seg.width)
        .height(Length::Fixed(HEIGHT - 2.0))
        .padding(0)
        .style(move |_, status| {
            let hover = matches!(status, button::Status::Hovered | button::Status::Pressed);
            let background = match (bg, hover) {
                (Some(c), _) => Some(c.into()),
                (None, true) => Some(t.hover.into()),
                (None, false) => None,
            };
            button::Style {
                background,
                text_color: fg,
                border: Border {
                    radius: 5.0.into(),
                    width: if mixed { 1.0 } else { 0.0 },
                    color: t.accent,
                },
                shadow: if raised {
                    Shadow {
                        color: iced::Color::from_rgba(
                            0.0,
                            0.0,
                            0.0,
                            if t.dark { 0.35 } else { 0.14 },
                        ),
                        offset: Vector::new(0.0, 0.5),
                        blur_radius: 1.5,
                    }
                } else {
                    Shadow::default()
                },
                ..button::Style::default()
            }
        })
        .on_press(seg.on_press)
        .into()
    });
    container(row(buttons).spacing(1).align_y(Center))
        .padding(1)
        .width(Length::Fill)
        .height(Length::Fixed(HEIGHT))
        .style(move |_| container::Style {
            background: Some(t.segment_track.into()),
            border: Border {
                radius: 6.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
        .into()
}

/// A single-selection control over a list of values.
pub fn select<'a, V, M>(
    t: Tokens,
    look: Look,
    items: &[(V, Content<'a>)],
    selected: Option<V>,
    on_select: impl Fn(V) -> M,
) -> Element<'a, M>
where
    V: Copy + PartialEq,
    M: Clone + 'a,
{
    view(
        t,
        look,
        items
            .iter()
            .map(|&(value, content)| {
                Segment::new(content, selected == Some(value), on_select(value))
            })
            .collect(),
    )
}
