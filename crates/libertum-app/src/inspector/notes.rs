//! Notes inspector: every footnote and endnote, editable in place.

use iced::widget::{button, column, container, text, text_input};
use iced::{Element, Length, Padding};

use crate::app::Message;
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::divider;

use libertum_model::{BlockId, NoteKind};

/// One note row. `index` is the note's position in its paragraph's note list.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteEntry {
    pub block: BlockId,
    pub index: usize,
    pub offset: usize,
    pub kind: NoteKind,
    pub number: u32,
    pub text: String,
}

pub fn header<'a>(t: Tokens) -> Element<'a, Message> {
    column![
        container(text(s::PANE_NOTES).size(15.0).color(t.text)).padding(Padding {
            top: 14.0,
            bottom: 12.0,
            left: 14.0,
            right: 14.0,
        }),
        divider(t),
    ]
    .into()
}

pub fn body<'a>(notes: &'a [NoteEntry], t: Tokens) -> Element<'a, Message> {
    if notes.is_empty() {
        return container(text(s::NOTES_EMPTY).size(12.0).color(t.text_secondary))
            .padding(Padding {
                top: 14.0,
                bottom: 14.0,
                left: 14.0,
                right: 14.0,
            })
            .into();
    }
    let mut list = column![].spacing(6.0);
    for note in notes {
        let kind = match note.kind {
            NoteKind::Footnote => "Footnote",
            NoteKind::Endnote => "Endnote",
        };
        let label = format!("{kind} {}", note.number);
        let input = text_input(s::FOOTNOTE_PLACEHOLDER, &note.text)
            .on_input(move |value| Message::NoteText(note.block, note.index, value))
            .size(12)
            .padding(6)
            .width(Length::Fill);
        list = list.push(
            column![
                button(text(label).size(11.0).color(t.text_secondary))
                    .padding(0)
                    .style(button::text)
                    .on_press(Message::GoToNote(note.block, note.index)),
                input,
            ]
            .spacing(3),
        );
    }
    column![
        container(text(s::NOTES_HINT).size(11.0).color(t.text_secondary)).padding(Padding {
            top: 10.0,
            bottom: 8.0,
            left: 14.0,
            right: 14.0,
        }),
        container(list).padding(Padding {
            top: 0.0,
            bottom: 12.0,
            left: 14.0,
            right: 14.0,
        }),
    ]
    .into()
}
