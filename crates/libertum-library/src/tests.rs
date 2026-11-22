use std::fs;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use super::*;

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("libertum-library-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create the test folder");
    dir
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

#[test]
fn a_failed_save_leaves_the_original_and_no_temporary_file() {
    let dir = folder("atomic-fail");
    let file = dir.join("book.docx");
    fs::write(&file, b"original").expect("write");
    let result: Result<(), SaveError<&str>> = save_atomic(&file, |temporary| {
        fs::write(temporary, b"half").expect("write");
        Err("boom")
    });
    assert!(matches!(result, Err(SaveError::Write("boom"))));
    assert_eq!(fs::read(&file).expect("read"), b"original");
    assert_eq!(fs::read_dir(&dir).expect("list").count(), 1, "no leftovers");
}

#[test]
fn a_save_replaces_the_file_in_one_step() {
    let dir = folder("atomic-ok");
    let file = dir.join("book.odt");
    fs::write(&file, b"old").expect("write");
    save_atomic::<io::Error>(&file, |temporary| fs::write(temporary, b"new")).expect("save");
    assert_eq!(fs::read(&file).expect("read"), b"new");
    assert_eq!(fs::read_dir(&dir).expect("list").count(), 1);
}

#[cfg(unix)]
#[test]
fn a_save_keeps_the_files_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = folder("atomic-permissions");
    let file = dir.join("book.odt");
    fs::write(&file, b"old").expect("write");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("make it private");
    save_atomic::<io::Error>(&file, |temporary| fs::write(temporary, b"new")).expect("save");
    let mode = fs::metadata(&file).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "a save must not widen access to the document");
}

#[test]
fn a_stamp_notices_changed_content_but_not_a_touch() {
    let dir = folder("stamp");
    let file = dir.join("a.lbtd");
    fs::write(&file, b"one").expect("write");
    let stamp = DiskStamp::of(&file).expect("stamp").expect("exists");
    assert!(!stamp.is_stale(&file).expect("check"));
    fs::write(&file, b"one").expect("rewrite the same bytes");
    assert!(!stamp.is_stale(&file).expect("check"), "same bytes");
    fs::write(&file, b"two").expect("change");
    assert!(stamp.is_stale(&file).expect("check"));
    fs::remove_file(&file).expect("delete");
    assert!(stamp.is_stale(&file).expect("check"), "a deleted file");
    assert_eq!(DiskStamp::of(&file).expect("stamp"), None);
}

#[test]
fn history_keeps_earlier_versions_and_skips_duplicates_and_rapid_saves() {
    let dir = folder("history");
    let file = dir.join("novel.lbtd");
    assert_eq!(snapshot(&file, at(1_000)).expect("none yet"), None);
    fs::write(&file, b"v1").expect("write");
    let first = snapshot(&file, at(1_000))
        .expect("snapshot")
        .expect("taken");
    assert_eq!(first.taken, 1_000);
    assert_eq!(fs::read(&first.path).expect("read"), b"v1");
    // Too soon after the last one.
    fs::write(&file, b"v2").expect("write");
    assert_eq!(snapshot(&file, at(1_100)).expect("snapshot"), None);
    // Later, with the same content as the newest version: nothing new.
    fs::write(&file, b"v1").expect("write");
    assert_eq!(snapshot(&file, at(2_000)).expect("snapshot"), None);
    fs::write(&file, b"v3").expect("write");
    let third = snapshot(&file, at(2_000))
        .expect("snapshot")
        .expect("taken");
    let all = versions(&file);
    assert_eq!(all.len(), 2);
    assert_eq!(all[0], third, "newest first");
    assert!(
        all[0].path.extension().is_some_and(|e| e == "lbtd"),
        "opens as a document"
    );
}

#[test]
fn old_versions_thin_out_to_one_a_day() {
    let dir = folder("prune");
    let file = dir.join("a.docx");
    fs::write(&file, b"x").expect("write");
    let history = history_dir(&file);
    fs::create_dir_all(&history).expect("create");
    let now = 200 * 86_400;
    // Three versions on one old day, and one from 120 days ago.
    for taken in [
        now - 10 * 86_400 - 40_000,
        now - 10 * 86_400 - 40_000 + 60,
        now - 10 * 86_400 - 40_000 + 120,
        now - 120 * 86_400,
    ] {
        fs::write(history.join(format!("{taken}.docx")), b"old").expect("write");
    }
    fs::write(&file, b"changed").expect("write");
    snapshot(&file, at(now)).expect("snapshot");
    let ages: Vec<u64> = versions(&file)
        .iter()
        .map(|v| (now - v.taken).div_ceil(86_400))
        .collect();
    assert_eq!(
        versions(&file).len(),
        2,
        "the new one and one old day's: {ages:?}"
    );
    assert!(!ages.iter().any(|d| *d >= 100), "past the limit: {ages:?}");
}

#[test]
fn conflict_copies_are_recognised_only_beside_their_original() {
    let siblings: Vec<String> = [
        "Novel.docx",
        "Novel (conflicted copy 2026-11-13).docx",
        "Novel (1).docx",
        "Novel-LAPTOP.docx",
        "Other-Stuff.docx",
    ]
    .map(String::from)
    .to_vec();
    for copy in &siblings[1..4] {
        assert_eq!(
            conflict_original(copy, &siblings).as_deref(),
            Some("Novel.docx"),
            "{copy}"
        );
    }
    assert_eq!(conflict_original("Novel.docx", &siblings), None);
    assert_eq!(
        conflict_original("Other-Stuff.docx", &siblings),
        None,
        "no 'Other.docx' beside it"
    );
}

#[test]
fn the_library_lists_documents_and_skips_the_history() {
    let dir = folder("scan");
    fs::create_dir_all(dir.join("Series")).expect("create");
    fs::write(dir.join("a.docx"), b"a").expect("write");
    fs::write(dir.join("Series").join("b.lbtd"), b"b").expect("write");
    fs::write(dir.join("notes.txt"), b"not a document").expect("write");
    fs::write(dir.join("a (1).docx"), b"copy").expect("write");
    snapshot(&dir.join("a.docx"), at(5_000)).expect("snapshot");
    let found = scan_library(&dir, 3);
    let names: Vec<String> = found
        .iter()
        .map(|e| {
            e.path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(found.len(), 3, "{names:?}");
    assert!(!names.iter().any(|n| n == "notes.txt" || n.starts_with('.')));
    let copy = found
        .iter()
        .find(|e| e.path.ends_with("a (1).docx"))
        .expect("copy");
    assert_eq!(copy.conflict_of.as_deref(), Some("a.docx"));
    let original = found
        .iter()
        .find(|e| e.path.ends_with("a.docx"))
        .expect("a");
    assert_eq!(original.versions, 1);
}

#[test]
fn restoring_a_version_keeps_what_it_replaces_so_it_can_be_undone() {
    let dir = folder("restore");
    let file = dir.join("a.lbtd");
    fs::write(&file, b"old").expect("write");
    let old = snapshot(&file, at(1_000))
        .expect("snapshot")
        .expect("taken");
    fs::write(&file, b"newer").expect("write");
    // Only moments later: a restore still keeps the current file.
    restore(&file, &old.path, at(1_010)).expect("restore");
    assert_eq!(fs::read(&file).expect("read"), b"old");
    let all = versions(&file);
    assert_eq!(all.len(), 2);
    assert_eq!(fs::read(&all[0].path).expect("read"), b"newer");
}

#[test]
fn a_conflict_is_settled_either_way_without_losing_either_side() {
    let dir = folder("conflict");
    let original = dir.join("n.docx");
    let copy = dir.join("n (1).docx");
    fs::write(&original, b"mine").expect("write");
    fs::write(&copy, b"theirs").expect("write");
    archive_copy(&original, &copy).expect("archive");
    assert!(!copy.exists());
    assert_eq!(fs::read(&original).expect("read"), b"mine");
    assert_eq!(
        fs::read(&versions(&original)[0].path).expect("read"),
        b"theirs"
    );

    fs::write(&copy, b"theirs again").expect("write");
    use_copy(&original, &copy, at(9_000)).expect("use");
    assert!(!copy.exists());
    assert_eq!(fs::read(&original).expect("read"), b"theirs again");
    let kept: Vec<Vec<u8>> = versions(&original)
        .iter()
        .map(|v| fs::read(&v.path).expect("read"))
        .collect();
    assert!(
        kept.contains(&b"mine".to_vec()),
        "my version is still there"
    );
}

#[test]
fn two_versions_in_the_same_second_do_not_overwrite_each_other() {
    let dir = folder("same-second");
    let file = dir.join("a.lbtd");
    fs::write(&file, b"one").expect("write");
    let first = snapshot(&file, at(5_000))
        .expect("snapshot")
        .expect("taken");
    fs::write(&file, b"two").expect("write");
    keep_current(&file, at(5_000)).expect("keep");
    assert_eq!(
        fs::read(&first.path).expect("read"),
        b"one",
        "not overwritten"
    );
    let all = versions(&file);
    assert_eq!(all.len(), 2);
    assert_eq!(fs::read(&all[0].path).expect("read"), b"two");
}

// ------------------------------------------------------------------ last posted copies

#[test]
fn a_posted_copy_is_saved_and_loaded_by_its_chapter() {
    let dir = folder("posted");
    let file = dir.join("book.docx");
    fs::write(&file, b"doc").expect("write");
    let one = ChapterKey::new(0, "Chapter One");
    let two = ChapterKey::new(1, "Chapter Two");
    assert!(load_posted(&file, &one).is_none());
    let paragraphs = vec!["Chapter One".to_owned(), "It began — “quietly”.".to_owned()];
    let saved = save_posted(&file, &one, &paragraphs, at(1_000)).expect("save");
    save_posted(&file, &two, &["Two".to_owned()], at(1_001)).expect("save");
    assert!(saved.path.starts_with(history_dir(&file).join(POSTED_DIR)));

    let loaded = load_posted(&file, &one).expect("loaded");
    assert_eq!(loaded.paragraphs, paragraphs);
    assert_eq!(loaded.saved_at, 1_000);
    assert_eq!(loaded.key, one);

    // Marking it again replaces the copy.
    save_posted(&file, &one, &["New".to_owned()], at(2_000)).expect("save");
    assert_eq!(
        load_posted(&file, &one).expect("loaded").paragraphs,
        ["New"]
    );
    let all = list_posted(&file);
    assert_eq!(
        all.iter().map(|c| c.key.clone()).collect::<Vec<_>>(),
        vec![one.clone(), two]
    );

    // The posted folder is not mistaken for a version, and pruning leaves it alone.
    assert!(versions(&file).is_empty());
    keep_current(&file, at(3_000)).expect("snapshot");
    assert_eq!(versions(&file).len(), 1);
    assert!(load_posted(&file, &one).is_some());
}

#[test]
fn a_moved_chapter_finds_its_copy_by_title_unless_the_title_is_shared() {
    let dir = folder("posted-moved");
    let file = dir.join("book.lbtd");
    save_posted(
        &file,
        &ChapterKey::new(2, "Interlude"),
        &["x".to_owned()],
        at(5),
    )
    .expect("save");
    let moved = load_posted(&file, &ChapterKey::new(4, "Interlude")).expect("by title");
    assert_eq!(moved.key.index, 2);
    assert!(load_posted(&file, &ChapterKey::new(2, "Something else")).is_none());

    save_posted(
        &file,
        &ChapterKey::new(7, "Interlude"),
        &["y".to_owned()],
        at(6),
    )
    .expect("save");
    assert!(
        load_posted(&file, &ChapterKey::new(4, "Interlude")).is_none(),
        "two copies share the title: no guess"
    );
    assert_eq!(
        load_posted(&file, &ChapterKey::new(7, "Interlude"))
            .expect("exact")
            .paragraphs,
        ["y"]
    );
}

#[test]
fn an_unreadable_posted_file_is_skipped() {
    let dir = folder("posted-bad");
    let file = dir.join("book.md");
    fs::create_dir_all(posted_dir(&file)).expect("dir");
    fs::write(posted_dir(&file).join("junk.json"), b"{not json").expect("write");
    assert!(list_posted(&file).is_empty());
}
