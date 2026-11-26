//! Notes inspector: footnotes and endnotes (one list each, editable in place) and the story
//! checklist.

use iced::widget::{button, column, container, text, text_input};
use iced::{Element, Length, Padding};

use crate::app::Message;
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::divider;
use crate::widgets::segmented::{self, Content, Look};

use libertum_model::{BlockId, NoteKind};

use super::characters::{self, CharactersView};
use super::checklist::{self, ChecklistView};

/// Which list the Notes pane shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NotesTab {
    /// The story checklist: first, and what the pane opens on.
    #[default]
    Checklist,
    /// The characters saved for the story.
    Characters,
    Footnotes,
    Endnotes,
}

/// The tabs of the Notes pane, left to right.
pub const TAB_ORDER: [NotesTab; 4] = [
    NotesTab::Checklist,
    NotesTab::Characters,
    NotesTab::Footnotes,
    NotesTab::Endnotes,
];

fn tab_label(tab: NotesTab) -> &'static str {
    match tab {
        NotesTab::Checklist => s::NOTES_TAB_CHECKLIST,
        NotesTab::Characters => s::NOTES_TAB_CHARACTERS,
        NotesTab::Footnotes => s::NOTES_TAB_FOOTNOTES,
        NotesTab::Endnotes => s::NOTES_TAB_ENDNOTES,
    }
}

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

pub fn header<'a>(tab: NotesTab, t: Tokens) -> Element<'a, Message> {
    let items: Vec<(NotesTab, Content)> = TAB_ORDER
        .iter()
        .map(|tab| (*tab, Content::Label(tab_label(*tab))))
        .collect();
    let tabs = segmented::select(t, Look::Raised, &items, Some(tab), Message::NotesTab);
    column![
        container(column![text(s::PANE_NOTES).size(15.0).color(t.text), tabs].spacing(10.0))
            .padding(Padding {
                top: 14.0,
                bottom: 12.0,
                left: 14.0,
                right: 14.0,
            }),
        divider(t),
    ]
    .into()
}

/// Everything the Notes pane draws.
pub struct NotesPane<'a> {
    pub tab: NotesTab,
    pub entries: &'a [NoteEntry],
    pub checklist: ChecklistView<'a>,
    pub characters: CharactersView<'a>,
}

pub fn body<'a>(pane: NotesPane<'a>, t: Tokens) -> Element<'a, Message> {
    let kind = match pane.tab {
        NotesTab::Footnotes => NoteKind::Footnote,
        NotesTab::Endnotes => NoteKind::Endnote,
        NotesTab::Checklist => return checklist::body(pane.checklist, t),
        NotesTab::Characters => return characters::body(pane.characters, t),
    };
    let shown: Vec<&NoteEntry> = pane
        .entries
        .iter()
        .filter(|note| note.kind == kind)
        .collect();
    if shown.is_empty() {
        let empty = match kind {
            NoteKind::Footnote => s::FOOTNOTES_EMPTY,
            NoteKind::Endnote => s::ENDNOTES_EMPTY,
        };
        return container(text(empty).size(12.0).color(t.text_secondary))
            .padding(Padding {
                top: 14.0,
                bottom: 14.0,
                left: 14.0,
                right: 14.0,
            })
            .into();
    }
    let mut list = column![].spacing(6.0);
    for note in shown {
        let label = format!(
            "{} {}",
            match note.kind {
                NoteKind::Footnote => "Footnote",
                NoteKind::Endnote => "Endnote",
            },
            note.number
        );
        let (block, index) = (note.block, note.index);
        let input = text_input(s::FOOTNOTE_PLACEHOLDER, &note.text)
            .on_input(move |value| Message::NoteText(block, index, value))
            .size(12)
            .padding(6)
            .width(Length::Fill);
        list = list.push(
            column![
                button(text(label).size(11.0).color(t.text_secondary))
                    .padding(0)
                    .style(button::text)
                    .on_press(Message::GoToNote(block, index)),
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
