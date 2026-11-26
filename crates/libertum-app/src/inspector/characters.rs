//! The characters tab of the Notes pane: the characters saved for the story, a search box, and
//! Add. Each character can open the plot points they are linked to; they start closed.

use std::collections::HashSet;

use iced::widget::{button, column, container, row, space, text, text_input};
use iced::{Center, Element, Length, Padding};

use crate::app::{ImportMsg, Message};
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{self, divider};

use libertum_model::{Characters, Checklist};

/// The most characters drawn at once. A longer list asks the writer to search instead, which
/// keeps the pane quick with the hundreds of names an import can bring in.
pub const MAX_SHOWN: usize = 50;

/// What the tab draws. The app owns all of it; this only borrows.
pub struct CharactersView<'a> {
    pub cast: &'a Characters,
    /// The story checklist, for the text of each linked plot point.
    pub points: &'a Checklist,
    pub search: &'a str,
    /// The characters whose plot points are open.
    pub expanded: &'a HashSet<u64>,
}

pub fn body<'a>(view: CharactersView<'a>, t: Tokens) -> Element<'a, Message> {
    let pad = |top: f32, bottom: f32| Padding {
        top,
        bottom,
        left: 14.0,
        right: 14.0,
    };

    let add = button(text(s::CHARACTERS_ADD).size(12.0).color(t.on_accent))
        .padding([5, 12])
        .style(move |_, status| {
            let _ = status;
            button::Style {
                background: Some(t.accent.into()),
                text_color: t.on_accent,
                border: iced::border::rounded(6),
                ..button::Style::default()
            }
        })
        .on_press(Message::CharacterAdd);
    let search = text_input(s::CHARACTERS_SEARCH, view.search)
        .on_input(Message::CharactersSearch)
        .size(12)
        .padding(6)
        .width(Length::Fill);
    let import = button(text(s::CHARACTERS_IMPORT).size(12.0).color(t.accent))
        .padding([5, 8])
        .style(button::text)
        .on_press(Message::Import(ImportMsg::Open));
    let top = row![search, import, add].spacing(6).align_y(Center);

    let found = view.cast.search(view.search);
    let shown = found.len().min(MAX_SHOWN);

    let mut list = column![].spacing(12.0);
    for character in found.iter().take(shown) {
        let id = character.id;
        let open = view.expanded.contains(&id);

        let mut heading = row![
            text(character.name.as_str())
                .size(13.0)
                .font(controls::bold())
                .color(t.text)
        ]
        .spacing(8)
        .align_y(Center);
        if !character.age.is_empty() {
            heading = heading.push(
                text(character.age.as_str())
                    .size(11.0)
                    .color(t.text_secondary),
            );
        }
        let actions = row![
            button(text(s::CHARACTER_EDIT).size(11.0).color(t.accent))
                .padding(0)
                .style(button::text)
                .on_press(Message::CharacterEditOpen(id)),
            button(text(s::CHARACTER_REMOVE).size(11.0).color(t.text_secondary))
                .padding(0)
                .style(button::text)
                .on_press(Message::CharacterRemove(id)),
        ]
        .spacing(10);

        let mut entry = column![heading].spacing(3);
        if !character.details.is_empty() {
            entry = entry.push(
                text(character.details.as_str())
                    .size(11.5)
                    .color(t.text_secondary),
            );
        }

        let linked: Vec<&str> = character
            .plot_points
            .iter()
            .filter_map(|point| view.points.get(*point))
            .map(|item| item.text.as_str())
            .collect();
        let toggle_label = if open {
            s::CHARACTER_HIDE_POINTS.to_owned()
        } else {
            format!("{} ({})", s::CHARACTER_SHOW_POINTS, linked.len())
        };
        entry = entry.push(
            row![
                button(text(toggle_label).size(11.0).color(t.accent))
                    .padding(0)
                    .style(button::text)
                    .on_press(Message::CharacterToggle(id)),
                space::horizontal(),
                actions,
            ]
            .align_y(Center),
        );
        if open {
            let mut points = column![].spacing(2).padding(Padding {
                top: 2.0,
                bottom: 0.0,
                left: 10.0,
                right: 0.0,
            });
            if linked.is_empty() {
                points = points.push(
                    text(s::CHARACTER_NO_POINTS)
                        .size(11.0)
                        .color(t.text_secondary),
                );
            }
            for point in linked {
                points = points.push(text(format!("\u{2022} {point}")).size(11.0).color(t.text));
            }
            entry = entry.push(points);
        }
        list = list.push(entry);
    }

    let mut content = column![container(top).padding(pad(10.0, 8.0))];
    if found.len() > shown {
        content = content.push(
            container(
                text(format!(
                    "{} {shown} {} {}. {}",
                    s::CHARACTERS_SHOWING,
                    s::CHARACTERS_OF,
                    found.len(),
                    s::CHARACTERS_SEARCH_FOR_MORE
                ))
                .size(10.5)
                .color(t.text_secondary),
            )
            .padding(pad(0.0, 8.0)),
        );
    }
    content = content.push(divider(t));
    if found.is_empty() {
        let empty = if view.cast.items().is_empty() {
            s::CHARACTERS_EMPTY
        } else {
            s::CHARACTERS_NONE_FOUND
        };
        content = content.push(
            container(text(empty).size(12.0).color(t.text_secondary)).padding(pad(12.0, 12.0)),
        );
    } else {
        content = content.push(container(list).padding(pad(12.0, 14.0)));
    }
    content.into()
}
