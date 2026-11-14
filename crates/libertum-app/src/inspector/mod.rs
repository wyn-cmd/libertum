//! The right-hand inspector: Format (Style, Layout, More) or Document (Document, Section).

pub mod bookmarks;
pub mod document;
pub mod format;
pub mod headers;
pub mod notes;

use bookmarks::BookmarkEntry;
use headers::HeadingEntry;
use notes::NoteEntry;

use iced::widget::{column, container, row, scrollable, space};
use iced::{Border, Element, Length};

use crate::app::Message;
use crate::state::{AppState, Field, Inspector};
use crate::theme::Tokens;
use crate::widgets::stepper;

/// The sidebar as it should be drawn right now, or `None` once it is fully hidden.
///
/// Its outer width follows the slide animation while the content keeps its full width and is
/// clipped, so nothing reflows during the animation.
pub fn view<'a>(
    state: &'a AppState,
    headings: &'a [HeadingEntry],
    collapsed_headings: &'a std::collections::HashSet<libertum_model::BlockId>,
    notes: &'a [NoteEntry],
    bookmarks: &'a [BookmarkEntry],
    draft: Option<(libertum_model::BlockId, usize, &'a str)>,
    t: Tokens,
) -> Option<Element<'a, Message>> {
    let shown = state.sidebar_shown_width();
    if shown < 0.5 {
        return None;
    }
    let (header, body) = match state.inspector {
        Inspector::Format => (format::header(state, t), format::body(state, t)),
        Inspector::Document => (document::header(state, t), document::body(state, t)),
        Inspector::Headers => (
            headers::header(t),
            headers::body(headings, collapsed_headings, t),
        ),
        Inspector::Notes => (notes::header(t), notes::body(notes, t)),
        Inspector::Bookmarks => (bookmarks::header(t), bookmarks::body(bookmarks, draft, t)),
    };
    let scroll = scrollable(body)
        .height(Length::Fill)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(6)
                .scroller_width(6)
                .margin(2),
        ))
        .style(move |_, status| {
            let hot = !matches!(status, scrollable::Status::Active { .. });
            let scroller = scrollable::Scroller {
                background: iced::Color {
                    a: if hot { 0.45 } else { 0.28 },
                    ..t.text_secondary
                }
                .into(),
                border: Border {
                    radius: 3.0.into(),
                    ..Border::default()
                },
            };
            let rail = scrollable::Rail {
                background: None,
                border: Border::default(),
                scroller,
            };
            scrollable::Style {
                container: container::Style::default(),
                vertical_rail: rail,
                horizontal_rail: rail,
                gap: None,
                auto_scroll: scrollable::AutoScroll {
                    background: t.menu.into(),
                    border: Border::default(),
                    shadow: iced::Shadow::default(),
                    icon: t.text,
                },
            }
        });

    let content = column![header, scroll]
        .width(Length::Fixed(state.sidebar_width - 1.0))
        .height(Length::Fill);
    let edge = container(space().width(Length::Fixed(1.0)).height(Length::Fill)).style(move |_| {
        container::Style {
            background: Some(t.chrome_border.into()),
            ..container::Style::default()
        }
    });
    Some(
        container(row![edge, content])
            .width(Length::Fixed(shown))
            .height(Length::Fill)
            .clip(true)
            .style(move |_| container::background(t.inspector))
            .into(),
    )
}

/// A stepper bound to an inspector field.
pub fn field<'a>(state: &'a AppState, t: Tokens, field: Field) -> Element<'a, Message> {
    let editing = state
        .editing
        .as_ref()
        .filter(|(f, _)| *f == field)
        .map(|(_, text)| text.as_str());
    stepper::view(
        t,
        field.spec(state.units),
        field.get(state),
        field.unit_label(state.units),
        editing,
        move |text| Message::FieldText(field, text),
        Message::FieldCommit,
        move |direction| Message::Step(field, direction),
    )
}
