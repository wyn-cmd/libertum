//! Small files that belong to a document but have nowhere to live inside its format, such as
//! the story checklist of a `.docx`, `.odt` or Markdown file.
//!
//! They sit in the document's history folder (`.libertum-history/<file name>/`), so they travel
//! with the document through a sync service the way its versions do, and like the versions they
//! belong to the file name: renaming the document outside Libertum leaves them behind. The
//! names must not look like a version (a number), or the version list would pick them up.

use std::fs;
use std::io;
use std::path::Path;

use crate::{history_dir, seconds};

/// The bytes of the sidecar `name` for the document at `path`, if there is one.
pub fn load_sidecar(path: &Path, name: &str) -> Option<Vec<u8>> {
    fs::read(history_dir(path).join(name)).ok()
}

/// Writes the sidecar `name` for the document at `path`, or removes it when `bytes` is `None`
/// (an empty checklist leaves no file, and no folder, behind). The file is written beside its
/// place and renamed over it, so a sync client never uploads half of it.
pub fn save_sidecar(path: &Path, name: &str, bytes: Option<&[u8]>) -> io::Result<()> {
    let dir = history_dir(path);
    let target = dir.join(name);
    let Some(bytes) = bytes else {
        return match fs::remove_file(&target) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    };
    fs::create_dir_all(&dir)?;
    let temporary = dir.join(format!(
        ".{name}.{}.{}.saving",
        std::process::id(),
        seconds(std::time::SystemTime::now())
    ));
    if let Err(error) = fs::write(&temporary, bytes).and_then(|()| fs::rename(&temporary, &target))
    {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sidecar_round_trips_is_replaced_and_removed() {
        let dir = std::env::temp_dir().join(format!("libertum-sidecar-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("story.docx");

        assert_eq!(load_sidecar(&doc, "checklist.json"), None);
        save_sidecar(&doc, "checklist.json", Some(b"one")).unwrap();
        assert_eq!(
            load_sidecar(&doc, "checklist.json").as_deref(),
            Some(&b"one"[..])
        );
        save_sidecar(&doc, "checklist.json", Some(b"two")).unwrap();
        assert_eq!(
            load_sidecar(&doc, "checklist.json").as_deref(),
            Some(&b"two"[..])
        );
        // It is not mistaken for an earlier version of the document.
        assert!(crate::versions(&doc).is_empty());
        // No temporary file is left behind.
        let leftovers = fs::read_dir(history_dir(&doc)).unwrap().count();
        assert_eq!(leftovers, 1);

        save_sidecar(&doc, "checklist.json", None).unwrap();
        assert_eq!(load_sidecar(&doc, "checklist.json"), None);
        // Removing what is not there is not an error.
        save_sidecar(&doc, "checklist.json", None).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
