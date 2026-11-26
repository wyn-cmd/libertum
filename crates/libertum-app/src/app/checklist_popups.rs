//! The checklist's pop-ups: which chapters a new point is for, and editing a point.

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, space, stack, text, text_input,
};
use iced::{Element, Length, border};

use libertum_model::{ChapterRef, Scope};

use super::{App, Message};
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{self, checkbox_row, flat};
use crate::widgets::segmented::{self, Content, Look};

/// What the writer is choosing between.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScopeKind {
    /// Only the chapter being written.
    ThisChapter,
    /// Every chapter of the story.
    #[default]
    Story,
    /// Chapters picked from the list.
    Chapters,
}

/// A point waiting for its scope.
#[derive(Clone, Debug)]
pub struct ScopeDraft {
    pub text: String,
    pub kind: ScopeKind,
    /// Every chapter of the document, in order.
    pub chapters: Vec<ChapterRef>,
    /// Which of them the caret is in.
    pub current: usize,
    /// One flag per chapter: ticked in the list.
    pub ticked: Vec<bool>,
    /// Also every chapter after the last ticked one.
    pub onward: bool,
}

impl ScopeDraft {
    pub fn new(text: String, chapters: Vec<ChapterRef>, current: usize) -> Self {
        let mut ticked = vec![false; chapters.len()];
        if let Some(flag) = ticked.get_mut(current) {
            *flag = true;
        }
        Self {
            text,
            kind: ScopeKind::default(),
            chapters,
            current,
            ticked,
            onward: false,
        }
    }

    /// The scope this choice makes, or `None` while it names no chapter.
    pub fn scope(&self) -> Option<Scope> {
        match self.kind {
            ScopeKind::Story => Some(Scope::Story),
            ScopeKind::ThisChapter => self.chapters.get(self.current).cloned().map(Scope::only),
            ScopeKind::Chapters => {
                let chapters: Vec<ChapterRef> = self
                    .chapters
                    .iter()
                    .zip(&self.ticked)
                    .filter(|(_, ticked)| **ticked)
                    .map(|(chapter, _)| chapter.clone())
                    .collect();
                (!chapters.is_empty()).then_some(Scope::Chapters {
                    chapters,
                    onward: self.onward,
                })
            }
        }
    }
}

impl App {
    pub(super) fn checklist_scope_layer<'a>(
        &'a self,
        draft: &'a ScopeDraft,
        t: Tokens,
    ) -> Element<'a, Message> {
        let kinds = segmented::select(
            t,
            Look::Raised,
            &[
                (
                    ScopeKind::ThisChapter,
                    Content::Label(s::SCOPE_THIS_CHAPTER),
                ),
                (ScopeKind::Story, Content::Label(s::SCOPE_STORY)),
                (ScopeKind::Chapters, Content::Label(s::SCOPE_CHAPTERS)),
            ],
            Some(draft.kind),
            Message::ChecklistScopeKind,
        );

        let explanation = match draft.kind {
            ScopeKind::ThisChapter => format!(
                "{} {}",
                s::SCOPE_THIS_CHAPTER_HINT,
                draft
                    .chapters
                    .get(draft.current)
                    .map_or("", |chapter| chapter.title.as_str())
            ),
            ScopeKind::Story => s::SCOPE_STORY_HINT.to_owned(),
            ScopeKind::Chapters => s::SCOPE_CHAPTERS_HINT.to_owned(),
        };

        let mut body = column![
            text(draft.text.as_str()).size(13).color(t.text),
            kinds,
            text(explanation).size(12).color(t.text_secondary),
        ]
        .spacing(10);

        if draft.kind == ScopeKind::Chapters {
            let mut list = column![].spacing(6);
            for (index, chapter) in draft.chapters.iter().enumerate() {
                list = list.push(checkbox_row(
                    t,
                    chapter.title.as_str(),
                    draft.ticked[index],
                    false,
                    move |_| Message::ChecklistScopeToggle(index),
                ));
            }
            body = body.push(scrollable(list).height(Length::Shrink).spacing(4));
            body = body.push(checkbox_row(
                t,
                s::SCOPE_ONWARD,
                draft.onward,
                false,
                Message::ChecklistScopeOnward,
            ));
        }

        let cancel = button(text(s::CANCEL).size(13))
            .padding([6, 16])
            .style(move |_, status| flat(t, status))
            .on_press(Message::ChecklistScopeCancel);
        let add = button(text(s::CHECKLIST_ADD_POINT).size(13))
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
            .on_press_maybe(draft.scope().map(|_| Message::ChecklistScopeConfirm));

        let card = container(
            column![
                text(s::SCOPE_TITLE)
                    .size(15)
                    .font(controls::bold())
                    .color(t.text),
                body,
                row![space::horizontal(), cancel, add].spacing(8),
            ]
            .spacing(14),
        )
        .padding(18)
        .width(Length::Fixed(420.0))
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
                .on_press(Message::ChecklistScopeCancel),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}

/// A point being edited in its pop-up. Nothing changes until Save.
#[derive(Clone, Debug)]
pub struct EditDraft {
    pub id: u64,
    pub text: String,
    pub aliases: String,
}

impl EditDraft {
    /// Whether the point can be saved: it needs some text.
    pub fn valid(&self) -> bool {
        !self.text.trim().is_empty()
    }
}

impl App {
    pub(super) fn checklist_edit_layer<'a>(
        &'a self,
        draft: &'a EditDraft,
        t: Tokens,
    ) -> Element<'a, Message> {
        let save_message = draft.valid().then_some(Message::ChecklistEditSave);
        let mut point = text_input(s::CHECKLIST_ADD_PLACEHOLDER, &draft.text)
            .on_input(Message::ChecklistEditText)
            .size(13)
            .padding(8);
        let mut aliases = text_input(s::CHECKLIST_ALIASES_PLACEHOLDER, &draft.aliases)
            .on_input(Message::ChecklistEditAliases)
            .size(13)
            .padding(8);
        if let Some(message) = save_message.clone() {
            point = point.on_submit(message.clone());
            aliases = aliases.on_submit(message);
        }
        let cancel = button(text(s::CANCEL).size(13))
            .padding([6, 16])
            .style(move |_, status| flat(t, status))
            .on_press(Message::ChecklistEditCancel);
        let save = button(text(s::CHECKLIST_SAVE).size(13))
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
        let card = container(
            column![
                text(s::CHECKLIST_EDIT_TITLE)
                    .size(15)
                    .font(controls::bold())
                    .color(t.text),
                column![point, aliases].spacing(8),
                row![space::horizontal(), cancel, save].spacing(8),
            ]
            .spacing(14),
        )
        .padding(18)
        .width(Length::Fixed(420.0))
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
                .on_press(Message::ChecklistEditCancel),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}
