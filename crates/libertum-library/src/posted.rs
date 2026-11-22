//! "Last posted" copies: the text of a chapter as it was when the writer last marked it
//! posted to AO3 (AO3 plan, stage B2/B4). A local copy only; nothing is uploaded.
//!
//! Copies live in a `posted` folder inside the document's history folder
//! (`.libertum-history/<file name>/posted/`), so they travel with the document through a
//! sync service the way its versions do. One JSON file per chapter holds the chapter's key,
//! the time it was marked and its paragraphs as plain text. Like the versions, the copies
//! belong to the file name: renaming the document outside Libertum leaves them behind.
//!
//! ## Why a chapter is keyed by its position *and* its title
//!
//! A position alone would hand chapter 3's copy to a new chapter inserted before it; a title
//! alone cannot tell apart chapters that share one ("Untitled chapter", or two "Interlude"s).
//! So a copy is stored under both, and [`load_posted`] first looks for that exact pair. If
//! there is none, it falls back to the one copy with the same title, which follows a chapter
//! that moved. A copy that matches neither is not guessed at.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{fnv1a, history_dir, save_atomic, seconds};

/// The folder, inside a document's history folder, that holds its "last posted" copies.
pub const POSTED_DIR: &str = "posted";

/// The file format written; a newer format is not read.
const FORMAT: u32 = 1;

/// Which chapter a copy belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ChapterKey {
    /// The chapter's position among the document's chapters, from 0.
    pub index: usize,
    /// The chapter's heading text (or "Whole document").
    pub title: String,
}

impl ChapterKey {
    pub fn new(index: usize, title: impl Into<String>) -> Self {
        Self {
            index,
            title: title.into(),
        }
    }

    fn file_name(&self) -> String {
        format!(
            "chapter-{:04}-{:016x}.json",
            self.index,
            fnv1a(self.title.as_bytes())
        )
    }
}

/// A chapter's text as it was last posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostedCopy {
    pub key: ChapterKey,
    /// The chapter's paragraphs, plain text, in order.
    pub paragraphs: Vec<String>,
    /// When it was marked posted, seconds since the Unix epoch.
    pub saved_at: u64,
    /// The file it is kept in.
    pub path: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Stored {
    format: u32,
    key: ChapterKey,
    saved_at: u64,
    paragraphs: Vec<String>,
}

/// The folder holding the "last posted" copies of the document at `path`.
pub fn posted_dir(path: &Path) -> PathBuf {
    history_dir(path).join(POSTED_DIR)
}

/// Records `paragraphs` as chapter `key`'s last posted text, replacing an earlier copy under
/// the same key. The file is written safely (beside, then renamed).
pub fn save_posted(
    path: &Path,
    key: &ChapterKey,
    paragraphs: &[String],
    now: SystemTime,
) -> io::Result<PostedCopy> {
    let dir = posted_dir(path);
    fs::create_dir_all(&dir)?;
    let target = dir.join(key.file_name());
    let stored = Stored {
        format: FORMAT,
        key: key.clone(),
        saved_at: seconds(now),
        paragraphs: paragraphs.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&stored).map_err(io::Error::other)?;
    save_atomic(&target, |temporary| fs::write(temporary, &bytes)).map_err(
        |error| match error {
            crate::SaveError::Write(error)
            | crate::SaveError::Permissions(error)
            | crate::SaveError::Replace(error) => error,
        },
    )?;
    Ok(PostedCopy {
        key: stored.key,
        paragraphs: stored.paragraphs,
        saved_at: stored.saved_at,
        path: target,
    })
}

fn read(path: &Path) -> Option<PostedCopy> {
    let bytes = fs::read(path).ok()?;
    let stored: Stored = serde_json::from_slice(&bytes).ok()?;
    (stored.format <= FORMAT).then(|| PostedCopy {
        key: stored.key,
        paragraphs: stored.paragraphs,
        saved_at: stored.saved_at,
        path: path.to_owned(),
    })
}

/// Chapter `key`'s last posted copy: the one stored under exactly this key, or else the only
/// copy with the same title (the chapter moved). `None` when there is no such copy or the
/// title is ambiguous.
pub fn load_posted(path: &Path, key: &ChapterKey) -> Option<PostedCopy> {
    if let Some(copy) = read(&posted_dir(path).join(key.file_name()))
        && copy.key == *key
    {
        return Some(copy);
    }
    let mut same_title = list_posted(path)
        .into_iter()
        .filter(|copy| copy.key.title == key.title);
    match (same_title.next(), same_title.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    }
}

/// Every "last posted" copy of the document at `path`, in chapter order. Unreadable files
/// are skipped.
pub fn list_posted(path: &Path) -> Vec<PostedCopy> {
    let Ok(entries) = fs::read_dir(posted_dir(path)) else {
        return Vec::new();
    };
    let mut copies: Vec<PostedCopy> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|file| file.extension().is_some_and(|e| e == "json"))
        .filter_map(|file| read(&file))
        .collect();
    copies.sort_by(|a, b| {
        a.key
            .index
            .cmp(&b.key.index)
            .then_with(|| a.key.title.cmp(&b.key.title))
    });
    copies
}
