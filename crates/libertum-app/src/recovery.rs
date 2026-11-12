//! Autosave snapshots for crash recovery (PLAN Phase 8).
//!
//! Every `autosave_secs` a dirty document is written to the platform data directory. The
//! snapshot never replaces the user's own file: on the next launch it is offered for
//! restore, and it is deleted after a successful save or a clean close, so it only survives
//! a crash.
//!
//! The snapshot is the native `.lbtd` container, an exact copy of the model, so a recovered
//! document loses nothing that the native format can hold (which is everything).
//!
//! **Several windows.** Each running Libertum writes its own snapshot, named after its
//! process id (`recovery-<pid>.lbtd` with `recovery-<pid>.txt`). A launch offers only a
//! snapshot whose process is no longer running, so a second window never offers (or
//! discards) the live, unsaved work of the first, and two windows never overwrite each
//! other's snapshot. Older versions wrote one shared `recovery.lbtd`; it is still read, but
//! only once it has gone stale (a running older window rewrites it every minute).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use libertum_model::Document;
use libertum_native::Native;

/// A snapshot in the shared, pre-per-window layout is treated as abandoned once it is this
/// old: the older versions rewrote it every autosave period (a minute by default).
const LEGACY_STALE_SECS: u64 = 180;

/// What a snapshot remembers about the document it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// The file the document was saved as, if it had one; Save writes back there.
    pub path: Option<PathBuf>,
    /// When the snapshot was written (seconds since the Unix epoch).
    pub saved_unix: u64,
    /// The process that wrote it (`None` for the older shared layout).
    pub owner: Option<u32>,
}

/// The directory snapshots live in (created on first write).
pub fn data_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "Libertum", "Libertum").map(|dirs| dirs.data_dir().to_path_buf())
}

fn stem(owner: Option<u32>) -> String {
    match owner {
        Some(pid) => format!("recovery-{pid}"),
        None => "recovery".to_owned(),
    }
}

fn snapshot_path(dir: &Path, owner: Option<u32>) -> PathBuf {
    dir.join(format!("{}.lbtd", stem(owner)))
}

fn meta_path(dir: &Path, owner: Option<u32>) -> PathBuf {
    dir.join(format!("{}.txt", stem(owner)))
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Writes this process's snapshot of `document`, which came from `path` (if any).
pub fn write(dir: &Path, document: &Document, path: Option<&Path>) -> std::io::Result<()> {
    write_as(dir, std::process::id(), document, path)
}

/// Writes the snapshot of process `owner` (tests use another id to stand for a crashed
/// window).
pub fn write_as(
    dir: &Path,
    owner: u32,
    document: &Document,
    path: Option<&Path>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // The native format stores the model one-to-one, so a recovered document is exact
    // rather than as close as ODT gets.
    let bytes = Native::new(document.clone())
        .to_bytes()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let snapshot = snapshot_path(dir, Some(owner));
    // Write beside the real file and rename: a crash mid-write leaves the old snapshot whole.
    let temporary = snapshot.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &snapshot)?;
    let path = path.map(|p| p.display().to_string()).unwrap_or_default();
    std::fs::write(
        meta_path(dir, Some(owner)),
        format!("{}|{owner}|{path}", now_unix()),
    )
}

/// The newest snapshot left by a window that is no longer running, if one is complete and
/// parses. This process's own snapshot and those of running windows are never returned.
pub fn load(dir: &Path) -> Option<(Document, Meta)> {
    let me = std::process::id();
    let mut found: Vec<Meta> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name
            .strip_prefix("recovery")
            .and_then(|rest| rest.strip_suffix(".txt"))
        else {
            continue;
        };
        let owner = match id.strip_prefix('-') {
            Some(pid) => match pid.parse::<u32>() {
                Ok(pid) => Some(pid),
                Err(_) => continue,
            },
            None if id.is_empty() => None,
            None => continue,
        };
        let Some(meta) = read_meta(dir, owner) else {
            continue;
        };
        let abandoned = match owner {
            Some(pid) => pid != me && !is_running(pid),
            None => now_unix().saturating_sub(meta.saved_unix) >= LEGACY_STALE_SECS,
        };
        if abandoned && snapshot_path(dir, owner).exists() {
            found.push(meta);
        }
    }
    found.sort_by_key(|meta| std::cmp::Reverse(meta.saved_unix));
    found.into_iter().find_map(|meta| {
        let document = Native::open(snapshot_path(dir, meta.owner))
            .ok()?
            .document()
            .clone();
        Some((document, meta))
    })
}

/// Reads a snapshot's record: `saved|pid|path` (or the older `saved|path`).
fn read_meta(dir: &Path, owner: Option<u32>) -> Option<Meta> {
    let record = std::fs::read_to_string(meta_path(dir, owner)).ok()?;
    let (saved, rest) = record.split_once('|')?;
    let saved_unix = saved.trim().parse().ok()?;
    let path = match owner {
        Some(_) => rest.split_once('|').map_or("", |(_, path)| path),
        None => rest,
    };
    let path = path.trim();
    let path = (!path.is_empty()).then(|| PathBuf::from(path));
    Some(Meta {
        path,
        saved_unix,
        owner,
    })
}

/// Deletes this process's snapshot: after a save, or after a clean close.
pub fn clear(dir: &Path) {
    remove_owner(dir, Some(std::process::id()));
}

/// Deletes the snapshot `meta` was read from (once it is restored or discarded).
pub fn remove(dir: &Path, meta: &Meta) {
    remove_owner(dir, meta.owner);
}

fn remove_owner(dir: &Path, owner: Option<u32>) {
    let _ = std::fs::remove_file(snapshot_path(dir, owner));
    let _ = std::fs::remove_file(meta_path(dir, owner));
}

/// Whether process `pid` is still running.
#[cfg(windows)]
fn is_running(pid: u32) -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    // SAFETY: plain Win32 calls; the handle is checked for null and closed exactly once.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
        ok != 0 && code == STILL_ACTIVE
    }
}

/// Whether process `pid` is still running (signal 0 checks without sending anything).
#[cfg(unix)]
fn is_running(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 performs only the existence and permission check.
    let result = unsafe { kill(pid, 0) };
    // EPERM (1): the process exists but belongs to someone else.
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
}

#[cfg(not(any(windows, unix)))]
fn is_running(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use libertum_model::{BlockId, Paragraph};

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("libertum-recovery-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn doc(text: &str) -> Document {
        Document::from_paragraphs(vec![Paragraph::new(BlockId(1), text)])
    }

    /// A process id that is certainly not running.
    const GONE: u32 = u32::MAX - 6;

    #[test]
    fn a_crashed_window_snapshot_round_trips_with_its_file() {
        let dir = scratch("round-trip");
        let target = PathBuf::from("/tmp/novel.odt");
        write_as(&dir, GONE, &doc("Unsaved words"), Some(&target)).expect("write");
        let (read, meta) = load(&dir).expect("a snapshot");
        assert_eq!(read.paragraph_at(0).expect("p").text(), "Unsaved words");
        assert_eq!(meta.path.as_deref(), Some(target.as_path()));
        assert_eq!(meta.owner, Some(GONE));
        assert!(meta.saved_unix > 0);
        remove(&dir, &meta);
        assert!(load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshots_of_running_windows_are_never_offered() {
        let dir = scratch("running");
        // This process is running, and so is its own snapshot's owner.
        write(&dir, &doc("Mine, still open"), None).expect("write");
        assert!(load(&dir).is_none(), "its own snapshot");
        // A crashed window's snapshot is offered; clearing ours leaves it alone.
        write_as(&dir, GONE, &doc("From a crash"), None).expect("write");
        clear(&dir);
        let (read, _) = load(&dir).expect("the crashed window's");
        assert_eq!(read.paragraph_at(0).expect("p").text(), "From a crash");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_newest_abandoned_snapshot_is_offered_first() {
        let dir = scratch("newest");
        write_as(&dir, GONE, &doc("Older"), None).expect("write");
        write_as(&dir, GONE - 1, &doc("Newer"), None).expect("write");
        // Make the first one clearly older.
        std::fs::write(meta_path(&dir, Some(GONE)), format!("1|{GONE}|")).expect("age");
        let (read, meta) = load(&dir).expect("a snapshot");
        assert_eq!(read.paragraph_at(0).expect("p").text(), "Newer");
        remove(&dir, &meta);
        let (read, _) = load(&dir).expect("the other one");
        assert_eq!(read.paragraph_at(0).expect("p").text(), "Older");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_shared_snapshot_is_offered_only_once_stale() {
        let dir = scratch("legacy");
        std::fs::create_dir_all(&dir).expect("dir");
        let bytes = Native::new(doc("Old layout")).to_bytes().expect("bytes");
        std::fs::write(snapshot_path(&dir, None), bytes).expect("snapshot");
        // Fresh: an older Libertum may still be running and rewriting it.
        std::fs::write(
            meta_path(&dir, None),
            format!("{}|C:/books/a.odt", now_unix()),
        )
        .expect("meta");
        assert!(load(&dir).is_none());
        // Stale: offered, with its file.
        std::fs::write(meta_path(&dir, None), "1000|C:/books/a.odt").expect("meta");
        let (read, meta) = load(&dir).expect("a snapshot");
        assert_eq!(read.paragraph_at(0).expect("p").text(), "Old layout");
        assert_eq!(meta.path, Some(PathBuf::from("C:/books/a.odt")));
        assert_eq!(meta.owner, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_process_is_running_and_a_made_up_one_is_not() {
        assert!(is_running(std::process::id()));
        assert!(!is_running(GONE));
    }
}
