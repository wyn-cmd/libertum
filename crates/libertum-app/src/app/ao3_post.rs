//! Mark as Posted in the AO3 chapter window: keeps the chapter's text as its "last posted"
//! copy for Compare with Posted. Libertum never connects to AO3; the writer copies the HTML,
//! title and tags from the chapter window and pastes them into AO3 themselves.

use std::time::SystemTime;

use iced::Task;
use libertum_library::ChapterKey;

use super::{App, Message, Modal};
use crate::strings as s;

/// What the chapter window's Mark as Posted button asks for.
#[derive(Debug, Clone)]
pub enum Ao3PostMsg {
    /// The writer posted: keep this chapter as its "last posted" copy.
    MarkPosted,
}

impl App {
    pub(super) fn update_ao3_post(&mut self, msg: Ao3PostMsg) -> Task<Message> {
        match msg {
            Ao3PostMsg::MarkPosted => {
                self.ao3_post_notice = Some(self.mark_ao3_chapter_posted());
                Task::none()
            }
        }
    }

    /// Keeps the chapter the window shows as its "last posted" copy (for Compare with
    /// Posted). Returns the notice to show.
    fn mark_ao3_chapter_posted(&self) -> String {
        let (Some(Modal::Ao3Chapter { chapter, title, .. }), Some(path), Some(doc)) =
            (&self.modal, self.file.as_ref(), self.doc.as_ref())
        else {
            return s::AO3_POST_SAVE_FIRST.to_owned();
        };
        let Some(range) = self
            .state
            .ao3_chapters
            .get(*chapter)
            .map(|c| c.start..c.end)
        else {
            return s::AO3_POST_SAVE_FIRST.to_owned();
        };
        let texts = libertum_diff::posted_texts(doc.document(), range);
        let key = ChapterKey::new(*chapter, title.clone());
        match libertum_library::save_posted(path, &key, &texts, SystemTime::now()) {
            Ok(_) => s::compare_marked(title),
            Err(error) => format!("{} {error}", s::COMPARE_MARK_FAILED),
        }
    }
}
