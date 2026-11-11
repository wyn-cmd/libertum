//! Autosave snapshots for crash recovery (PLAN Phase 8).
//!
//! Every `autosave_secs` a dirty document is written to the platform data directory. The
//! snapshot never replaces the user's own file: on the next launch it is offered for
//! restore, and it is deleted after a successful save or a clean close, so it only survives
//! a crash.
//!
//! The snapshot is the native `.lbtd` container, an exact copy of the model, so a recovered
//! document loses nothing that the native format can hold (which is everything).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use libertum_model::Document;
use libertum_native::Native;

/// What a snapshot remembers about the document it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    /// The file the document was saved as, if it had one; Save writes back there.
    pub path: Option<PathBuf>,
    /// When the snapshot was written (seconds since the Unix epoch).
    pub saved_unix: u64,
}

/// The directory snapshots live in (created on first write).
pub fn data_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "Libertum", "Libertum").map(|dirs| dirs.data_dir().to_path_buf())
}

fn snapshot_path(dir: &Path) -> PathBuf {
    dir.join("recovery.lbtd")
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("recovery.txt")
}

/// Writes the snapshot for `document`, which came from `path` (if any).
pub fn write(dir: &Path, document: &Document, path: Option<&Path>) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // The native format stores the model one-to-one, so a recovered document is exact
    // rather than as close as ODT gets.
    let bytes = Native::new(document.clone())
        .to_bytes()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let snapshot = snapshot_path(dir);
    // Write beside the real file and rename: a crash mid-write leaves the old snapshot whole.
    let temporary = snapshot.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, &snapshot)?;
    let saved_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = match path {
        Some(path) => format!("{saved_unix}|{}", path.display()),
        None => format!("{saved_unix}|"),
    };
    std::fs::write(meta_path(dir), record)
}

/// Reads the snapshot, if one is complete and parses.
pub fn load(dir: &Path) -> Option<(Document, Meta)> {
    let snapshot = snapshot_path(dir);
    if !snapshot.exists() {
        return None;
    }
    let record = std::fs::read_to_string(meta_path(dir)).ok()?;
    let (saved, path) = record.split_once('|')?;
    let saved_unix = saved.trim().parse().ok()?;
    let path = path.trim();
    let path = (!path.is_empty()).then(|| PathBuf::from(path));
    let document = Native::open(&snapshot).ok()?.document().clone();
    Some((document, Meta { path, saved_unix }))
}

/// Deletes the snapshot: after a save, or after a clean close.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_file(snapshot_path(dir));
    let _ = std::fs::remove_file(meta_path(dir));
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

    #[test]
    fn a_snapshot_round_trips_with_its_file() {
        let dir = scratch("round-trip");
        let document = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Unsaved words")]);
        let target = PathBuf::from("/tmp/novel.odt");
        write(&dir, &document, Some(&target)).expect("write");
        let (read, meta) = load(&dir).expect("a snapshot");
        assert_eq!(read.paragraph_at(0).expect("p").text(), "Unsaved words");
        assert_eq!(meta.path.as_deref(), Some(target.as_path()));
        assert!(meta.saved_unix > 0);

        // Writing again replaces the snapshot; clearing removes both files.
        write(&dir, &document, None).expect("rewrite");
        assert_eq!(load(&dir).expect("a snapshot").1.path, None);
        clear(&dir);
        assert!(load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
