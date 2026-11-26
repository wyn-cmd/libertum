//! The pop-up for adding or editing one character.

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, stack, text, text_input,
};
use iced::{Element, Length, border};

use libertum_model::{CharacterError, Characters};

use super::{App, Message};
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{self, checkbox_row, flat};

/// A character being written in its pop-up. Nothing is saved until Save.
#[derive(Clone, Debug)]
pub struct CharacterDraft {
    /// The character being edited, or `None` for a new one.
    pub id: Option<u64>,
    pub name: String,
    pub age: String,
    pub details: String,
    /// Plot points this character is linked to.
    pub linked: Vec<u64>,
    /// Every plot point (checklist item id and text) the writer can link to.
    pub points: Vec<(u64, String)>,
}

impl CharacterDraft {
    pub fn new(points: Vec<(u64, String)>) -> Self {
        Self {
            id: None,
            name: String::new(),
            age: String::new(),
            details: String::new(),
            linked: Vec::new(),
            points,
        }
    }

    /// Whether this draft could be saved into `cast` now.
    pub fn check(&self, cast: &Characters) -> Result<(), CharacterError> {
        if self.name.trim().is_empty() {
            Err(CharacterError::NoName)
        } else if cast.has_name(&self.name, self.id) {
            Err(CharacterError::Duplicate)
        } else {
            Ok(())
        }
    }

    pub fn toggle(&mut self, point: u64) {
        if let Some(position) = self.linked.iter().position(|p| *p == point) {
            self.linked.remove(position);
        } else {
            self.linked.push(point);
        }
    }
}

impl App {
    pub(super) fn character_edit_layer<'a>(
        &'a self,
        draft: &'a CharacterDraft,
        t: Tokens,
    ) -> Element<'a, Message> {
        let state = draft.check(&self.characters.list);
        let save_message = state.is_ok().then_some(Message::CharacterSave);
        let mut name = text_input(s::CHARACTER_NAME_PLACEHOLDER, &draft.name)
            .on_input(Message::CharacterName)
            .size(13)
            .padding(8);
        let mut age = text_input(s::CHARACTER_AGE_PLACEHOLDER, &draft.age)
            .on_input(Message::CharacterAge)
            .size(13)
            .padding(8);
        let mut details = text_input(s::CHARACTER_DETAILS_PLACEHOLDER, &draft.details)
            .on_input(Message::CharacterDetails)
            .size(13)
            .padding(8);
        if let Some(message) = save_message.clone() {
            name = name.on_submit(message.clone());
            age = age.on_submit(message.clone());
            details = details.on_submit(message);
        }

        let mut fields = column![name, age, details].spacing(8);
        if state == Err(CharacterError::Duplicate) {
            fields = fields.push(
                text(s::CHARACTER_DUPLICATE)
                    .size(11.5)
                    .color(t.text_secondary),
            );
        }

        let mut points = column![].spacing(6);
        for (id, label) in &draft.points {
            let id = *id;
            points = points.push(checkbox_row(
                t,
                label.as_str(),
                draft.linked.contains(&id),
                false,
                move |_| Message::CharacterPoint(id),
            ));
        }
        let linking = if draft.points.is_empty() {
            column![
                text(s::CHARACTER_NO_PLOT_POINTS)
                    .size(12)
                    .color(t.text_secondary)
            ]
        } else {
            column![
                text(s::CHARACTER_LINK_HINT)
                    .size(12)
                    .color(t.text_secondary),
                scrollable(points).height(Length::Fixed(140.0)).spacing(4),
            ]
            .spacing(8)
        };

        let cancel = button(text(s::CANCEL).size(13))
            .padding([6, 16])
            .style(move |_, status| flat(t, status))
            .on_press(Message::CharacterCancel);
        let save = button(text(s::CHARACTER_SAVE).size(13))
            .padding([6, 18])
            .style(move |_, status| {
                let on = !matches!(status, button::Status::Disabled);
                button::Style {
                    background: Some(if on { t.accent } else { t.control }.into()),
                    text_color: if on { t.on_accent } else { t.text_secondary },
                    border: border::rounded(6),
                    ..button::Style::default()
                }
            })
            .on_press_maybe(save_message);

        let title = if draft.id.is_some() {
            s::CHARACTER_EDIT_TITLE
        } else {
            s::CHARACTER_ADD_TITLE
        };
        let card = container(
            column![
                text(title).size(15).font(controls::bold()).color(t.text),
                fields,
                text(s::CHARACTER_PLOT_POINTS)
                    .size(13)
                    .font(controls::bold())
                    .color(t.text),
                linking,
                row![space::horizontal(), cancel, save].spacing(8),
            ]
            .spacing(14),
        )
        .padding(18)
        .width(Length::Fixed(440.0))
        .style(move |_| container::Style {
            background: Some(t.inspector.into()),
            border: border::rounded(12),
            shadow: iced::Shadow {
                color: t.shadow,
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..container::Style::default()
        });
        stack![
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::CharacterCancel),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}
