//! The pop-up for Insert > Music: the track's title, link and credit, and whether to keep a copy
//! of the audio inside the document.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use iced::widget::{button, column, container, mouse_area, row, space, stack, text, text_input};
use iced::{Element, Length, border};

use libertum_model::{AudioCopy, Soundtrack, TrackError};

use super::{App, Message};
use crate::strings as s;
use crate::theme::Tokens;
use crate::widgets::controls::{self, checkbox_row, flat};
use crate::wiki_fetch::AudioFetchError;

/// Where the copy stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyState {
    Idle,
    Fetching,
    /// Why the last fetch gave nothing, in words.
    Failed(String),
}

/// A track being written in its pop-up. Nothing is inserted until Insert.
#[derive(Clone, Debug)]
pub struct MusicDraft {
    pub title: String,
    pub url: String,
    pub credit: String,
    /// The writer says they may keep a copy.
    pub permission: bool,
    pub copy: Option<AudioCopy>,
    /// The link `copy` was made from; a copy of another link is not kept.
    pub copy_url: String,
    pub state: CopyState,
    pub cancel: Arc<AtomicBool>,
}

impl MusicDraft {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            url: String::new(),
            credit: String::new(),
            permission: false,
            copy: None,
            copy_url: String::new(),
            state: CopyState::Idle,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The copy, if it was made from the link as it stands now.
    pub fn current_copy(&self) -> Option<&AudioCopy> {
        self.copy
            .as_ref()
            .filter(|_| self.copy_url == self.url.trim())
    }

    /// Whether the draft could be inserted now.
    pub fn check(&self) -> Result<(), TrackError> {
        Soundtrack::validate(&self.title, &self.url, self.current_copy().is_some())
    }

    pub fn can_save(&self) -> bool {
        self.check().is_ok() && self.state != CopyState::Fetching
    }

    pub fn can_fetch(&self) -> bool {
        self.permission
            && self.url.trim().starts_with("https://")
            && self.state != CopyState::Fetching
    }
}

impl Default for MusicDraft {
    fn default() -> Self {
        Self::new()
    }
}

/// A fetch's failure, in words.
pub fn fetch_failure(error: &AudioFetchError) -> String {
    match error {
        AudioFetchError::NotHttps => s::MUSIC_FETCH_NOT_HTTPS.to_owned(),
        AudioFetchError::Robots(host) => format!("{} {host}", s::MUSIC_FETCH_ROBOTS),
        AudioFetchError::Refused(code) => format!("{} {code}.", s::MUSIC_FETCH_REFUSED),
        AudioFetchError::TooLarge => s::MUSIC_FETCH_TOO_LARGE.to_owned(),
        AudioFetchError::NotAudio => s::MUSIC_FETCH_NOT_AUDIO.to_owned(),
        AudioFetchError::Stopped => s::MUSIC_FETCH_STOPPED.to_owned(),
        AudioFetchError::Network(why) => format!("{} {why}", s::MUSIC_FETCH_NETWORK),
    }
}

impl App {
    pub(super) fn music_edit_layer<'a>(
        &'a self,
        draft: &'a MusicDraft,
        t: Tokens,
    ) -> Element<'a, Message> {
        let save_message = draft.can_save().then_some(Message::MusicSave);
        let mut title = text_input(s::MUSIC_TITLE_PLACEHOLDER, &draft.title)
            .on_input(Message::MusicTitle)
            .size(13)
            .padding(8);
        let mut url = text_input(s::MUSIC_URL_PLACEHOLDER, &draft.url)
            .on_input(Message::MusicUrl)
            .size(13)
            .padding(8);
        let mut credit = text_input(s::MUSIC_CREDIT_PLACEHOLDER, &draft.credit)
            .on_input(Message::MusicCredit)
            .size(13)
            .padding(8);
        if let Some(message) = save_message.clone() {
            title = title.on_submit(message.clone());
            url = url.on_submit(message.clone());
            credit = credit.on_submit(message);
        }
        let mut fields = column![title, url, credit].spacing(8);
        if draft.check() == Err(TrackError::NotHttps) {
            fields = fields.push(text(s::MUSIC_NOT_HTTPS).size(11.5).color(t.text_secondary));
        }

        let mut copy = column![
            text(s::MUSIC_COPY_HINT).size(11.5).color(t.text_secondary),
            checkbox_row(t, s::MUSIC_PERMISSION, draft.permission, false, |_| {
                Message::MusicPermission
            }),
        ]
        .spacing(8);
        let fetching = draft.state == CopyState::Fetching;
        let action = if fetching {
            button(text(s::MUSIC_STOP).size(12))
                .padding([5, 14])
                .style(move |_, status| flat(t, status))
                .on_press(Message::MusicStop)
        } else {
            button(
                text(if draft.current_copy().is_some() {
                    s::MUSIC_DROP_COPY
                } else {
                    s::MUSIC_KEEP
                })
                .size(12),
            )
            .padding([5, 14])
            .style(move |_, status| flat(t, status))
            .on_press_maybe(if draft.current_copy().is_some() {
                Some(Message::MusicDropCopy)
            } else {
                draft.can_fetch().then_some(Message::MusicKeep)
            })
        };
        let status = if fetching {
            Some(s::MUSIC_FETCHING.to_owned())
        } else if draft.current_copy().is_some() {
            Some(s::MUSIC_COPY_READY.to_owned())
        } else if let CopyState::Failed(why) = &draft.state {
            Some(why.clone())
        } else {
            None
        };
        let mut copy_row = row![action].spacing(10).align_y(iced::Center);
        if let Some(status) = status {
            copy_row = copy_row.push(text(status).size(11.5).color(t.text_secondary));
        }
        copy = copy.push(copy_row);

        let cancel = button(text(s::CANCEL).size(13))
            .padding([6, 16])
            .style(move |_, status| flat(t, status))
            .on_press(Message::MusicCancel);
        let save = button(text(s::MUSIC_SAVE).size(13))
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
                text(s::MUSIC_ADD_TITLE)
                    .size(15)
                    .font(controls::bold())
                    .color(t.text),
                text(s::MUSIC_PLAYER_HINT)
                    .size(11.5)
                    .color(t.text_secondary),
                fields,
                text(s::MUSIC_COPY_HEADING)
                    .size(13)
                    .font(controls::bold())
                    .color(t.text),
                copy,
                row![space::horizontal(), cancel, save].spacing(8),
            ]
            .spacing(14),
        )
        .padding(18)
        .width(Length::Fixed(460.0))
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
                .on_press(Message::MusicCancel),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }
}
