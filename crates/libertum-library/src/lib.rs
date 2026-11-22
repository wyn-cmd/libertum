//! Keeping documents safe in a folder that syncs between machines.
//!
//! Nothing here knows about document formats: it works on files. That is what lets one set of
//! rules cover `.lbtd`, `.docx`, `.odt`, `.rtf` and Markdown alike.
//!
//! - **Safe saves** ([`save_atomic`]): the new file is written beside the old one and renamed
//!   over it, so a sync client never uploads a half-written file and a crash never leaves a
//!   broken document.
//! - **Change detection** ([`DiskStamp`]): a stamp taken when a file is opened or saved says
//!   whether another machine (or program) has changed it since.
//! - **Version history** ([`snapshot`], [`versions`]): before a save replaces a file, its
//!   current content is copied into a hidden `.libertum-history` folder beside it, so the
//!   history travels with the document through the sync service.
//! - **Conflict copies** ([`conflict_original`]): sync services name the loser of a clash
//!   "name (conflicted copy ...)", "name (1)" or "name-MACHINE"; these are recognised so the
//!   library can point at them.
//! - **The library folder** ([`scan_library`]): the documents in one folder tree, newest first.
//! - **"Last posted" copies** ([`save_posted`], [`load_posted`]): a chapter's text as it was
//!   when the writer last marked it posted, kept in the history folder for comparison.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod posted;
pub use posted::{
    ChapterKey, POSTED_DIR, PostedCopy, list_posted, load_posted, posted_dir, save_posted,
};

/// The hidden folder, beside a document, that holds its earlier versions.
pub const HISTORY_DIR: &str = ".libertum-history";

/// A snapshot is taken at most this often per document, however often it is saved.
pub const MIN_SNAPSHOT_GAP_SECS: u64 = 300;

/// Every snapshot from the last day is kept (up to `RECENT_CAP`); older ones thin out to one
/// per day, and nothing is kept past `KEEP_DAYS`.
const RECENT_SECS: u64 = 86_400;
const RECENT_CAP: usize = 50;
const KEEP_DAYS: u64 = 90;

/// The extensions the library lists as documents.
pub const DOCUMENT_EXTENSIONS: [&str; 6] = ["lbtd", "docx", "odt", "rtf", "md", "markdown"];

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

// ------------------------------------------------------------------ safe saves

/// Writes a file by letting `write` fill a temporary file beside `path` and then renaming it
/// over `path`. On failure the temporary file is removed and `path` is untouched.
pub fn save_atomic<E>(
    path: &Path,
    write: impl FnOnce(&Path) -> Result<(), E>,
) -> Result<(), SaveError<E>> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    let temporary = path.with_file_name(format!(".{name}.{}.saving", std::process::id()));
    // A rename replaces the file's mode as well as its bytes. Give the new file the mode of
    // the one it replaces, or a private document (0600) would become world-readable (0644).
    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    if let Err(error) = write(&temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(SaveError::Write(error));
    }
    if let Some(permissions) = permissions
        && let Err(error) = fs::set_permissions(&temporary, permissions)
    {
        let _ = fs::remove_file(&temporary);
        return Err(SaveError::Permissions(error));
    }
    // A sync client may hold the file for a moment: try a few times before giving up.
    let mut last = None;
    for attempt in 0..5u64 {
        match fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(error) => {
                last = Some(error);
                std::thread::sleep(Duration::from_millis(60 * (attempt + 1)));
            }
        }
    }
    let _ = fs::remove_file(&temporary);
    Err(SaveError::Replace(last.unwrap_or_else(|| {
        io::Error::other("the file could not be replaced")
    })))
}

/// Why [`save_atomic`] failed.
#[derive(Debug)]
pub enum SaveError<E> {
    /// The caller's writer failed; the original file is untouched.
    Write(E),
    /// The new file could not be given the original's permissions; it was removed and the
    /// original is untouched.
    Permissions(io::Error),
    /// The finished temporary file could not be moved into place.
    Replace(io::Error),
}

// ------------------------------------------------------------------ change detection

/// What a file looked like when it was last read or written by this program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskStamp {
    modified: Option<SystemTime>,
    len: u64,
    hash: u64,
}

impl DiskStamp {
    /// The stamp of the file at `path`, or `None` when there is no such file.
    pub fn of(path: &Path) -> io::Result<Option<Self>> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let modified = fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(Some(Self {
            modified,
            len: bytes.len() as u64,
            hash: fnv1a(&bytes),
        }))
    }

    /// Whether the file's *content* differs from this stamp. A file merely touched by a sync
    /// service (same bytes, new time) is not a change; a deleted file is.
    pub fn is_stale(&self, path: &Path) -> io::Result<bool> {
        Ok(match Self::of(path)? {
            None => true,
            Some(now) => now.len != self.len || now.hash != self.hash,
        })
    }

    /// When the file was last modified, if the system says.
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }
}

// ------------------------------------------------------------------ version history

/// One earlier version of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    /// The copy in the history folder (same extension as the document, so it opens as one).
    pub path: PathBuf,
    /// When the version was taken, seconds since the Unix epoch.
    pub taken: u64,
    pub size: u64,
}

/// The history folder of the document at `path`.
pub fn history_dir(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".to_owned());
    path.parent()
        .unwrap_or_else(|| Path::new("."))
        .join(HISTORY_DIR)
        .join(name)
}

/// The earlier versions of the document at `path`, newest first.
pub fn versions(path: &Path) -> Vec<Version> {
    let Ok(entries) = fs::read_dir(history_dir(path)) else {
        return Vec::new();
    };
    let mut found: Vec<Version> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let taken: u64 = path.file_stem()?.to_str()?.parse().ok()?;
            Some(Version {
                size: entry.metadata().map_or(0, |m| m.len()),
                path,
                taken,
            })
        })
        .collect();
    found.sort_by_key(|version| std::cmp::Reverse(version.taken));
    found
}

/// Copies the document's current file into its history, if it differs from the newest
/// version and the newest is older than [`MIN_SNAPSHOT_GAP_SECS`]; then thins the history.
/// Returns the new version. Call this *before* the save that replaces the file. A document
/// that does not exist yet has nothing to keep.
pub fn snapshot(path: &Path, now: SystemTime) -> io::Result<Option<Version>> {
    snapshot_after(path, now, MIN_SNAPSHOT_GAP_SECS)
}

fn snapshot_after(path: &Path, now: SystemTime, gap: u64) -> io::Result<Option<Version>> {
    // The gap check comes first: on most saves a recent version exists, and hashing the
    // whole document just to discard the snapshot would make every save read the file twice.
    let existing = versions(path);
    let now_secs = seconds(now);
    if let Some(newest) = existing.first()
        && now_secs.saturating_sub(newest.taken) < gap
    {
        return Ok(None);
    }
    let Some(stamp) = DiskStamp::of(path)? else {
        return Ok(None);
    };
    if let Some(newest) = existing.first()
        && fs::read(&newest.path).is_ok_and(|bytes| fnv1a(&bytes) == stamp.hash)
    {
        return Ok(None);
    }
    let dir = history_dir(path);
    fs::create_dir_all(&dir)?;
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    // Two versions in the same second must not share a name, or the second would overwrite the
    // first: take the next free second.
    let mut taken = now_secs;
    while dir.join(format!("{taken}.{extension}")).exists() {
        taken += 1;
    }
    let target = dir.join(format!("{taken}.{extension}"));
    fs::copy(path, &target)?;
    let version = Version {
        size: stamp.len,
        path: target,
        taken,
    };
    prune(path, now_secs);
    Ok(Some(version))
}

/// Applies the retention rules to the history of the document at `path`.
fn prune(path: &Path, now: u64) {
    let mut seen_days = std::collections::HashSet::new();
    let mut recent = 0usize;
    for version in versions(path) {
        let age = now.saturating_sub(version.taken);
        let keep = if age < RECENT_SECS {
            recent += 1;
            recent <= RECENT_CAP
        } else {
            age < KEEP_DAYS * 86_400 && seen_days.insert(age / 86_400)
        };
        if !keep {
            let _ = fs::remove_file(&version.path);
        }
    }
}

/// Keeps the document's current file in its history whatever the time since the last version
/// (unless it is identical to the newest one). For actions that are about to replace it.
pub fn keep_current(path: &Path, now: SystemTime) -> io::Result<()> {
    snapshot_after(path, now, 0).map(|_| ())
}

/// Makes `version` the document's current file. The file it replaces is kept in the history
/// first, so a restore can itself be undone.
pub fn restore(path: &Path, version: &Path, now: SystemTime) -> io::Result<()> {
    keep_current(path, now)?;
    save_atomic(path, |temporary| fs::copy(version, temporary).map(|_| ())).map_err(|error| {
        match error {
            SaveError::Write(error) | SaveError::Permissions(error) | SaveError::Replace(error) => {
                error
            }
        }
    })
}

/// Settles a sync conflict in favour of `copy`: the original's current content is kept in its
/// history, the copy's content becomes the original, and the copy file is removed.
pub fn use_copy(original: &Path, copy: &Path, now: SystemTime) -> io::Result<()> {
    restore(original, copy, now)?;
    fs::remove_file(copy)
}

/// Settles a sync conflict in favour of the original: the copy is moved into the original's
/// history (so nothing is lost) and no longer sits beside it.
pub fn archive_copy(original: &Path, copy: &Path) -> io::Result<()> {
    let dir = history_dir(original);
    fs::create_dir_all(&dir)?;
    let extension = copy.extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let mut taken = fs::metadata(copy)
        .and_then(|m| m.modified())
        .map_or(0, seconds);
    while dir.join(format!("{taken}.{extension}")).exists() {
        taken += 1;
    }
    let target = dir.join(format!("{taken}.{extension}"));
    if fs::rename(copy, &target).is_err() {
        fs::copy(copy, &target)?;
        fs::remove_file(copy)?;
    }
    Ok(())
}

// ------------------------------------------------------------------ conflict copies

/// When `file_name` looks like the loser of a sync clash, the name of the document it is a
/// copy of. Recognised: "name (conflicted copy ...).ext", "name (1).ext", "name-MACHINE.ext"
/// and Syncthing's "name.sync-conflict-....ext"; each is only matched when the original is
/// among `siblings`.
pub fn conflict_original(file_name: &str, siblings: &[String]) -> Option<String> {
    let (stem, extension) = file_name.rsplit_once('.')?;
    let candidates = [
        stem.split_once(" (conflicted copy").map(|(base, _)| base),
        stem.split_once(" (Case Conflict").map(|(base, _)| base),
        stem.strip_suffix(')')
            .and_then(|rest| rest.rsplit_once(" ("))
            .filter(|(_, number)| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
            .map(|(base, _)| base),
        stem.rsplit_once('-').map(|(base, _)| base),
        stem.split_once(".sync-conflict-").map(|(base, _)| base),
    ];
    candidates.into_iter().flatten().find_map(|base| {
        let original = format!("{base}.{extension}");
        (original != file_name && siblings.contains(&original)).then_some(original)
    })
}

// ------------------------------------------------------------------ the library folder

/// One document found in the library folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryEntry {
    pub path: PathBuf,
    /// Seconds since the Unix epoch.
    pub modified: u64,
    pub size: u64,
    /// The document this file is a sync conflict copy of, when it looks like one.
    pub conflict_of: Option<String>,
    /// How many earlier versions the document has.
    pub versions: usize,
}

/// The documents in `root` and the folders under it (up to `depth` deep, skipping hidden
/// folders such as the history), newest first.
pub fn scan_library(root: &Path, depth: usize) -> Vec<LibraryEntry> {
    let mut out = Vec::new();
    scan(root, depth, &mut out);
    out.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

fn scan(dir: &Path, depth: usize, out: &mut Vec<LibraryEntry>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<_> = entries.filter_map(Result::ok).collect();
    let names: Vec<String> = entries
        .iter()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    for entry in &entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name.starts_with('~') {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            if depth > 0 {
                scan(&path, depth - 1, out);
            }
            continue;
        }
        let is_document = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| DOCUMENT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
        if !is_document {
            continue;
        }
        out.push(LibraryEntry {
            modified: meta.modified().map_or(0, seconds),
            size: meta.len(),
            conflict_of: conflict_original(&name, &names),
            versions: versions(&path).len(),
            path,
        });
    }
}

#[cfg(test)]
mod tests;
