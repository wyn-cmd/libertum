use std::time::Instant;

use proptest::prelude::*;

use super::*;

/// Current paragraphs with ids 1, 2, 3...
fn current(texts: &[&str]) -> Vec<(BlockId, String)> {
    texts
        .iter()
        .enumerate()
        .map(|(i, t)| (BlockId(i as u64 + 1), (*t).to_owned()))
        .collect()
}

fn view(paras: &[(BlockId, String)]) -> Vec<NewParagraph<'_>> {
    paras
        .iter()
        .map(|(id, text)| NewParagraph { id: *id, text })
        .collect()
}

/// Applies a revert the way the app would, with fresh ids from `next_id`.
fn apply(paras: &mut Vec<(BlockId, String)>, revert: Revert, next_id: &mut u64) {
    let index_of =
        |paras: &[(BlockId, String)], id: BlockId| paras.iter().position(|(b, _)| *b == id);
    match revert {
        Revert::EditText { block, edits } => {
            let i = index_of(paras, block).expect("paragraph to edit");
            for edit in edits {
                paras[i].1.replace_range(edit.range, &edit.replacement);
            }
        }
        Revert::Replace {
            remove,
            after,
            before,
            texts,
        } => {
            let at = match (remove.first(), after, before) {
                (Some(first), _, _) => index_of(paras, *first).expect("first removed"),
                (None, Some(after), _) => index_of(paras, after).expect("after") + 1,
                (None, None, Some(before)) => index_of(paras, before).expect("before"),
                (None, None, None) => 0,
            };
            paras.retain(|(id, _)| !remove.contains(id));
            for (offset, text) in texts.into_iter().enumerate() {
                *next_id += 1;
                paras.insert(at + offset, (BlockId(*next_id), text));
            }
        }
    }
}

/// Reverts every hunk, last first, and returns the texts.
fn revert_all(old: &[&str], new: &[&str]) -> Vec<String> {
    let mut paras = current(new);
    let comparison = compare(old, &view(&paras)).expect("compare");
    let mut next_id = 10_000;
    for hunk in comparison.hunks.iter().rev() {
        apply(&mut paras, hunk.revert(), &mut next_id);
    }
    paras.into_iter().map(|(_, t)| t).collect()
}

#[test]
fn identical_texts_have_no_hunks() {
    let paras = current(&["One.", "Two.", ""]);
    let comparison = compare(&["One.", "Two.", ""], &view(&paras)).expect("compare");
    assert!(comparison.hunks.is_empty());
}

#[test]
fn empty_documents_compare() {
    let none: [&str; 0] = [];
    assert!(compare(&none, &[]).expect("compare").hunks.is_empty());

    let paras = current(&["New."]);
    let added = compare(&none, &view(&paras)).expect("compare");
    assert_eq!(added.hunks.len(), 1);
    assert_eq!(added.hunks[0].kind, HunkKind::Added);
    assert_eq!(added.hunks[0].marks[0].inserted.first(), Some(&(0..4)));

    let removed = compare(&["Gone."], &[]).expect("compare");
    assert_eq!(removed.hunks[0].kind, HunkKind::Removed);
    assert_eq!(
        removed.hunks[0].revert(),
        Revert::Replace {
            remove: vec![],
            after: None,
            before: None,
            texts: vec!["Gone.".into()],
        }
    );
}

#[test]
fn an_added_paragraph_is_one_hunk_between_its_neighbours() {
    let paras = current(&["A.", "Inserted.", "B."]);
    let comparison = compare(&["A.", "B."], &view(&paras)).expect("compare");
    assert_eq!(comparison.hunks.len(), 1);
    let hunk = &comparison.hunks[0];
    assert_eq!(hunk.kind, HunkKind::Added);
    assert_eq!(hunk.new_ids, vec![BlockId(2)]);
    assert_eq!(hunk.after, Some(BlockId(1)));
    assert_eq!(hunk.before, Some(BlockId(3)));
}

#[test]
fn a_removed_paragraph_sits_after_the_one_before_it() {
    let paras = current(&["A.", "C."]);
    let comparison = compare(&["A.", "B.", "C."], &view(&paras)).expect("compare");
    let hunk = &comparison.hunks[0];
    assert_eq!(hunk.kind, HunkKind::Removed);
    assert_eq!(hunk.old_texts, vec!["B.".to_owned()]);
    assert_eq!(hunk.old_range, 1..2);
    assert_eq!(hunk.new_range, 1..1);
    assert_eq!(hunk.after, Some(BlockId(1)));
    assert!(hunk.marks.is_empty());
}

#[test]
fn a_changed_word_is_marked_and_reverts_in_place() {
    let paras = current(&["Intro.", "The quick brown fox.", "End."]);
    let comparison =
        compare(&["Intro.", "The slow brown fox.", "End."], &view(&paras)).expect("compare");
    assert_eq!(comparison.hunks.len(), 1);
    let hunk = &comparison.hunks[0];
    assert_eq!(hunk.kind, HunkKind::Changed);
    let marks = &hunk.marks[0];
    assert_eq!(marks.block, BlockId(2));
    assert_eq!(&paras[1].1[marks.inserted[0].clone()], "quick");
    assert_eq!(
        marks.deleted,
        vec![Deletion {
            offset: 4,
            text: "slow".into()
        }]
    );
    assert_eq!(
        hunk.revert(),
        Revert::EditText {
            block: BlockId(2),
            edits: vec![TextEdit {
                range: 4..9,
                replacement: "slow".into()
            }],
        }
    );
}

#[test]
fn edits_to_two_paragraphs_are_two_hunks() {
    let old = ["One two three.", "Four five six.", "Seven."];
    let new = ["One 2 three.", "Four five 6.", "Seven."];
    let paras = current(&new);
    let comparison = compare(&old, &view(&paras)).expect("compare");
    assert_eq!(comparison.hunks.len(), 2);
    assert!(comparison.hunks.iter().all(|h| h.kind == HunkKind::Changed));
    assert_eq!(comparison.hunks[0].new_ids, vec![BlockId(1)]);
    assert_eq!(comparison.hunks[1].new_ids, vec![BlockId(2)]);
    assert_eq!(revert_all(&old, &new), old);
}

#[test]
fn a_split_paragraph_stays_one_hunk() {
    let old = ["First half and second half."];
    let new = ["First half", "and second half."];
    let paras = current(&new);
    let comparison = compare(&old, &view(&paras)).expect("compare");
    assert_eq!(comparison.hunks.len(), 1);
    assert_eq!(comparison.hunks[0].kind, HunkKind::Changed);
    assert_eq!(comparison.hunks[0].new_ids.len(), 2);
    assert!(matches!(
        comparison.hunks[0].revert(),
        Revert::Replace { .. }
    ));
    assert_eq!(revert_all(&old, &new), old);
}

#[test]
fn a_merge_shows_the_removed_break_as_a_newline() {
    let old = ["Alpha.", "Beta."];
    let new = ["Alpha. Beta."];
    let paras = current(&new);
    let comparison = compare(&old, &view(&paras)).expect("compare");
    let deleted: String = comparison.hunks[0].marks[0]
        .deleted
        .iter()
        .map(|d| d.text.as_str())
        .collect();
    assert!(deleted.contains('\n'), "{deleted:?}");
    assert_eq!(revert_all(&old, &new), old);
}

#[test]
fn a_moved_paragraph_is_removed_in_one_place_and_added_in_another() {
    let old = ["A.", "B.", "C.", "D."];
    let new = ["A.", "C.", "D.", "B."];
    let paras = current(&new);
    let comparison = compare(&old, &view(&paras)).expect("compare");
    let kinds: Vec<HunkKind> = comparison.hunks.iter().map(|h| h.kind).collect();
    assert!(kinds.contains(&HunkKind::Removed), "{kinds:?}");
    assert!(kinds.contains(&HunkKind::Added), "{kinds:?}");
    assert_eq!(revert_all(&old, &new), old);
}

#[test]
fn unicode_text_is_cut_on_character_boundaries() {
    let old = ["Café — naïve 日本語 text 🙂 here."];
    let new = ["Café – naïf 日本 text 🙃 here!"];
    let paras = current(&new);
    let comparison = compare(&old, &view(&paras)).expect("compare");
    for mark in comparison.hunks.iter().flat_map(|h| &h.marks) {
        for range in &mark.inserted {
            assert!(new[0].is_char_boundary(range.start) && new[0].is_char_boundary(range.end));
        }
        for deletion in &mark.deleted {
            assert!(new[0].is_char_boundary(deletion.offset));
        }
    }
    assert_eq!(revert_all(&old, &new), old);
}

#[test]
fn too_much_text_is_refused() {
    let paras = current(&["0123456789"]);
    let options = Options {
        max_bytes: 15,
        ..Options::default()
    };
    assert_eq!(
        compare_with(&["0123456789"], &view(&paras), &options),
        Err(CompareError::TooLarge {
            bytes: 20,
            limit: 15
        })
    );
}

#[test]
fn a_chapter_slice_of_a_document_is_compared() {
    use libertum_model::{Block, Paragraph};
    let blocks: Vec<Block> = ["Title", "Body one.", "Body two."]
        .iter()
        .enumerate()
        .map(|(i, t)| Block::Paragraph(Paragraph::new(BlockId(100 + i as u64), *t)))
        .collect();
    let document = Document::from_blocks(blocks);
    let chapter = paragraphs(&document, 1..3);
    assert_eq!(chapter.len(), 2);
    assert_eq!(
        posted_texts(&document, 1..99),
        vec!["Body one.", "Body two."]
    );
    let comparison = compare(&["Body one.", "Body 2."], &chapter).expect("compare");
    assert_eq!(comparison.hunks.len(), 1);
    assert_eq!(comparison.hunks[0].new_ids, vec![BlockId(102)]);
    assert_eq!(comparison.hunks[0].after, Some(BlockId(101)));
}

/// Rough speed check: a 300,000-word book against a lightly edited copy. Run with
/// `cargo test -p libertum-diff --release -- --ignored --nocapture`.
#[test]
#[ignore = "timing; run in release"]
fn a_long_book_compares_quickly() {
    let words = [
        "the", "light", "fell", "across", "her", "hands", "and", "she", "said", "nothing", "while",
        "rain", "kept", "on",
    ];
    let mut seed = 7u64;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) as usize
    };
    let old: Vec<String> = (0..4_000)
        .map(|_| {
            let mut p: Vec<&str> = (0..75).map(|_| words[next() % words.len()]).collect();
            p.push("end.");
            p.join(" ")
        })
        .collect();
    let mut new = old.clone();
    for i in (0..new.len()).step_by(40) {
        new[i] = new[i].replacen("light", "shadow", 1);
    }
    new.insert(1_000, "A wholly new paragraph.".into());
    new.remove(3_000);
    timed(&old, new);
    // The worst case: every paragraph edited, so the whole book is one word-level block.
    let every: Vec<String> = old.iter().map(|p| format!("{p} More.")).collect();
    timed(&old, every);
}

fn timed(old: &[String], new: Vec<String>) {
    let paras: Vec<(BlockId, String)> = new
        .into_iter()
        .enumerate()
        .map(|(i, t)| (BlockId(i as u64), t))
        .collect();
    let started = Instant::now();
    let comparison = compare(old, &view(&paras)).expect("compare");
    eprintln!(
        "{} words, {} hunks, {:?}",
        old.iter().map(|p| p.split(' ').count()).sum::<usize>(),
        comparison.hunks.len(),
        started.elapsed()
    );
}

fn paragraph_strategy() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(vec![
            "a", "b", "cat", "dog", "é", "日本", ",", ".", " ", "  ", "🙂", "-", "x'y",
        ]),
        0..8,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #[test]
    fn reverting_every_hunk_gives_back_the_posted_text(
        old in prop::collection::vec(paragraph_strategy(), 0..8),
        new in prop::collection::vec(paragraph_strategy(), 0..8),
    ) {
        let old_refs: Vec<&str> = old.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new.iter().map(String::as_str).collect();
        prop_assert_eq!(revert_all(&old_refs, &new_refs), old);
    }

    #[test]
    fn small_edits_revert_exactly(
        old in prop::collection::vec(paragraph_strategy(), 1..8),
        edits in prop::collection::vec((0usize..8, paragraph_strategy(), 0u8..3), 0..4),
    ) {
        let mut new = old.clone();
        for (at, text, kind) in edits {
            let at = at % (new.len() + 1);
            match kind {
                0 => new.insert(at, text),
                1 if at < new.len() => { new.remove(at); }
                _ if at < new.len() => new[at].push_str(&text),
                _ => {}
            }
        }
        let old_refs: Vec<&str> = old.iter().map(String::as_str).collect();
        let new_refs: Vec<&str> = new.iter().map(String::as_str).collect();
        let paras = current(&new_refs);
        let comparison = compare(&old_refs, &view(&paras)).expect("compare");
        prop_assert_eq!(comparison.hunks.is_empty(), old == new);
        for mark in comparison.hunks.iter().flat_map(|h| &h.marks) {
            let text = &paras.iter().find(|(id, _)| *id == mark.block).expect("block").1;
            for range in &mark.inserted {
                prop_assert!(range.end <= text.len() && text.is_char_boundary(range.start)
                    && text.is_char_boundary(range.end));
            }
        }
        prop_assert_eq!(revert_all(&old_refs, &new_refs), old);
    }
}
