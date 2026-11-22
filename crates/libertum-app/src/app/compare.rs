//! Compare mode (AO3 plan, stage B): the editor turns read-only and marks what changed in
//! each chapter since the writer last marked it as posted.
//!
//! - A chapter's "last posted" copy is plain text kept in the document's history folder
//!   (`libertum_library::save_posted`). Nothing is uploaded anywhere.
//! - Comparing runs on a task, off the UI thread, when the mode starts and after every
//!   change it makes; never on the keystroke path (typing is blocked while it is on).
//! - Each change can be kept (its mark is dismissed) or reverted (the posted text is put
//!   back as one undoable edit). Front matter before the first chapter heading belongs to
//!   no chapter and is not compared.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use iced::Task;
use libertum_diff::{Hunk, HunkKind, Revert};
use libertum_layout::SelectionRect;
use libertum_library::ChapterKey;
use libertum_model::{BlockId, Document};

use super::{App, Message, Modal};
use crate::strings as s;

/// What the compare bar asks for.
#[derive(Debug, Clone)]
pub enum CompareMsg {
    /// View > Compare with Posted: start or stop.
    Toggle,
    /// A comparison finished (its generation, so a stale one is ignored).
    Computed(u64, Arc<Computed>),
    Next,
    Previous,
    /// Dismiss the current change's mark.
    Keep,
    /// Put the posted text back for the current change.
    Revert,
    /// Record the chapter at the caret (or of the current change) as just posted.
    MarkPosted,
    Exit,
}

/// One chapter as the comparison saw it.
#[derive(Clone, Debug)]
pub struct ChapterInfo {
    pub key: ChapterKey,
    /// Unix seconds of the posted copy, or `None` when the chapter was never marked.
    pub posted_at: Option<u64>,
}

/// A change and the chapter it is in.
#[derive(Clone, Debug)]
pub struct Located {
    pub chapter: usize,
    pub hunk: Hunk,
    /// Identifies the change by content, so a kept change stays kept across comparisons.
    pub key: u64,
}

/// The result of comparing every chapter with its posted copy.
#[derive(Debug, Default)]
pub struct Computed {
    pub chapters: Vec<ChapterInfo>,
    pub hunks: Vec<Located>,
    /// A chapter too large to compare, as a user-facing message.
    pub error: Option<String>,
}

/// Compare mode's state while it is on.
#[derive(Default)]
pub struct CompareMode {
    generation: u64,
    computing: bool,
    result: Option<Arc<Computed>>,
    kept: HashSet<u64>,
    /// Index into [`visible`](Self::visible).
    current: Option<usize>,
    pub notice: Option<String>,
    /// Mark rectangles for a page-layout generation (added, removed).
    rects: Mutex<Option<(u64, u64, RectSet)>>,
}

type RectSet = (Vec<SelectionRect>, Vec<SelectionRect>);

impl CompareMode {
    /// The changes not kept, in document order.
    pub fn visible(&self) -> Vec<&Located> {
        self.result
            .as_ref()
            .map(|result| {
                result
                    .hunks
                    .iter()
                    .filter(|located| !self.kept.contains(&located.key))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn current_hunk(&self) -> Option<Located> {
        let index = self.current?;
        self.visible().get(index).map(|located| (*located).clone())
    }

    /// The text the compare bar shows.
    pub fn summary(&self) -> String {
        if self.computing && self.result.is_none() {
            return s::COMPARE_WORKING.to_owned();
        }
        if let Some(notice) = &self.notice {
            return notice.clone();
        }
        let Some(result) = &self.result else {
            return s::COMPARE_WORKING.to_owned();
        };
        if let Some(error) = &result.error {
            return error.clone();
        }
        let posted = result
            .chapters
            .iter()
            .filter(|c| c.posted_at.is_some())
            .count();
        if posted == 0 {
            return s::COMPARE_NONE_POSTED.to_owned();
        }
        let visible = self.visible();
        match (self.current, visible.len()) {
            (_, 0) => s::compare_no_changes(posted),
            (Some(index), total) => {
                let located = visible[index.min(total - 1)];
                let title = &result.chapters[located.chapter].key.title;
                let mut line = s::compare_position(index + 1, total, title);
                let removed: Vec<&str> =
                    located.hunk.old_texts.iter().map(String::as_str).collect();
                if located.hunk.kind != HunkKind::Added && !removed.is_empty() {
                    line.push_str(" \u{b7} ");
                    line.push_str(&s::compare_posted_text(&removed.join(" / ")));
                }
                line
            }
            (None, total) => s::compare_count(total, posted),
        }
    }

    pub fn has_current(&self) -> bool {
        self.current.is_some()
    }
}

/// Compares every chapter of `document` that has a posted copy beside `path`.
fn compute(
    document: &Document,
    levels: libertum_export::ChapterLevels,
    path: &std::path::Path,
) -> Computed {
    let mut out = Computed::default();
    for (index, chapter) in libertum_export::chapters(document, levels)
        .into_iter()
        .enumerate()
    {
        let key = ChapterKey::new(index, chapter.title.clone());
        let posted = libertum_library::load_posted(path, &key);
        out.chapters.push(ChapterInfo {
            key: key.clone(),
            posted_at: posted.as_ref().map(|copy| copy.saved_at),
        });
        let Some(posted) = posted else {
            continue;
        };
        let current = libertum_diff::paragraphs(document, chapter.start..chapter.end);
        match libertum_diff::compare(&posted.paragraphs, &current) {
            Ok(comparison) => {
                let chapter_index = out.chapters.len() - 1;
                for hunk in comparison.hunks {
                    let key = hunk_key(&key.title, &hunk, document);
                    out.hunks.push(Located {
                        chapter: chapter_index,
                        hunk,
                        key,
                    });
                }
            }
            Err(error) => {
                out.error = Some(s::compare_too_large(&key.title, &error.to_string()));
            }
        }
    }
    out
}

/// A change's identity: its chapter, the posted text and the current text.
fn hunk_key(title: &str, hunk: &Hunk, document: &Document) -> u64 {
    let mut h = DefaultHasher::new();
    title.hash(&mut h);
    hunk.old_texts.hash(&mut h);
    for id in &hunk.new_ids {
        document.paragraph(*id).map(|p| p.text()).hash(&mut h);
    }
    h.finish()
}

impl App {
    /// Runs the comparison now, on this thread: tests (which cannot run the task) and the
    /// `LIBERTUM_COMPARE=next` screenshot hook.
    pub(super) fn compare_now(&mut self) {
        let (Some(doc), Some(path), Some(mode)) =
            (self.doc.as_ref(), self.file.clone(), self.compare.as_ref())
        else {
            return;
        };
        let result = compute(doc.document(), self.settings.chapter_headings, &path);
        let generation = mode.generation;
        let _ = self.update_compare(CompareMsg::Computed(generation, Arc::new(result)));
    }

    /// The changes compare mode shows now (tests).
    #[cfg(test)]
    pub(super) fn compare_visible(&self) -> usize {
        self.compare.as_ref().map_or(0, |mode| mode.visible().len())
    }

    pub(super) fn update_compare(&mut self, msg: CompareMsg) -> Task<Message> {
        match msg {
            CompareMsg::Toggle => {
                self.state.open_menu = None;
                if self.compare_on() {
                    self.exit_compare();
                    return Task::none();
                }
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                if self.file.is_none() {
                    self.modal = Some(Modal::Error(s::COMPARE_NO_FILE.to_owned()));
                    return Task::none();
                }
                doc.read_only = true;
                self.state.compare = true;
                self.compare = Some(CompareMode::default());
                if self.state.find.open {
                    self.close_find();
                }
                self.recompare()
            }
            CompareMsg::Computed(generation, result) => {
                let on = self.compare_on();
                let Some(mode) = self.compare.as_mut() else {
                    return Task::none();
                };
                if !on || generation != mode.generation {
                    return Task::none();
                }
                mode.computing = false;
                mode.result = Some(result);
                let total = mode.visible().len();
                mode.current = mode.current.filter(|_| total > 0).map(|i| i.min(total - 1));
                if let Ok(mut rects) = mode.rects.lock() {
                    *rects = None;
                }
                if mode.current.is_some() {
                    self.reveal_current();
                }
                Task::none()
            }
            CompareMsg::Next | CompareMsg::Previous => {
                let Some(mode) = self.compare.as_mut() else {
                    return Task::none();
                };
                mode.notice = None;
                let total = mode.visible().len();
                if total == 0 {
                    return Task::none();
                }
                let forward = matches!(msg, CompareMsg::Next);
                mode.current = Some(match (mode.current, forward) {
                    (None, true) => 0,
                    (None, false) => total - 1,
                    (Some(i), true) => (i + 1) % total,
                    (Some(i), false) => (i + total - 1) % total,
                });
                self.reveal_current();
                Task::none()
            }
            CompareMsg::Keep => {
                let Some(mode) = self.compare.as_mut() else {
                    return Task::none();
                };
                mode.notice = None;
                if let Some(located) = mode.current_hunk() {
                    mode.kept.insert(located.key);
                    if let Ok(mut rects) = mode.rects.lock() {
                        *rects = None;
                    }
                    let total = mode.visible().len();
                    mode.current = mode.current.filter(|_| total > 0).map(|i| i.min(total - 1));
                    if mode.current.is_some() {
                        self.reveal_current();
                    }
                }
                Task::none()
            }
            CompareMsg::Revert => {
                let Some(located) = self.compare.as_ref().and_then(CompareMode::current_hunk)
                else {
                    return Task::none();
                };
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                let result = match located.hunk.revert() {
                    Revert::EditText { block, edits } => {
                        let edits: Vec<_> = edits
                            .into_iter()
                            .map(|edit| (edit.range, edit.replacement))
                            .collect();
                        doc.edit_unlocked(|ed| ed.replace_in_paragraph(block, &edits))
                    }
                    Revert::Replace {
                        remove,
                        after,
                        before,
                        texts,
                    } => doc
                        .edit_unlocked(|ed| ed.replace_paragraphs(&remove, after, before, &texts)),
                };
                match result {
                    Ok(()) => {
                        self.dirty = true;
                        self.after_edit();
                        if let Some(mode) = self.compare.as_mut() {
                            mode.notice = None;
                        }
                        self.recompare()
                    }
                    Err(error) => {
                        tracing::warn!(%error, "revert to posted failed");
                        Task::none()
                    }
                }
            }
            CompareMsg::MarkPosted => self.mark_posted(),
            CompareMsg::Exit => {
                self.exit_compare();
                Task::none()
            }
        }
    }

    /// Compare mode is on for the open document.
    pub(super) fn compare_on(&self) -> bool {
        self.compare.is_some() && self.doc.as_ref().is_some_and(|doc| doc.read_only)
    }

    pub(super) fn exit_compare(&mut self) {
        self.compare = None;
        self.state.compare = false;
        if let Some(doc) = self.doc.as_mut() {
            doc.read_only = false;
        }
    }

    /// Starts a comparison of the document as it is now, on a task.
    fn recompare(&mut self) -> Task<Message> {
        let (Some(doc), Some(path), Some(mode)) =
            (self.doc.as_ref(), self.file.clone(), self.compare.as_mut())
        else {
            return Task::none();
        };
        mode.generation += 1;
        mode.computing = true;
        let generation = mode.generation;
        let document = doc.document().clone();
        let levels = self.settings.chapter_headings;
        Task::perform(
            async move { compute(&document, levels, &path) },
            move |result| Message::Compare(CompareMsg::Computed(generation, Arc::new(result))),
        )
    }

    /// Selects the current change and scrolls it into view.
    fn reveal_current(&mut self) {
        let Some(located) = self.compare.as_ref().and_then(CompareMode::current_hunk) else {
            return;
        };
        let Some(doc) = self.doc.as_mut() else {
            return;
        };
        let (block, range) = change_anchor(&located.hunk, doc.document());
        let Some(block) = block else {
            return;
        };
        doc.move_caret(|ed| {
            ed.set_selection(
                libertum_editor::DocPosition {
                    block,
                    offset: range.start,
                },
                libertum_editor::DocPosition {
                    block,
                    offset: range.end,
                },
            );
        });
        self.ensure_caret_visible();
        self.refresh_view();
    }

    /// Records the chapter at the caret (or the current change's chapter) as posted.
    fn mark_posted(&mut self) -> Task<Message> {
        let (Some(doc), Some(path)) = (self.doc.as_ref(), self.file.clone()) else {
            return Task::none();
        };
        let document = doc.document();
        let chapters = libertum_export::chapters(document, self.settings.chapter_headings);
        let caret = doc.editor().caret().block;
        let at = document.body().index_of(caret).or_else(|| {
            let located = self.compare.as_ref()?.current_hunk()?;
            let (block, _) = change_anchor(&located.hunk, document);
            document.body().index_of(block?)
        });
        let Some((index, chapter)) = at.and_then(|at| {
            chapters
                .iter()
                .enumerate()
                .find(|(_, chapter)| (chapter.start..chapter.end).contains(&at))
        }) else {
            if let Some(mode) = self.compare.as_mut() {
                mode.notice = Some(s::COMPARE_NOT_IN_CHAPTER.to_owned());
            }
            return Task::none();
        };
        let key = ChapterKey::new(index, chapter.title.clone());
        let texts = libertum_diff::posted_texts(document, chapter.start..chapter.end);
        let notice = match libertum_library::save_posted(&path, &key, &texts, SystemTime::now()) {
            Ok(_) => s::compare_marked(&chapter.title),
            Err(error) => {
                tracing::warn!(%error, "could not keep the posted copy");
                format!("{} {error}", s::COMPARE_MARK_FAILED)
            }
        };
        if let Some(mode) = self.compare.as_mut() {
            mode.notice = Some(notice);
        }
        self.recompare()
    }

    /// The mark rectangles for every change not kept: added text, and bars where posted
    /// text was removed.
    pub(super) fn compare_rects(&self) -> RectSet {
        let (Some(mode), Some(doc)) = (self.compare.as_ref(), self.doc.as_ref()) else {
            return RectSet::default();
        };
        if !self.compare_on() {
            return RectSet::default();
        }
        let pages = doc.source.pages.generation();
        let key = mode.generation ^ (mode.kept.len() as u64).rotate_left(32);
        let mut cache = mode.rects.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_pages, cached_key, rects)) = cache.as_ref()
            && *cached_pages == pages
            && *cached_key == key
        {
            return rects.clone();
        }
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let bar = |block: BlockId, offset: usize| {
            doc.position_rect(block, offset).map(|r| SelectionRect {
                page: r.page,
                x: r.x - 1.0,
                y: r.y,
                width: 2.0,
                height: r.height,
            })
        };
        for located in mode.visible() {
            let hunk = &located.hunk;
            for marks in &hunk.marks {
                for range in &marks.inserted {
                    added.extend(doc.range_rects(marks.block, range.start, range.end));
                }
                for deletion in &marks.deleted {
                    removed.extend(bar(marks.block, deletion.offset));
                }
            }
            if hunk.kind == HunkKind::Removed {
                let place = match (hunk.after, hunk.before) {
                    (Some(after), _) => doc
                        .document()
                        .paragraph(after)
                        .map(|p| (after, p.text().len())),
                    (None, Some(before)) => Some((before, 0)),
                    (None, None) => None,
                };
                if let Some((block, offset)) = place {
                    removed.extend(bar(block, offset));
                }
            }
        }
        let rects = (added, removed);
        *cache = Some((pages, key, rects.clone()));
        rects
    }
}

/// Where a change sits in the current text: the paragraph and the range to select.
fn change_anchor(hunk: &Hunk, document: &Document) -> (Option<BlockId>, std::ops::Range<usize>) {
    if let Some(marks) = hunk.marks.first() {
        if let Some(range) = marks.inserted.first() {
            return (Some(marks.block), range.clone());
        }
        if let Some(deletion) = marks.deleted.first() {
            return (Some(marks.block), deletion.offset..deletion.offset);
        }
    }
    if let Some(first) = hunk.new_ids.first() {
        let len = document.paragraph(*first).map_or(0, |p| p.text().len());
        return (Some(*first), 0..len);
    }
    match (hunk.after, hunk.before) {
        (Some(after), _) => {
            let len = document.paragraph(after).map_or(0, |p| p.text().len());
            (Some(after), len..len)
        }
        (None, Some(before)) => (Some(before), 0..0),
        (None, None) => (None, 0..0),
    }
}
