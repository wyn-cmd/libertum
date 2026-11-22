//! Exporting a document to formats that are not the app's own: an AO3 chapter
//! fragment, an EPUB book and plain text.
//!
//! Nothing here touches the UI. The chapter rules live in [`ChapterLevels`] and are used by
//! both the AO3 export and EPUB, so a book splits the same way everywhere.

mod autotags;
mod epub;
mod html;
mod tag_cache;
mod tags;
mod text;

pub use autotags::{
    MAX_CHARACTERS, MAX_KEYWORDS, MAX_RELATIONSHIPS, NameBook, auto_suggestions,
    auto_suggestions_with, merge_suggestions,
};
pub use epub::{EpubError, build_epub};
pub use html::{AO3_WHITELIST, ao3_fragment, chapter_html};
pub use tag_cache::{
    CacheSource, CachedTag, MAX_CACHED_TAGS, STARTER_LIST, TagCache, parse_tag_import, starter_tags,
};
pub use tags::{
    AO3_RATINGS, AO3_WARNINGS, SUGGESTION_LIMIT, Tag, TagCategory, TagMatch, TagSuggestion,
    suggest_tags, suggest_tags_with, tags_to_text, terms,
};
pub use text::{chapter_text, document_text};

use libertum_model::{Block, Document};
use serde::{Deserialize, Serialize};

/// Which heading levels open a new chapter. The writer chooses in Settings; Heading 1 and
/// Heading 2 are on by default, as requested for books whose chapters use both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChapterLevels {
    pub h1: bool,
    pub h2: bool,
    pub h3: bool,
    pub h4: bool,
}

impl Default for ChapterLevels {
    fn default() -> Self {
        Self {
            h1: true,
            h2: true,
            h3: false,
            h4: false,
        }
    }
}

impl ChapterLevels {
    /// Whether a heading level (1-based, as `ParaProps::outline_level`) opens a chapter.
    /// Levels the settings cannot choose (5 to 9) never do.
    pub fn includes(self, level: u8) -> bool {
        match level {
            1 => self.h1,
            2 => self.h2,
            3 => self.h3,
            4 => self.h4,
            _ => false,
        }
    }

    /// The chosen levels as 1-based numbers, for messages and tests.
    pub fn levels(self) -> impl Iterator<Item = u8> {
        [self.h1, self.h2, self.h3, self.h4]
            .into_iter()
            .enumerate()
            .filter_map(|(index, on)| on.then_some(index as u8 + 1))
    }

    pub fn any(self) -> bool {
        self.h1 || self.h2 || self.h3 || self.h4
    }
}

/// One chapter: a heading at a chosen level and the blocks up to, but not including, the
/// next such heading. `level` is `None` for the whole-document fallback (a document with no
/// matching heading), whose title is [`WHOLE_DOCUMENT`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chapter {
    pub title: String,
    /// Index of the heading paragraph among the document's top-level blocks (or 0).
    pub start: usize,
    /// One past the chapter's last top-level block.
    pub end: usize,
    /// The heading level, 1 to 4, or `None` for the whole-document chapter.
    pub level: Option<u8>,
}

/// The title of the chapter that is the whole document.
pub const WHOLE_DOCUMENT: &str = "Whole document";

/// Splits a document at its headings. Content before the first matching heading is front
/// matter and is not part of any chapter; a document with no matching heading is one
/// chapter containing everything.
pub fn chapters(document: &Document, levels: ChapterLevels) -> Vec<Chapter> {
    let blocks: Vec<&Block> = document.body().iter().collect();
    let starts: Vec<(usize, String, u8)> = blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| {
            let paragraph = block.as_paragraph()?;
            let level = paragraph.props.outline_level?;
            if !levels.includes(level) {
                return None;
            }
            // The generated table of contents has a heading-styled title ("Contents")
            // followed by its entries. That title is not a chapter.
            let next_is_toc = blocks
                .get(index + 1)
                .and_then(|block| block.as_paragraph())
                .is_some_and(|next| next.props.toc_target.is_some());
            if next_is_toc || paragraph.props.toc_target.is_some() {
                return None;
            }
            let title = paragraph.text().trim();
            let title = if title.is_empty() {
                "Untitled chapter".to_owned()
            } else {
                title.to_owned()
            };
            Some((index, title, level))
        })
        .collect();

    if starts.is_empty() {
        return vec![Chapter {
            title: WHOLE_DOCUMENT.to_owned(),
            start: 0,
            end: blocks.len(),
            level: None,
        }];
    }

    starts
        .iter()
        .enumerate()
        .map(|(position, (start, title, level))| Chapter {
            title: title.clone(),
            start: *start,
            end: starts
                .get(position + 1)
                .map_or(blocks.len(), |(next, _, _)| *next),
            level: Some(*level),
        })
        .collect()
}

/// The model blocks of `chapter`, in order.
pub(crate) fn chapter_blocks<'a>(document: &'a Document, chapter: &Chapter) -> Vec<&'a Block> {
    document
        .body()
        .iter()
        .skip(chapter.start)
        .take(chapter.end.saturating_sub(chapter.start))
        .collect()
}

#[cfg(test)]
mod tests;
