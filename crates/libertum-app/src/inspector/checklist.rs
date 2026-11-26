//! The story checklist tab of the Notes pane: the writer's points, ranked by how little of
//! their wording the open chapter uses (or how much).

use iced::widget::{button, column, container, mouse_area, row, space, text, text_input};
use iced::{Border, Center, Element, Length, Padding};

use crate::app::Message;
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{checkbox_row, divider};
use crate::widgets::segmented::{self, Content, Look};

use libertum_model::{Checklist, ChecklistOrder, Ranked, Scope};

/// What the tab draws. The app owns all of it; this only borrows.
pub struct ChecklistView<'a> {
    pub checklist: &'a Checklist,
    /// The items in the order to show them.
    pub rows: &'a [Ranked],
    /// The chapter being scored against, if the document has one.
    pub chapter: Option<&'a str>,
    pub order: ChecklistOrder,
    /// The text of the point being typed.
    pub draft: &'a str,
}

pub fn body<'a>(view: ChecklistView<'a>, t: Tokens) -> Element<'a, Message> {
    let pad = |top: f32, bottom: f32| Padding {
        top,
        bottom,
        left: 14.0,
        right: 14.0,
    };

    let order = segmented::select(
        t,
        Look::Raised,
        &[
            (
                ChecklistOrder::LeastCovered,
                Content::Label(s::CHECKLIST_ORDER_LEAST),
            ),
            (
                ChecklistOrder::MostRelevant,
                Content::Label(s::CHECKLIST_ORDER_MOST),
            ),
        ],
        Some(view.order),
        Message::ChecklistOrder,
    );

    let add = row![
        text_input(s::CHECKLIST_ADD_PLACEHOLDER, view.draft)
            .on_input(Message::ChecklistDraft)
            .on_submit(Message::ChecklistAdd)
            .size(12)
            .padding(6)
            .width(Length::Fill),
        button(text(s::CHECKLIST_ADD).size(12.0).color(t.accent))
            .padding(Padding {
                top: 5.0,
                bottom: 5.0,
                left: 8.0,
                right: 8.0,
            })
            .style(button::text)
            .on_press_maybe((!view.draft.trim().is_empty()).then_some(Message::ChecklistAdd)),
    ]
    .spacing(4)
    .align_y(Center);

    let mut list = column![].spacing(10.0);
    for ranked in view.rows {
        let Some(item) = view.checklist.get(ranked.id) else {
            continue;
        };
        let id = item.id;

        let tick = checkbox_row(t, &item.text, ranked.done, false, move |done| {
            Message::ChecklistDone(id, done)
        });
        let score = match ranked.score.coverage {
            Some(coverage) => {
                let percent = (coverage * 100.0).round().clamp(0.0, 100.0) as u16;
                let filled = container(space().height(Length::Fixed(4.0)))
                    .width(Length::FillPortion(percent.max(1)))
                    .style(move |_| container::background(t.accent));
                let rest = container(space().height(Length::Fixed(4.0)))
                    .width(Length::FillPortion(101 - percent.max(1)))
                    .style(move |_| container::background(t.segment_track));
                let bar = container(row![filled, rest])
                    .width(Length::Fill)
                    .style(move |_| container::Style {
                        border: Border {
                            radius: 2.0.into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    })
                    .clip(true);
                column![
                    bar,
                    text(format!(
                        "{percent}% {} \u{00b7} {} {}",
                        s::CHECKLIST_COVERAGE,
                        ranked.score.mentions,
                        s::CHECKLIST_MENTIONS
                    ))
                    .size(10.5)
                    .color(t.text_secondary),
                ]
                .spacing(3)
            }
            None => column![
                text(s::CHECKLIST_NO_KEYWORDS)
                    .size(10.5)
                    .color(t.text_secondary)
            ],
        };

        let mut entry = column![tick, score].spacing(4);
        let scope_label = match &item.scope {
            Scope::Story => None,
            Scope::Chapters { chapters, onward } => Some(match (chapters.len(), onward) {
                (1, false) => s::CHECKLIST_SCOPE_ONLY_HERE,
                (_, false) => s::CHECKLIST_SCOPE_SOME,
                (_, true) => s::CHECKLIST_SCOPE_ONWARD,
            }),
        };
        if let Some(label) = scope_label {
            entry = entry.push(text(label).size(10.5).color(t.text_secondary));
        }
        // Right-click opens a menu with Edit and Delete.
        list = list.push(mouse_area(entry).on_right_press(Message::ChecklistContext(id)));
    }

    let mut content = column![container(order).padding(pad(10.0, 8.0))];
    // Just the chapter's title: it is the chapter the points are scored against.
    if let Some(title) = view.chapter {
        content = content
            .push(container(text(title).size(11.0).color(t.text_secondary)).padding(pad(0.0, 8.0)));
    }
    content = content.push(container(add).padding(pad(0.0, 10.0)));
    content = content.push(divider(t));
    if view.rows.is_empty() {
        content = content.push(
            container(text(s::CHECKLIST_EMPTY).size(12.0).color(t.text_secondary))
                .padding(pad(12.0, 12.0)),
        );
    } else {
        content = content.push(container(list).padding(pad(12.0, 14.0)));
    }
    content.into()
}
