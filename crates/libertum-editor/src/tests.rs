use std::time::{Duration, Instant};

use libertum_model::{
    BlockId, Borders, DateTimeContent, DateTimeFormat, DateTimeSource, Document, HfKind, HfVariant,
    Ligatures, Margins, Orientation, PageSetup, PaperSize, Paragraph,
};

use super::*;
use crate::objects::format_date_time;
use libertum_model::CrossRefKind;

fn doc(texts: &[&str]) -> Document {
    Document::from_paragraphs(
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| Paragraph::new(BlockId(i as u64 + 1), *t))
            .collect(),
    )
}

fn editor(texts: &[&str]) -> Editor {
    Editor::new(doc(texts))
}

fn text_at(ed: &Editor, index: usize) -> String {
    ed.document()
        .paragraph_at(index)
        .expect("paragraph")
        .text()
        .to_owned()
}

fn pos(ed: &Editor, index: usize, offset: usize) -> DocPosition {
    DocPosition {
        block: ed.document().paragraph_at(index).expect("paragraph").id,
        offset,
    }
}

#[test]
fn typing_inserts_and_undo_restores() {
    let mut ed = editor(&[""]);
    let before = ed.document().clone();
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.insert_text("hello").unwrap();
    assert_eq!(text_at(&ed, 0), "hello");
    assert_eq!(ed.caret(), pos(&ed, 0, 5));
    assert_eq!(ed.document().word_count(), 1);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&before));
    assert_eq!(text_at(&ed, 0), "");
    ed.redo().unwrap();
    assert_eq!(text_at(&ed, 0), "hello");
}

#[test]
fn newline_splits_and_backspace_at_start_merges() {
    let mut ed = editor(&["abcd"]);
    ed.set_caret(pos(&ed, 0, 2), false);
    ed.insert_text("\n").unwrap();
    assert_eq!(text_at(&ed, 0), "ab");
    assert_eq!(text_at(&ed, 1), "cd");
    assert_eq!(ed.caret(), pos(&ed, 1, 0));

    ed.delete_backward().unwrap();
    assert_eq!(text_at(&ed, 0), "abcd");
    assert_eq!(ed.document().len(), 1);
    assert_eq!(ed.caret(), pos(&ed, 0, 2));

    ed.undo().unwrap();
    assert_eq!(ed.document().len(), 2);
    ed.undo().unwrap();
    assert_eq!(ed.document().len(), 1);
    assert_eq!(text_at(&ed, 0), "abcd");
}

#[test]
fn delete_forward_at_end_merges_next_block() {
    let mut ed = editor(&["one", "two"]);
    ed.set_caret(pos(&ed, 0, 3), false);
    ed.delete_forward().unwrap();
    assert_eq!(ed.document().len(), 1);
    assert_eq!(text_at(&ed, 0), "onetwo");
    assert_eq!(ed.caret(), pos(&ed, 0, 3));
}

#[test]
fn selecting_across_blocks_and_deleting_joins_the_tails() {
    let mut ed = editor(&["alpha", "beta", "gamma"]);
    ed.set_selection(pos(&ed, 0, 2), pos(&ed, 2, 2));
    assert_eq!(ed.selected_text().as_deref(), Some("pha\nbeta\nga"));
    ed.delete_forward().unwrap();
    assert_eq!(ed.document().len(), 1);
    assert_eq!(text_at(&ed, 0), "al mma".replace(' ', ""));
    assert_eq!(ed.caret(), pos(&ed, 0, 2));
    ed.undo().unwrap();
    assert_eq!(ed.document().len(), 3);
    assert_eq!(text_at(&ed, 0), "alpha");
    assert_eq!(text_at(&ed, 1), "beta");
    assert_eq!(text_at(&ed, 2), "gamma");
}

#[test]
fn select_all_delete_leaves_one_empty_paragraph() {
    let mut ed = editor(&["a", "b", "c"]);
    ed.select_all();
    ed.delete_forward().unwrap();
    assert_eq!(ed.document().len(), 1);
    assert_eq!(text_at(&ed, 0), "");
    assert_eq!(ed.caret(), pos(&ed, 0, 0));
}

#[test]
fn typing_coalesces_into_one_undo_step() {
    let mut ed = editor(&[""]);
    let t0 = Instant::now();
    ed.insert_text_at(t0, "a").unwrap();
    ed.insert_text_at(t0 + Duration::from_millis(50), "b")
        .unwrap();
    ed.insert_text_at(t0 + Duration::from_millis(100), "c")
        .unwrap();
    assert_eq!(text_at(&ed, 0), "abc");
    assert_eq!(ed.history().undo_len(), 1);
    ed.undo().unwrap();
    assert_eq!(text_at(&ed, 0), "");
}

#[test]
fn whitespace_and_pauses_break_typing_runs() {
    let mut ed = editor(&[""]);
    let t0 = Instant::now();
    ed.insert_text_at(t0, "a").unwrap();
    ed.insert_text_at(t0 + Duration::from_millis(50), "b")
        .unwrap();
    ed.insert_text_at(t0 + Duration::from_millis(100), " ")
        .unwrap();
    ed.insert_text_at(t0 + Duration::from_millis(150), "c")
        .unwrap();
    assert_eq!(ed.history().undo_len(), 3, "ab | space | c");
    ed.insert_text_at(t0 + Duration::from_secs(5), "d").unwrap();
    assert_eq!(ed.history().undo_len(), 4);
}

#[test]
fn backspace_coalesces_and_removes_graphemes() {
    let mut ed = editor(&["abc"]);
    ed.set_caret(pos(&ed, 0, 3), false);
    let t0 = Instant::now();
    ed.delete_backward_at(t0).unwrap();
    ed.delete_backward_at(t0 + Duration::from_millis(40))
        .unwrap();
    assert_eq!(text_at(&ed, 0), "a");
    assert_eq!(ed.history().undo_len(), 1);
    ed.undo().unwrap();
    assert_eq!(text_at(&ed, 0), "abc");
}

#[test]
fn grapheme_movement_handles_zwj_clusters() {
    // One grapheme: woman + ZWJ + girl.
    let family = "\u{1F469}\u{200D}\u{1F467}";
    let text = format!("a{family}b");
    let mut ed = editor(&[text.as_str()]);
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.move_right(false);
    assert_eq!(ed.caret().offset, 1);
    ed.move_right(false);
    assert_eq!(
        ed.caret().offset,
        1 + family.len(),
        "the ZWJ cluster moves as one grapheme"
    );
    ed.move_right(false);
    assert_eq!(ed.caret().offset, text.len());
    ed.move_left(false);
    assert_eq!(ed.caret().offset, 1 + family.len());
}

#[test]
fn word_and_line_movement() {
    let mut ed = editor(&["hello world"]);
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.move_word_right(false);
    assert_eq!(ed.caret().offset, 5);
    ed.move_word_right(false);
    assert_eq!(ed.caret().offset, 6);
    ed.move_word_right(false);
    assert_eq!(ed.caret().offset, 11);
    ed.move_word_left(false);
    assert_eq!(ed.caret().offset, 6);
    ed.move_line_start(false);
    assert_eq!(ed.caret().offset, 0);
    ed.move_line_end(false);
    assert_eq!(ed.caret().offset, 11);
}

#[test]
fn shift_movement_extends_selection() {
    let mut ed = editor(&["abcdef"]);
    ed.set_caret(pos(&ed, 0, 2), false);
    ed.move_right(true);
    ed.move_right(true);
    assert_eq!(
        ed.ordered_selection(),
        Some((pos(&ed, 0, 2), pos(&ed, 0, 4)))
    );
    // Moving without shift collapses to the ordered edge.
    ed.move_left(false);
    assert!(!ed.has_selection());
    assert_eq!(ed.caret(), pos(&ed, 0, 2));
}

#[test]
fn paragraph_and_document_movement() {
    let mut ed = editor(&["abc", "defgh"]);
    ed.set_caret(pos(&ed, 0, 2), false);
    ed.move_paragraph(1, false);
    assert_eq!(ed.caret(), pos(&ed, 1, 2));
    ed.move_paragraph(1, false);
    assert_eq!(ed.caret(), pos(&ed, 1, 2), "no-op past the end");
    ed.move_doc_start(false);
    assert_eq!(ed.caret(), pos(&ed, 0, 0));
    ed.move_doc_end(false);
    assert_eq!(ed.caret(), pos(&ed, 1, 5));
}

#[test]
fn copy_cut_and_paste_are_plain_text() {
    let mut ed = editor(&["one", "two"]);
    ed.set_selection(pos(&ed, 0, 1), pos(&ed, 1, 2));
    assert_eq!(ed.copy().as_deref(), Some("ne\ntw"));
    let cut = ed.cut().unwrap();
    assert_eq!(cut.as_deref(), Some("ne\ntw"));
    assert_eq!(text_at(&ed, 0), "oo");
    ed.set_caret(pos(&ed, 0, 1), false);
    ed.paste("ne\ntw").unwrap();
    assert_eq!(ed.document().len(), 2);
    assert_eq!(text_at(&ed, 0), "one");
    assert_eq!(text_at(&ed, 1), "two");
}

#[test]
fn paste_normalises_windows_newlines() {
    let mut ed = editor(&[""]);
    ed.paste("a\r\nb\rc").unwrap();
    assert_eq!(ed.document().len(), 3);
    assert_eq!(text_at(&ed, 0), "a");
    assert_eq!(text_at(&ed, 1), "b");
    assert_eq!(text_at(&ed, 2), "c");
}

#[test]
fn delete_word_backward_and_forward() {
    let mut ed = editor(&["hello big world"]);
    ed.set_caret(pos(&ed, 0, 9), false); // after "big"
    ed.delete_word_backward().unwrap();
    assert_eq!(text_at(&ed, 0), "hello  world");
    ed.set_caret(pos(&ed, 0, 6), false);
    ed.delete_word_forward().unwrap();
    assert_eq!(text_at(&ed, 0), "hello world");
}

#[test]
fn thousand_undo_steps_through_the_editor() {
    let mut ed = editor(&[""]);
    ed.set_caret(pos(&ed, 0, 0), false);
    let t0 = Instant::now();
    // Whitespace between every character keeps each insertion a separate undo step.
    for i in 0..500 {
        ed.insert_text_at(t0 + Duration::from_millis(i as u64), "x")
            .unwrap();
        ed.insert_text_at(t0 + Duration::from_millis(i as u64), " ")
            .unwrap();
        ed.set_caret(
            DocPosition {
                block: ed.caret().block,
                offset: ed.caret().offset,
            },
            false,
        );
    }
    assert_eq!(text_at(&ed, 0), "x ".repeat(500));
    assert!(ed.history().undo_len() >= 1000);
    for _ in 0..1000 {
        ed.undo().unwrap();
    }
    assert_eq!(text_at(&ed, 0), "");
}

// ---------------------------------------------------------------- page breaks, sections, TOC

use libertum_model::{
    Align, LineSpacing, ParaFormat, ParaProps, Section, StyleId, StyleSheet, VerticalAlign,
};

/// Title, "Contents", one TOC entry, then two chapters (each a heading plus body). The first
/// chapter starts section 1. Ids are 1..=7 in order.
fn book() -> Editor {
    let chapter = |id: u64, text: &str, section_start: bool| {
        Paragraph::new(BlockId(id), text).with_props(ParaProps {
            outline_level: Some(1),
            format: ParaFormat {
                align: Some(Align::Centre),
                ..ParaFormat::default()
            },
            page_break_before: true,
            section_start,
            ..ParaProps::default()
        })
    };
    let entry = |id: u64, target: u64, text: &str| {
        Paragraph::new(BlockId(id), text).with_props(ParaProps {
            toc_target: Some(BlockId(target)),
            ..ParaProps::default()
        })
    };
    let mut doc = Document::from_paragraphs(vec![
        Paragraph::new(BlockId(1), "Title"),
        Paragraph::new(BlockId(2), "Contents"),
        entry(3, 4, "Chapter One"),
        chapter(4, "Chapter One", true),
        Paragraph::new(BlockId(5), "It began."),
        chapter(6, "Chapter Two", false),
        Paragraph::new(BlockId(7), "It ended."),
    ]);
    doc.sections.push(Section::default());
    let mut ed = Editor::new(doc);
    // Chapter Two is missing from the TOC until it is refreshed.
    let changed = ed.refresh_table_of_contents().expect("refresh");
    assert!(changed.structural);
    assert!(!ed.can_undo(), "a refresh on open is not an undo step");
    ed
}

fn toc(ed: &Editor) -> Vec<(BlockId, String)> {
    ed.document()
        .body()
        .iter()
        .filter_map(|b| b.as_paragraph())
        .filter_map(|p| p.props.toc_target.map(|t| (t, p.text().to_owned())))
        .collect()
}

#[test]
fn toc_lists_every_chapter_heading() {
    let ed = book();
    assert_eq!(
        toc(&ed),
        vec![
            (BlockId(4), "Chapter One".to_owned()),
            (BlockId(6), "Chapter Two".to_owned())
        ]
    );
}

#[test]
fn renaming_a_heading_updates_its_entry_and_undo_restores_both() {
    let mut ed = book();
    let before = ed.document().clone();
    let heading = ed.document().index_of(BlockId(6)).expect("chapter two");
    ed.set_caret(pos(&ed, heading, "Chapter Two".len()), false);
    let changed = ed.insert_text(": Home").expect("type");
    assert_eq!(toc(&ed)[1].1, "Chapter Two: Home");
    let entry = toc_entry_id(&ed, 1);
    assert!(
        changed.dirty.contains(&entry),
        "layout must repaint the entry"
    );
    while ed.can_undo() && !ed.document().content_eq(&before) {
        ed.undo().expect("undo");
    }
    assert!(ed.document().content_eq(&before));
}

fn toc_entry_id(ed: &Editor, k: usize) -> BlockId {
    ed.document()
        .body()
        .iter()
        .filter_map(|b| b.as_paragraph())
        .filter(|p| p.props.toc_target.is_some())
        .nth(k)
        .expect("entry")
        .id
}

#[test]
fn adding_and_removing_a_chapter_rebuilds_the_toc_in_one_undo_step() {
    let mut ed = book();
    let before = ed.document().clone();
    // Merging Chapter Two's heading into the body above removes it as a heading.
    let heading = ed.document().index_of(BlockId(6)).expect("chapter two");
    ed.set_caret(pos(&ed, heading, 0), false);
    ed.delete_backward().expect("remove the page break");
    ed.delete_backward().expect("merge the heading away");
    assert_eq!(toc(&ed), vec![(BlockId(4), "Chapter One".to_owned())]);
    ed.undo().expect("undo merge");
    ed.undo().expect("undo break removal");
    assert!(
        ed.document().content_eq(&before),
        "TOC and heading come back together"
    );
}

#[test]
fn toc_entries_are_read_only() {
    let mut ed = book();
    let entry = ed.document().index_of(toc_entry_id(&ed, 0)).expect("entry");
    ed.set_caret(pos(&ed, entry, 3), false);
    assert_eq!(ed.insert_text("x"), Err(EditorError::ReadOnly));
    assert_eq!(ed.delete_backward(), Err(EditorError::ReadOnly));
    assert_eq!(ed.insert_page_break(), Err(EditorError::ReadOnly));
    // Merging the paragraph after the TOC into the last entry is refused too.
    let contents = ed.document().index_of(BlockId(2)).expect("contents");
    ed.set_caret(pos(&ed, contents, "Contents".len()), false);
    assert_eq!(ed.delete_forward(), Err(EditorError::ReadOnly));
    // A selection that covers an entry cannot delete it.
    ed.set_selection(pos(&ed, contents, 0), pos(&ed, entry + 1, 2));
    assert_eq!(ed.delete_backward(), Err(EditorError::ReadOnly));
}

#[test]
fn a_section_start_cannot_be_merged_away() {
    let mut ed = book();
    let chapter = ed.document().index_of(BlockId(4)).expect("chapter one");
    ed.set_caret(pos(&ed, chapter, 0), false);
    // The last TOC entry sits above, so this is refused as read-only first; either way the
    // section survives.
    assert!(ed.delete_backward().is_err());
    let body = ed.document().index_of(BlockId(5)).expect("body");
    ed.set_selection(pos(&ed, chapter - 1, 0), pos(&ed, body, 2));
    assert!(ed.delete_backward().is_err());
    assert!(
        ed.document()
            .para_props(BlockId(4))
            .is_some_and(|p| p.section_start)
    );
}

#[test]
fn page_break_splits_and_starts_a_new_page_and_backspace_removes_it() {
    let mut ed = editor(&["Hello world"]);
    ed.set_caret(pos(&ed, 0, 5), false);
    ed.insert_page_break().expect("page break");
    assert_eq!(text_at(&ed, 0), "Hello");
    assert_eq!(text_at(&ed, 1), " world");
    let second = ed.document().paragraph_at(1).expect("second").props;
    assert!(second.page_break_before);
    assert!(
        !ed.document()
            .paragraph_at(0)
            .expect("first")
            .props
            .page_break_before
    );
    assert_eq!(ed.caret(), pos(&ed, 1, 0));

    // Backspace at the start removes the break but keeps the paragraphs apart.
    ed.delete_backward().expect("remove break");
    assert_eq!(ed.document().len(), 2);
    assert!(
        !ed.document()
            .paragraph_at(1)
            .expect("second")
            .props
            .page_break_before
    );
    // A second Backspace merges as usual.
    ed.delete_backward().expect("merge");
    assert_eq!(text_at(&ed, 0), "Hello world");

    // The whole sequence undoes cleanly.
    ed.undo().expect("undo merge");
    ed.undo().expect("undo break removal");
    ed.undo().expect("undo page break");
    assert_eq!(ed.document().len(), 1);
    assert_eq!(text_at(&ed, 0), "Hello world");
}

#[test]
fn enter_after_a_heading_starts_a_body_paragraph() {
    let mut ed = book();
    let heading = ed.document().index_of(BlockId(4)).expect("chapter one");
    ed.set_caret(pos(&ed, heading, "Chapter One".len()), false);
    ed.insert_text("\n").expect("enter");
    let next = ed
        .document()
        .paragraph_at(heading + 1)
        .expect("new paragraph");
    assert_eq!(next.props.outline_level, None);
    assert!(next.props.format.is_empty());
    assert!(!next.props.page_break_before && !next.props.section_start);
    assert_eq!(toc(&ed).len(), 2, "the new paragraph is not a chapter");
}

#[test]
fn caret_section_and_set_section_are_undoable() {
    let mut ed = book();
    let body = ed.document().index_of(BlockId(5)).expect("body");
    ed.set_caret(pos(&ed, body, 0), false);
    assert_eq!(ed.caret_section(), 1);
    ed.set_caret(pos(&ed, 0, 0), false);
    assert_eq!(ed.caret_section(), 0);
    let numbered = Section {
        page_numbers: Some(libertum_model::PageNumberPos::BOTTOM_CENTRE),
        ..Section::default()
    };
    let set = ed.set_section(1, numbered.clone()).expect("set section");
    assert!(set.sections);
    assert_eq!(ed.document().sections[1], numbered);
    ed.undo().expect("undo");
    assert_eq!(ed.document().sections[1], Section::default());
}

#[test]
fn columns_and_gutter_edit_one_section_and_undo() {
    let mut ed = book();
    assert_eq!(ed.document().sections.len(), 2);
    let body = ed.document().index_of(BlockId(5)).expect("body");
    ed.set_caret(pos(&ed, body, 0), false);
    assert_eq!(ed.caret_section(), 1);

    let change = ed.set_columns(1, 3).expect("set columns");
    assert!(change.sections);
    assert_eq!(ed.document().sections[1].columns, 3);
    assert_eq!(
        ed.document().sections[0].columns,
        1,
        "section 0 keeps its own columns"
    );

    let change = ed.set_column_gutter(1, 18.0).expect("set gutter");
    assert!(change.sections);
    assert!((ed.document().sections[1].column_gutter_pt - 18.0).abs() < f32::EPSILON);

    ed.undo().expect("undo gutter");
    assert!(
        (ed.document().sections[1].column_gutter_pt - libertum_model::DEFAULT_COLUMN_GUTTER_PT)
            .abs()
            < f32::EPSILON
    );
    ed.undo().expect("undo columns");
    assert_eq!(ed.document().sections[1].columns, 1);
}

#[test]
fn columns_are_clamped_and_redundant_changes_are_not_undo_steps() {
    let mut ed = book();
    // Already one column: nothing changes and no undo entry is recorded.
    let before = ed.document().clone();
    let change = ed.set_columns(1, 1).expect("no-op");
    assert!(change.is_empty());
    assert!(ed.document().content_eq(&before));
    // Above the maximum clamps to 12 rather than storing an impossible count.
    let change = ed.set_columns(1, 99).expect("clamped");
    assert!(change.sections);
    assert_eq!(
        ed.document().sections[1].columns,
        libertum_model::MAX_COLUMNS
    );
    ed.undo().expect("undo the clamp");
    assert_eq!(ed.document().sections[1].columns, 1);
}

#[test]
fn page_setup_applies_to_every_section_in_one_undo_step() {
    let mut ed = book();
    assert_eq!(ed.document().sections.len(), 2, "the book has two sections");
    let mut numbered = ed.document().sections[1].clone();
    numbered.page_numbers = Some(libertum_model::PageNumberPos::BOTTOM_CENTRE);
    numbered.first_page_number = Some(5);
    ed.set_section(1, numbered).expect("number section 1");
    let before = ed.document().clone();
    let letter = PageSetup {
        paper: PaperSize::Letter,
        orientation: Orientation::Landscape,
        margins: Margins::uniform(54.0),
    };
    let change = ed.set_page_setup(letter).expect("page setup");
    assert!(change.sections);
    assert!(
        ed.document().sections.iter().all(|s| s.page == letter),
        "every section carries the document-wide setup"
    );
    assert_eq!(ed.page_setup(), letter);
    assert_eq!(
        ed.document().sections[1].page_numbers,
        Some(libertum_model::PageNumberPos::BOTTOM_CENTRE),
        "section settings other than page setup are kept"
    );
    assert_eq!(ed.document().sections[1].first_page_number, Some(5));
    ed.undo().expect("undo");
    assert_eq!(ed.document().sections, before.sections);
    assert_eq!(ed.page_setup(), PageSetup::a4());
}

#[test]
fn page_setup_with_the_same_value_is_a_no_op() {
    let mut ed = book();
    let change = ed.set_page_setup(PageSetup::a4()).expect("page setup");
    assert!(change.is_empty());
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| s.page == PageSetup::a4())
    );
}

#[test]
fn header_and_footer_text_is_one_undo_step_per_typed_run() {
    let mut ed = editor(&["Body"]);
    let section = Section::default();
    assert!(!section.headers.enabled());
    // Typing into the header creates it.
    let change = ed
        .set_header_footer_text(0, HfKind::Header, HfVariant::Default, "Draft".into())
        .expect("header");
    assert!(change.sections);
    assert_eq!(
        ed.document().sections[0].headers.default,
        Some(libertum_model::HeaderFooter::new("Draft"))
    );
    // A second keystroke within the typing window coalesces into the same undo step.
    ed.set_header_footer_text(0, HfKind::Header, HfVariant::Default, "Draft 2".into())
        .expect("header");
    assert_eq!(ed.history().undo_len(), 1, "typing coalesces");
    ed.undo().expect("undo");
    assert!(
        !ed.document().sections[0].headers.enabled(),
        "undo returns to no header"
    );

    ed.set_header_footer_text(0, HfKind::Footer, HfVariant::Default, "{page}".into())
        .expect("footer");
    assert_eq!(
        ed.document().sections[0]
            .footers
            .default
            .as_ref()
            .map(|f| f.text.as_str()),
        Some("{page}")
    );
}

#[test]
fn turning_a_header_off_keeps_its_first_page_text() {
    let mut ed = editor(&["Body"]);
    ed.set_header_footer_text(0, HfKind::Header, HfVariant::First, "Chapter one".into())
        .expect("first header");
    ed.set_different_first_page(0, true)
        .expect("different first");
    assert!(ed.document().sections[0].headers.different_first);
    ed.set_header_footer_enabled(0, HfKind::Header, false)
        .expect("off");
    assert!(!ed.document().sections[0].headers.enabled());
    ed.set_header_footer_enabled(0, HfKind::Header, true)
        .expect("on");
    assert_eq!(
        ed.document().sections[0]
            .headers
            .first
            .as_ref()
            .map(|h| h.text.as_str()),
        Some("Chapter one"),
        "turning the header back on restores its variants"
    );
    different_first_page_is_undoable(&mut ed);
}

/// Helper kept separate so the test above stays readable.
fn different_first_page_is_undoable(ed: &mut Editor) {
    let before = ed.document().sections[0].headers.different_first;
    ed.set_different_first_page(0, false).expect("off");
    assert!(!ed.document().sections[0].headers.different_first);
    ed.undo().expect("undo");
    assert_eq!(ed.document().sections[0].headers.different_first, before);
}

#[test]
fn typed_header_edits_to_different_variants_are_separate_undo_steps() {
    let mut ed = editor(&["Body"]);
    ed.set_different_first_page(0, true).expect("first page");
    ed.set_header_footer_text(0, HfKind::Header, HfVariant::Default, "Default".into())
        .expect("default");
    let default_only = ed.history().undo_len();
    ed.set_header_footer_text(0, HfKind::Header, HfVariant::First, "First".into())
        .expect("first");
    assert_eq!(
        ed.history().undo_len(),
        default_only + 1,
        "a different variant starts a new undo step"
    );
    // Undo and redo restore the second variant without losing the first.
    ed.undo().expect("undo");
    assert!(
        !ed.document().sections[0]
            .headers
            .first
            .as_ref()
            .is_some_and(|h| h.text == "First")
    );
    ed.redo().expect("redo");
    assert!(
        ed.document().sections[0]
            .headers
            .first
            .as_ref()
            .is_some_and(|h| h.text == "First")
    );
    assert_eq!(
        ed.document().sections[0]
            .headers
            .default
            .as_ref()
            .map(|h| h.text.as_str()),
        Some("Default"),
        "redo does not lose the other variant"
    );
}

#[test]
fn header_footer_distances_apply_to_every_section() {
    let mut ed = book();
    assert_eq!(ed.document().sections.len(), 2);
    let change = ed
        .set_header_footer_distance(HfKind::Header, 24.0)
        .expect("distance");
    assert!(change.sections);
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| (s.header_distance_pt - 24.0).abs() < 0.01)
    );
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| (s.footer_distance_pt - 36.0).abs() < 0.01),
        "the other side is untouched"
    );
    ed.undo().expect("undo");
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| (s.header_distance_pt - 36.0).abs() < 0.01)
    );
    // A repeated value is not an undo step.
    assert!(
        ed.set_header_footer_distance(HfKind::Header, 36.0)
            .expect("no-op")
            .is_empty()
    );
}

#[test]
fn document_ligature_default_is_one_undo_step() {
    let mut ed = editor(&["fi fl"]);
    assert_eq!(ed.document().styles.defaults.ligatures, Ligatures::Standard);
    let change = ed
        .set_document_ligatures(Ligatures::None)
        .expect("ligatures");
    assert!(
        change.all_styles,
        "every paragraph resolves through the defaults"
    );
    assert_eq!(ed.document().styles.defaults.ligatures, Ligatures::None);
    ed.undo().expect("undo");
    assert_eq!(ed.document().styles.defaults.ligatures, Ligatures::Standard);
}

#[test]
fn paragraph_borders_apply_to_the_selection_and_undo() {
    let mut ed = editor(&["first", "second", "third"]);
    ed.set_selection(pos(&ed, 0, 0), pos(&ed, 2, 0));
    let change = ed
        .format_paragraphs(&ParaFormat {
            borders: Some(Borders::line(1.0)),
            ..ParaFormat::default()
        })
        .expect("borders");
    assert_eq!(change.dirty.len(), 3);
    for index in 0..3 {
        let p = ed.document().paragraph_at(index).expect("paragraph");
        assert_eq!(p.props.format.borders.map(|b| b.width_pt), Some(1.0));
    }
    assert_eq!(
        ed.para_summary().borders.map(|b| b.map(|b| b.width_pt)),
        Some(Some(1.0))
    );

    ed.format_paragraphs(&ParaFormat {
        borders: Some(Borders::none()),
        ..ParaFormat::default()
    })
    .expect("clear");
    assert_eq!(
        ed.para_summary().borders,
        Some(None),
        "no border on any selected paragraph"
    );
    ed.undo().expect("undo clear");
    assert_eq!(
        ed.para_summary().borders.map(|b| b.map(|b| b.width_pt)),
        Some(Some(1.0))
    );
}

// ---------------------------------------------------------------- character formatting

#[test]
fn bold_over_a_selection_spanning_paragraphs_and_undo_restores_it() {
    let mut ed = editor(&["first paragraph", "second paragraph"]);
    let before = ed.document().clone();
    ed.set_selection(pos(&ed, 0, 6), pos(&ed, 1, 6));
    ed.toggle_bold().expect("bold");
    assert_eq!(ed.char_summary().is_bold(), Some(true));
    let p0 = ed.document().paragraph_at(0).expect("p0");
    assert_eq!(p0.runs().len(), 1);
    assert_eq!(p0.runs()[0].range, 6..15);
    let p1 = ed.document().paragraph_at(1).expect("p1");
    assert_eq!(p1.runs()[0].range, 0..6);
    // Toggling again removes it.
    ed.toggle_bold().expect("unbold");
    assert_eq!(ed.char_summary().is_bold(), Some(false));
    ed.undo().expect("undo unbold");
    ed.undo().expect("undo bold");
    assert!(ed.document().content_eq(&before));
}

#[test]
fn a_partly_bold_selection_is_mixed_and_toggles_to_bold() {
    let mut ed = editor(&["one two three"]);
    ed.set_selection(pos(&ed, 0, 4), pos(&ed, 0, 7));
    ed.toggle_bold().expect("bold 'two'");
    ed.set_selection(pos(&ed, 0, 0), pos(&ed, 0, 13));
    assert_eq!(ed.char_summary().is_bold(), None, "mixed");
    ed.toggle_bold().expect("bold all");
    assert_eq!(ed.char_summary().is_bold(), Some(true));
}

#[test]
fn caret_formatting_applies_to_the_next_typed_text_only_at_that_spot() {
    let mut ed = editor(&["plain "]);
    ed.set_caret(pos(&ed, 0, 6), false);
    ed.toggle_italic().expect("pending italic");
    assert_eq!(
        ed.char_summary().italic,
        Some(true),
        "the inspector shows it"
    );
    assert_eq!(
        ed.document().paragraph_at(0).unwrap().runs().len(),
        0,
        "nothing changed yet"
    );
    ed.insert_text("x").expect("type");
    ed.insert_text("y").expect("type");
    let p = ed.document().paragraph_at(0).unwrap();
    assert_eq!(p.text(), "plain xy");
    assert_eq!(p.runs().len(), 1);
    assert_eq!(p.runs()[0].range, 6..8, "both typed letters are italic");
    // Moving away drops a pending format.
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.toggle_bold().expect("pending bold");
    ed.set_caret(pos(&ed, 0, 3), false);
    ed.insert_text("z").expect("type");
    assert_eq!(ed.char_summary().is_bold(), Some(false));
}

#[test]
fn size_and_colour_patches_resolve_against_the_paragraph_style() {
    use libertum_model::{CharFormat, StyleId};
    let ed = editor(&["Heading text"]);
    let id = ed.document().paragraph_at(0).unwrap().id;
    let mut props = ed.document().para_props(id).unwrap();
    props.style = StyleId::HEADING_1;
    let mut doc = ed.document().clone();
    doc.apply(Operation::SetParaProps { block: id, props })
        .unwrap();
    let mut ed = Editor::new(doc);
    ed.set_selection(pos(&ed, 0, 0), pos(&ed, 0, 7));
    assert_eq!(ed.char_summary().is_bold(), Some(true), "Heading 1 is bold");
    ed.format_selection(&CharFormat {
        colour: Some([200, 0, 0, 255]),
        ..CharFormat::default()
    })
    .expect("colour");
    let sum = ed.char_summary();
    assert_eq!(sum.colour, Some([200, 0, 0, 255]));
    assert_eq!(sum.is_bold(), Some(true), "still bold from the style");
}

// ---------------------------------------------------------------- paragraph formatting

/// Three body paragraphs; a selection from the middle of the first into the second.
fn three_paragraphs() -> Editor {
    let mut ed = Editor::new(Document::from_paragraphs(vec![
        Paragraph::new(BlockId(1), "First paragraph."),
        Paragraph::new(BlockId(2), "Second paragraph."),
        Paragraph::new(BlockId(3), "Third paragraph."),
    ]));
    ed.set_selection(
        DocPosition {
            block: BlockId(1),
            offset: 3,
        },
        DocPosition {
            block: BlockId(2),
            offset: 4,
        },
    );
    ed
}

#[test]
fn paragraph_formatting_applies_to_every_touched_paragraph_and_undoes_exactly() {
    let mut ed = three_paragraphs();
    let before = ed.document().clone();
    let patch = ParaFormat {
        align: Some(Align::Justify),
        indent_first_pt: Some(18.0),
        line_spacing: Some(LineSpacing::Multiple(2.0)),
        space_after_pt: Some(12.0),
        keep_with_next: Some(true),
        ..ParaFormat::default()
    };
    let change = ed.format_paragraphs(&patch).expect("format");
    assert_eq!(change.dirty.len(), 2, "first and second only");
    let sum = ed.para_summary();
    assert_eq!(sum.align, Some(Align::Justify));
    assert_eq!(sum.indent_first_pt, Some(18.0));
    assert_eq!(sum.line_spacing, Some(LineSpacing::Multiple(2.0)));
    assert_eq!(sum.keep_with_next, Some(true));
    assert!(sum.differs_from_style);
    let third = ed.document().paragraph(BlockId(3)).expect("third");
    assert!(third.props.format.is_empty());
    ed.undo().expect("undo");
    assert!(ed.document().content_eq(&before));
}

#[test]
fn a_selection_with_different_alignments_reads_as_mixed() {
    let mut ed = three_paragraphs();
    ed.set_caret(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        false,
    );
    ed.format_paragraphs(&ParaFormat {
        align: Some(Align::Centre),
        ..ParaFormat::default()
    })
    .expect("centre");
    assert_eq!(ed.para_summary().align, Some(Align::Centre));
    ed.set_selection(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        DocPosition {
            block: BlockId(3),
            offset: 1,
        },
    );
    let sum = ed.para_summary();
    assert_eq!(sum.align, None, "centred and start-aligned");
    assert_eq!(sum.style, Some(StyleId::BODY));
}

#[test]
fn indent_steps_by_half_an_inch_and_stops_at_the_margin() {
    let mut ed = three_paragraphs();
    ed.indent(1).expect("indent");
    ed.indent(1).expect("indent");
    assert_eq!(ed.para_summary().indent_left_pt, Some(2.0 * INDENT_STEP_PT));
    for _ in 0..3 {
        ed.indent(-1).expect("outdent");
    }
    assert_eq!(ed.para_summary().indent_left_pt, Some(0.0));
}

#[test]
fn applying_a_heading_style_adds_it_to_the_table_of_contents_and_undo_removes_it() {
    let mut ed = book();
    let body = ed
        .document()
        .index_of(BlockId(5))
        .expect("chapter one body");
    ed.set_caret(pos(&ed, body, 0), false);
    let entries = toc(&ed).len();
    ed.set_paragraph_style(StyleId::HEADING_1).expect("style");
    let p = ed.document().paragraph(BlockId(5)).expect("body");
    assert_eq!(p.props.style, StyleId::HEADING_1);
    assert_eq!(p.props.outline_level, Some(1));
    assert_eq!(toc(&ed).len(), entries + 1);
    ed.set_paragraph_style(StyleId::BODY).expect("back to body");
    assert_eq!(toc(&ed).len(), entries);
    ed.undo().expect("undo");
    assert_eq!(toc(&ed).len(), entries + 1);
    ed.undo().expect("undo");
    assert_eq!(toc(&ed).len(), entries);
}

#[test]
fn return_at_the_end_of_a_title_starts_body_text() {
    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "My Book")]);
    doc.styles = StyleSheet::default();
    let mut ed = Editor::new(doc);
    ed.set_paragraph_style(StyleId::TITLE).expect("title");
    ed.set_caret(
        DocPosition {
            block: BlockId(1),
            offset: "My Book".len(),
        },
        false,
    );
    ed.insert_text("\n").expect("return");
    let next = ed.document().paragraph_at(1).expect("new paragraph");
    assert_eq!(next.props.style, StyleId::BODY);
    // Return in the middle keeps the style.
    ed.set_caret(
        DocPosition {
            block: BlockId(1),
            offset: 2,
        },
        false,
    );
    ed.insert_text("\n").expect("return");
    let tail = ed.document().paragraph_at(1).expect("tail");
    assert_eq!(tail.props.style, StyleId::TITLE);
}

#[test]
fn update_style_moves_direct_formatting_into_the_style_and_undo_restores_it() {
    let mut ed = three_paragraphs();
    ed.set_caret(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        false,
    );
    ed.format_paragraphs(&ParaFormat {
        space_after_pt: Some(20.0),
        ..ParaFormat::default()
    })
    .expect("spacing");
    // Make the whole first paragraph italic.
    ed.set_selection(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        DocPosition {
            block: BlockId(1),
            offset: "First paragraph.".len(),
        },
    );
    ed.toggle_italic().expect("italic");
    let before = ed.document().clone();
    assert!(ed.para_summary().differs_from_style);
    ed.update_style_from_selection().expect("update");
    let doc = ed.document();
    let first = doc.paragraph(BlockId(1)).expect("first");
    assert!(first.props.format.is_empty() && first.runs().is_empty());
    let body = doc.styles.get(StyleId::BODY).expect("body");
    assert_eq!(body.para.space_after_pt, Some(20.0));
    assert_eq!(body.char.italic, Some(true));
    // Every Body paragraph follows the style.
    let third = doc.paragraph(BlockId(3)).expect("third");
    assert_eq!(doc.styles.resolve_props(&third.props).space_after_pt, 20.0);
    assert!(!ed.para_summary().differs_from_style);
    ed.undo().expect("undo");
    assert!(ed.document().content_eq(&before));
}

#[test]
fn update_style_takes_a_size_set_on_part_of_a_paragraph() {
    let mut ed = three_paragraphs();
    let default_size = ed.document().styles.resolve_char(StyleId::BODY).size_pt;
    // Size 11 on "paragraph" only (not the whole of the first paragraph).
    ed.set_selection(
        DocPosition {
            block: BlockId(1),
            offset: 6,
        },
        DocPosition {
            block: BlockId(1),
            offset: 15,
        },
    );
    ed.format_selection(&CharFormat::sized(14.0)).expect("size");
    let before = ed.document().clone();
    assert!(ed.para_summary().differs_from_style);
    ed.update_style_from_selection().expect("update");
    let doc = ed.document();
    let body = doc.styles.get(StyleId::BODY).expect("body");
    assert_eq!(body.char.size_pt, Some(14.0));
    assert_ne!(default_size, 14.0);
    assert!(doc.paragraph(BlockId(1)).expect("first").runs().is_empty());
    assert!(!ed.para_summary().differs_from_style);
    ed.undo().expect("undo");
    assert!(ed.document().content_eq(&before));
}

#[test]
fn update_style_keeps_formatting_the_selection_does_not_share() {
    let mut ed = three_paragraphs();
    // Whole first paragraph at 14pt, the word "First" also bold.
    ed.set_selection(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        DocPosition {
            block: BlockId(1),
            offset: "First paragraph.".len(),
        },
    );
    ed.format_selection(&CharFormat::sized(14.0)).expect("size");
    ed.set_selection(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        DocPosition {
            block: BlockId(1),
            offset: 5,
        },
    );
    ed.toggle_bold().expect("bold");
    // Caret in the paragraph: the whole paragraph shares only the size.
    ed.set_caret(
        DocPosition {
            block: BlockId(1),
            offset: 3,
        },
        false,
    );
    ed.update_style_from_selection().expect("update");
    let doc = ed.document();
    let body = doc.styles.get(StyleId::BODY).expect("body");
    assert_eq!(body.char.size_pt, Some(14.0));
    assert_eq!(body.char.weight, None, "bold was not shared");
    let first = doc.paragraph(BlockId(1)).expect("first");
    assert_eq!(first.format_of(0).weight, Some(700));
    assert_eq!(first.format_of(0).size_pt, None);
}

#[test]
fn superscript_toggles_on_and_off() {
    let mut ed = three_paragraphs();
    ed.toggle_vertical(VerticalAlign::Superscript).expect("sup");
    assert_eq!(ed.char_summary().vertical, Some(VerticalAlign::Superscript));
    ed.toggle_vertical(VerticalAlign::Subscript).expect("sub");
    assert_eq!(ed.char_summary().vertical, Some(VerticalAlign::Subscript));
    ed.toggle_vertical(VerticalAlign::Subscript).expect("off");
    assert_eq!(ed.char_summary().vertical, Some(VerticalAlign::Baseline));
}

// ---------------------------------------------------------------- lists

use libertum_model::ListStyle;

fn labels(ed: &Editor) -> Vec<String> {
    ed.document()
        .list_labels()
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect()
}

#[test]
fn lists_number_continue_nest_and_end_on_an_empty_item() {
    let mut ed = three_paragraphs();
    ed.select_all();
    ed.set_list(Some(ListStyle::Numbered)).expect("list");
    assert_eq!(labels(&ed), ["1.", "2.", "3."]);
    assert_eq!(ed.para_summary().list, Some(Some(ListStyle::Numbered)));
    // Indent the second item: it becomes "a." under item 1.
    ed.set_caret(
        DocPosition {
            block: BlockId(2),
            offset: 0,
        },
        false,
    );
    assert!(ed.tab_changes_list_level());
    ed.indent(1).expect("level");
    assert_eq!(labels(&ed), ["1.", "a.", "2."]);
    // Return at the end of the last item continues the list; Return again ends it.
    ed.move_doc_end(false);
    ed.insert_text("\n").expect("return");
    assert_eq!(labels(&ed), ["1.", "a.", "2.", "3."]);
    ed.insert_text("\n").expect("return on empty item");
    assert_eq!(labels(&ed), ["1.", "a.", "2.", ""]);
    assert_eq!(ed.document().len(), 4, "no new paragraph");
    // Undo restores the item.
    ed.undo().expect("undo");
    assert_eq!(labels(&ed), ["1.", "a.", "2.", "3."]);
}

#[test]
fn backspace_at_the_start_of_an_item_removes_its_number_and_indent() {
    let mut ed = three_paragraphs();
    ed.select_all();
    ed.set_list(Some(ListStyle::Bullet)).expect("list");
    ed.set_caret(
        DocPosition {
            block: BlockId(2),
            offset: 0,
        },
        false,
    );
    let indent = ed.para_summary().indent_left_pt;
    assert_eq!(indent, Some(libertum_model::LIST_INDENT_PT));
    ed.delete_backward().expect("backspace");
    let p = ed.document().paragraph(BlockId(2)).expect("second");
    assert!(p.props.list.is_none());
    assert_eq!(p.text(), "Second paragraph.", "no merge yet");
    assert_eq!(ed.para_summary().indent_left_pt, Some(0.0));
}

#[test]
fn a_new_list_after_a_list_of_the_same_style_continues_it() {
    let mut ed = three_paragraphs();
    ed.set_caret(
        DocPosition {
            block: BlockId(1),
            offset: 0,
        },
        false,
    );
    ed.set_list(Some(ListStyle::Numbered)).expect("first");
    ed.set_caret(
        DocPosition {
            block: BlockId(2),
            offset: 0,
        },
        false,
    );
    ed.set_list(Some(ListStyle::Numbered)).expect("second");
    assert_eq!(labels(&ed), ["1.", "2.", ""]);
    ed.set_caret(
        DocPosition {
            block: BlockId(3),
            offset: 0,
        },
        false,
    );
    ed.set_list(Some(ListStyle::Lettered)).expect("lettered");
    assert_eq!(labels(&ed), ["1.", "2.", "A."]);
    ed.set_list(None).expect("off");
    assert_eq!(labels(&ed), ["1.", "2.", ""]);
}

// ---------------------------------------------------------------- tables

fn table_editor() -> Editor {
    let mut doc = doc(&["Before", "After"]);
    let table = doc.new_table(2, 2, 400.0);
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    Editor::new(doc)
}

fn table_id(ed: &Editor) -> BlockId {
    ed.document()
        .body()
        .iter()
        .find_map(|block| block.as_table())
        .map(|table| table.id)
        .expect("table")
}

fn cell_paragraph(ed: &Editor, table: BlockId, row: usize, cell: usize) -> BlockId {
    ed.document()
        .table(table)
        .expect("table")
        .cell(row, cell)
        .expect("cell")
        .paragraphs[0]
        .id
}

#[test]
fn tab_moves_between_cells_and_adds_a_row_at_the_end() {
    let mut ed = table_editor();
    let table = table_id(&ed);
    let first = cell_paragraph(&ed, table, 0, 0);
    ed.set_caret(
        DocPosition {
            block: first,
            offset: 0,
        },
        false,
    );
    ed.insert_text("A").unwrap();
    ed.tab_in_table(false).unwrap();
    assert_eq!(ed.caret_cell().expect("cell").cell, 1);
    ed.insert_text("B").unwrap();
    ed.tab_in_table(false).unwrap();
    assert_eq!(ed.caret_cell().expect("cell").row, 1);
    ed.insert_text("C").unwrap();
    ed.tab_in_table(false).unwrap();
    ed.insert_text("D").unwrap();
    let rows = ed.document().table(table).expect("table").rows.len();
    ed.tab_in_table(false).unwrap();
    let after = ed.document().table(table).expect("table");
    assert_eq!(after.rows.len(), rows + 1);
    assert_eq!(after.cell(0, 0).expect("cell").paragraphs[0].text(), "A");
    assert_eq!(after.cell(0, 1).expect("cell").paragraphs[0].text(), "B");
    assert_eq!(after.cell(1, 0).expect("cell").paragraphs[0].text(), "C");
    assert_eq!(after.cell(1, 1).expect("cell").paragraphs[0].text(), "D");
    assert_eq!(after.cell(2, 0).expect("cell").paragraphs[0].text(), "");
    assert_eq!(ed.caret_cell().expect("cell").row, 2);
    ed.undo().unwrap();
    assert_eq!(ed.document().table(table).expect("table").rows.len(), rows);
}

#[test]
fn backspace_at_a_cell_start_moves_to_the_previous_cell() {
    let mut ed = table_editor();
    let table = table_id(&ed);
    let second = cell_paragraph(&ed, table, 0, 1);
    ed.set_caret(
        DocPosition {
            block: second,
            offset: 0,
        },
        false,
    );
    let before = ed.document().clone();
    ed.delete_backward().unwrap();
    let caret = ed.caret();
    assert!(ed.document().content_eq(&before), "no text may be deleted");
    assert_eq!(caret.block, cell_paragraph(&ed, table, 0, 0));
    assert_eq!(ed.caret_cell().expect("cell").cell, 0);
}

#[test]
fn select_all_inside_a_table_selects_the_cell() {
    let mut ed = table_editor();
    let table = table_id(&ed);
    let first = cell_paragraph(&ed, table, 0, 0);
    ed.set_caret(
        DocPosition {
            block: first,
            offset: 0,
        },
        false,
    );
    ed.insert_text("only this cell").unwrap();
    ed.select_all();
    assert_eq!(ed.selected_text().as_deref(), Some("only this cell"));
}

#[test]
fn enter_in_a_cell_splits_inside_the_cell() {
    let mut ed = table_editor();
    let table = table_id(&ed);
    let first = cell_paragraph(&ed, table, 0, 0);
    ed.set_caret(
        DocPosition {
            block: first,
            offset: 0,
        },
        false,
    );
    let blocks = ed.document().len();
    ed.insert_text("one\n two").unwrap();
    let cell = ed
        .document()
        .table(table)
        .expect("table")
        .cell(0, 0)
        .cloned()
        .expect("cell");
    assert_eq!(cell.paragraphs.len(), 2);
    assert_eq!(cell.paragraphs[0].text(), "one");
    assert_eq!(cell.paragraphs[1].text(), " two");
    assert_eq!(ed.document().len(), blocks, "no new top-level block");
}

#[test]
fn row_and_column_commands_undo_exactly() {
    let mut ed = table_editor();
    let table = table_id(&ed);
    let first = cell_paragraph(&ed, table, 0, 0);
    ed.set_caret(
        DocPosition {
            block: first,
            offset: 0,
        },
        false,
    );
    let original = ed.document().clone();
    // Undo restores the table but leaves the caret at the document edge, so each command
    // starts by placing the caret back in the first cell.
    let first_cell = |ed: &Editor| DocPosition {
        block: cell_paragraph(ed, table, 0, 0),
        offset: 0,
    };

    ed.insert_row_below().unwrap();
    assert_eq!(ed.document().table(table).expect("table").rows.len(), 3);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&original));

    ed.set_caret(first_cell(&ed), false);
    ed.insert_column_right().unwrap();
    assert_eq!(ed.document().table(table).expect("table").columns(), 3);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&original));

    ed.set_caret(first_cell(&ed), false);
    ed.delete_row().unwrap();
    assert_eq!(ed.document().table(table).expect("table").rows.len(), 1);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&original));

    ed.set_caret(first_cell(&ed), false);
    ed.delete_column().unwrap();
    assert_eq!(ed.document().table(table).expect("table").columns(), 1);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&original));

    ed.set_caret(first_cell(&ed), false);
    ed.delete_table().unwrap();
    assert!(ed.document().table(table).is_none());
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&original));
}

#[test]
fn inserting_a_table_puts_the_caret_in_the_first_cell() {
    let mut ed = editor(&["Body"]);
    ed.set_caret(pos(&ed, 0, 4), false);
    let original = ed.document().clone();
    ed.insert_table(2, 3, 400.0).unwrap();
    let doc = ed.document();
    assert_eq!(doc.body().len(), 3, "body, table, trailing paragraph");
    let table = doc.body().at(1).and_then(|b| b.as_table()).expect("table");
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.columns(), 3);
    let table_id = table.id;
    let first_cell = table.rows[0].cells[0].paragraphs[0].id;
    assert_eq!(ed.caret().block, first_cell);
    assert!(ed.in_table());
    ed.insert_text("X").unwrap();
    assert_eq!(
        ed.document()
            .table(table_id)
            .expect("table")
            .cell(0, 0)
            .expect("cell")
            .paragraphs[0]
            .text(),
        "X"
    );
    ed.undo().unwrap();
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&original));
}

#[test]
fn footnotes_insert_edit_and_renumber_in_document_order() {
    let mut ed = editor(&["First", "Second"]);
    let first = ed.document().paragraphs().next().expect("first").id;
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.insert_footnote().expect("insert first");
    ed.set_footnote_text("First note".into())
        .expect("edit first");
    assert_eq!(ed.footnote(), Some("First note"));
    assert_eq!(ed.footnote_number(first), Some(1));

    let second = ed.document().paragraphs().nth(1).expect("second").id;
    ed.set_caret(pos(&ed, 1, 0), false);
    ed.insert_footnote().expect("insert second");
    ed.set_footnote_text("Second note".into())
        .expect("edit second");
    assert_eq!(ed.footnote_number(first), Some(1));
    assert_eq!(ed.footnote_number(second), Some(2));

    ed.undo().expect("undo the text");
    assert_eq!(
        ed.document().paragraph(second).and_then(|p| p.note()),
        Some("")
    );
    ed.undo().expect("undo the insertion");
    assert_eq!(ed.document().paragraph(second).and_then(|p| p.note()), None);
}

#[test]
fn multiple_notes_and_endnotes_are_kept_on_one_paragraph() {
    let mut ed = editor(&["Notes"]);
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.insert_footnote().expect("first footnote");
    ed.insert_footnote().expect("second footnote");
    ed.insert_endnote().expect("endnote");
    let notes = ed.notes();
    assert_eq!(notes.len(), 3);
    assert_eq!(
        notes
            .iter()
            .filter(|n| n.kind == NoteKind::Footnote)
            .count(),
        2
    );
    assert_eq!(
        notes.iter().filter(|n| n.kind == NoteKind::Endnote).count(),
        1
    );
    let block = ed.document().paragraphs().next().expect("paragraph").id;
    assert_eq!(ed.footnote_number(block), Some(1));
}

#[test]
fn bookmarks_insert_rename_remove_and_undo() {
    let mut ed = editor(&["alpha beta"]);
    ed.set_caret(pos(&ed, 0, 6), false);
    let (_, name) = ed.insert_bookmark("mark").expect("insert");
    assert_eq!(name, "mark");
    assert_eq!(
        ed.bookmark_position("mark"),
        Some(pos(&ed, 0, 6)),
        "the bookmark sits at the caret"
    );

    let block = ed.document().paragraph_at(0).expect("paragraph").id;
    ed.rename_bookmark(block, 0, "renamed mark")
        .expect("rename");
    assert_eq!(ed.bookmarks()[0].2.name, "renamed_mark");
    assert_eq!(ed.bookmark_position("renamed_mark"), Some(pos(&ed, 0, 6)));

    ed.remove_bookmark(block, 0).expect("remove");
    assert!(ed.bookmarks().is_empty());
    ed.undo().expect("undo");
    assert_eq!(ed.bookmarks()[0].2.name, "renamed_mark");
    ed.undo().expect("undo");
    assert_eq!(ed.bookmarks()[0].2.name, "mark");
    ed.undo().expect("undo");
    assert!(ed.bookmarks().is_empty(), "the first insert is undone");
}

#[test]
fn bookmark_names_are_repaired_and_unique() {
    let mut ed = editor(&["one", "two"]);
    ed.set_caret(pos(&ed, 0, 0), false);
    let (_, first) = ed.insert_bookmark("").expect("insert");
    assert_eq!(first, "Bookmark");
    ed.set_caret(pos(&ed, 1, 0), false);
    let (_, second) = ed.insert_bookmark("Bookmark").expect("insert");
    assert_eq!(second, "Bookmark_2", "duplicates get a suffix");
    let (_, third) = ed.insert_bookmark("9 lives!").expect("insert");
    assert_eq!(third, "B9_lives", "a leading digit gains a letter");
    assert_eq!(ed.next_bookmark_name(), "Bookmark_1");
}

#[test]
fn bookmarks_follow_typing_and_splitting() {
    let mut ed = editor(&["alpha beta"]);
    ed.set_caret(pos(&ed, 0, 5), false);
    ed.insert_bookmark("middle").expect("insert");
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.insert_text("XX").expect("type");
    assert_eq!(ed.bookmark_position("middle"), Some(pos(&ed, 0, 7)));

    ed.set_caret(pos(&ed, 0, 7), false);
    ed.insert_text("\n").expect("split");
    assert_eq!(
        ed.bookmark_position("middle"),
        Some(pos(&ed, 1, 0)),
        "a bookmark at the split point moves to the new paragraph"
    );
}

#[test]
fn bookmarks_cannot_be_placed_in_generated_toc_entries() {
    let mut ed = editor(&["Chapter", "Body"]);
    ed.set_caret(pos(&ed, 1, 0), false);
    ed.insert_table_of_contents().expect("toc");
    let toc_block = ed
        .document()
        .paragraphs()
        .find(|paragraph| paragraph.props.toc_target.is_some())
        .expect("toc entry")
        .id;
    ed.set_caret(
        DocPosition {
            block: toc_block,
            offset: 0,
        },
        false,
    );
    assert!(matches!(
        ed.insert_bookmark("nope"),
        Err(EditorError::ReadOnly)
    ));
}

// ---------------------------------------------------------------- images, references, merging

fn png(width: u32, height: u32) -> std::sync::Arc<libertum_model::Media> {
    std::sync::Arc::new(
        libertum_model::Media::new(libertum_model::test_png_header(width, height)).expect("png"),
    )
}

fn caret_in_cell(ed: &mut Editor, row: usize, cell: usize) -> BlockId {
    let table = table_id(ed);
    let block = cell_paragraph(ed, table, row, cell);
    ed.set_caret(DocPosition { block, offset: 0 }, false);
    table
}

#[test]
fn an_image_is_inserted_resized_described_and_removed_with_undo() {
    let mut ed = editor(&["A picture goes here"]);
    ed.set_caret(pos(&ed, 0, 2), false);
    ed.insert_image(png(800, 400), 300.0, "cat.png")
        .expect("insert");
    let (block, images) = ed.caret_images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].offset, 2);
    assert_eq!((images[0].width_pt, images[0].height_pt), (300.0, 150.0));
    assert_eq!(images[0].name, "cat.png");
    assert_eq!(ed.image_count(), 1);

    ed.resize_image(block, 0, 100.0).expect("resize");
    let (_, images) = ed.caret_images();
    assert_eq!((images[0].width_pt, images[0].height_pt), (100.0, 50.0));
    ed.set_image_alt(block, 0, "A cat").expect("alt");
    assert_eq!(ed.caret_images().1[0].alt, "A cat");
    // An unchanged value is not an undo step.
    let before = ed.history().undo_len();
    ed.set_image_alt(block, 0, "A cat").expect("alt again");
    assert_eq!(ed.history().undo_len(), before);

    // Typing before the image moves it along; typing at it goes in front of it.
    ed.set_caret(pos(&ed, 0, 0), false);
    ed.insert_text("X").expect("type");
    assert_eq!(ed.caret_images().1[0].offset, 3);

    ed.remove_image(block, 0).expect("remove");
    assert_eq!(ed.image_count(), 0);
    ed.undo().expect("undo");
    assert_eq!(ed.image_count(), 1);
    assert_eq!(ed.caret_images().1[0].alt, "A cat");
    // Out-of-range indices do nothing.
    assert!(ed.remove_image(block, 7).expect("no-op").is_empty());
}

#[test]
fn a_second_image_at_the_same_place_follows_the_first() {
    let mut ed = editor(&["ab"]);
    ed.set_caret(pos(&ed, 0, 1), false);
    ed.insert_image(png(10, 10), 300.0, "first").expect("first");
    ed.insert_image(png(20, 10), 300.0, "second")
        .expect("second");
    let names: Vec<&str> = ed
        .caret_images()
        .1
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(names, vec!["first", "second"]);
}

#[test]
fn a_cross_reference_shows_the_bookmark_text_or_page_and_refreshes() {
    let mut ed = editor(&["Chapter One", "See  for details, page ."]);
    ed.set_caret(pos(&ed, 0, 0), false);
    let (_, name) = ed.insert_bookmark("intro").expect("bookmark");
    let pages = std::cell::Cell::new(Some(4u32));
    let page_of = |_: BlockId| pages.get();

    ed.set_caret(pos(&ed, 1, 4), false);
    ed.insert_cross_ref(&name, CrossRefKind::Text, &page_of)
        .expect("text reference");
    assert_eq!(text_at(&ed, 1), "See Chapter One for details, page .");
    assert_eq!(ed.caret().offset, 15, "the caret lands after the reference");
    let end = text_at(&ed, 1).len() - 1;
    ed.set_caret(pos(&ed, 1, end), false);
    ed.insert_cross_ref(&name, CrossRefKind::Page, &page_of)
        .expect("page reference");
    assert_eq!(text_at(&ed, 1), "See Chapter One for details, page 4.");
    let refs = ed.cross_refs();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].2.range, 4..15);
    assert_eq!(refs[1].2.kind, CrossRefKind::Page);

    // Nothing stale: no change and no undo step.
    let before = ed.history().undo_len();
    assert!(ed.refresh_cross_refs(&page_of).expect("refresh").is_empty());
    assert_eq!(ed.history().undo_len(), before);

    // The heading is retitled and moves to page 12: both references follow in one step.
    ed.set_caret(pos(&ed, 0, 11), false);
    ed.insert_text(" Revised").expect("retitle");
    pages.set(Some(12));
    assert!(!ed.refresh_cross_refs(&page_of).expect("refresh").is_empty());
    assert_eq!(
        text_at(&ed, 1),
        "See Chapter One Revised for details, page 12."
    );
    let refs = ed.cross_refs();
    assert_eq!(refs[0].2.range, 4..23);
    assert_eq!(refs[1].2.range, 42..44);
    ed.undo().expect("undo the refresh");
    assert_eq!(text_at(&ed, 1), "See Chapter One for details, page 4.");
    assert_eq!(ed.cross_refs()[1].2.range, 34..35);

    // A page not laid out yet shows a question mark; an unknown bookmark inserts nothing.
    pages.set(None);
    ed.refresh_cross_refs(&page_of).expect("refresh");
    assert!(text_at(&ed, 1).ends_with("page ?."));
    assert!(
        ed.insert_cross_ref("nowhere", CrossRefKind::Text, &page_of)
            .expect("no-op")
            .is_empty()
    );
    // Removing a reference keeps its text.
    let (block, index, _) = ed.cross_refs()[0].clone();
    ed.remove_cross_ref(block, index).expect("remove");
    assert_eq!(ed.cross_refs().len(), 1);
    assert!(text_at(&ed, 1).starts_with("See Chapter One"));
}

#[test]
fn a_reference_to_its_own_paragraph_does_not_grow_when_refreshed() {
    let mut ed = editor(&["Heading "]);
    ed.set_caret(pos(&ed, 0, 0), false);
    let (_, name) = ed.insert_bookmark("self").expect("bookmark");
    let page_of = |_: BlockId| Some(1u32);
    ed.set_caret(pos(&ed, 0, 8), false);
    ed.insert_cross_ref(&name, CrossRefKind::Text, &page_of)
        .expect("reference");
    assert_eq!(text_at(&ed, 0), "Heading Heading");
    ed.refresh_cross_refs(&page_of).expect("refresh");
    ed.refresh_cross_refs(&page_of).expect("refresh again");
    assert_eq!(text_at(&ed, 0), "Heading Heading");
}

#[test]
fn a_refreshed_reference_keeps_its_formatting() {
    let mut ed = editor(&["Title", "See X."]);
    ed.set_caret(pos(&ed, 0, 0), false);
    let (_, name) = ed.insert_bookmark("t").expect("bookmark");
    let page_of = |_: BlockId| Some(1u32);
    ed.set_caret(pos(&ed, 1, 4), false);
    ed.insert_cross_ref(&name, CrossRefKind::Text, &page_of)
        .expect("reference");
    assert_eq!(text_at(&ed, 1), "See TitleX.");
    ed.set_selection(pos(&ed, 1, 4), pos(&ed, 1, 9));
    ed.toggle_italic().expect("italic");
    ed.set_caret(pos(&ed, 0, 5), false);
    ed.insert_text(" Two").expect("retitle");
    ed.refresh_cross_refs(&page_of).expect("refresh");
    assert_eq!(text_at(&ed, 1), "See Title TwoX.");
    let p = ed.document().paragraph_at(1).expect("paragraph");
    assert_eq!(p.format_of(12).italic, Some(true), "the new text is italic");
    assert_eq!(p.format_of(13).italic, None, "the text after it is not");
}

#[test]
fn a_table_style_is_applied_and_removed_in_one_step_each() {
    let mut ed = table_editor();
    let table = caret_in_cell(&mut ed, 0, 0);
    assert_eq!(ed.table_style_name(), None);
    ed.set_table_style(Some("Banded Rows")).expect("style");
    assert_eq!(ed.table_style_name().as_deref(), Some("Banded Rows"));
    let t = ed.document().table(table).expect("table");
    assert_eq!(t.header_rows, 1);
    assert!(t.style_fill(0).is_some());
    // The same style again, or an unknown one, is not a step.
    assert!(
        ed.set_table_style(Some("Banded Rows"))
            .expect("same")
            .is_empty()
    );
    assert!(
        ed.set_table_style(Some("No Such Style"))
            .expect("unknown")
            .is_empty()
    );
    ed.set_table_style(None).expect("remove");
    assert_eq!(ed.table_style_name(), None);
    ed.undo().expect("undo");
    assert_eq!(ed.table_style_name().as_deref(), Some("Banded Rows"));
    // Outside a table nothing happens.
    ed.set_caret(pos(&ed, 0, 0), false);
    assert!(
        ed.set_table_style(Some("Grid"))
            .expect("outside")
            .is_empty()
    );
}

#[test]
fn cells_merge_to_the_right_and_split_again() {
    let mut ed = table_editor();
    let table = caret_in_cell(&mut ed, 0, 0);
    ed.insert_text("left").expect("type");
    caret_in_cell(&mut ed, 0, 1);
    ed.insert_text("right").expect("type");
    let right = cell_paragraph(&ed, table, 0, 1);
    caret_in_cell(&mut ed, 0, 0);
    assert!(ed.can_merge_cell_right());
    assert!(!ed.can_split_cell());
    ed.merge_cell_right().expect("merge");
    let t = ed.document().table(table).expect("table");
    assert_eq!(t.rows[0].cells.len(), 1);
    assert_eq!(t.rows[0].cells[0].col_span, 2);
    assert_eq!(t.rows[0].cells[0].text(), "left\nright");
    assert_eq!(
        t.rows[0].cells[0].paragraphs[1].id, right,
        "the text keeps its id"
    );
    assert_eq!(t.rows[1].cells.len(), 2, "other rows are untouched");
    assert!(!ed.can_merge_cell_right(), "nothing to its right now");

    assert!(ed.can_split_cell());
    ed.split_cell().expect("split");
    let t = ed.document().table(table).expect("table");
    assert_eq!(t.rows[0].cells.len(), 2);
    assert_eq!(t.rows[0].cells[0].col_span, 1);
    assert_eq!(
        t.rows[0].cells[0].text(),
        "left\nright",
        "the text stays in the first cell"
    );
    assert_eq!(t.rows[0].cells[1].text(), "");
    let fresh = t.rows[0].cells[1].paragraphs[0].id;
    assert!(
        ed.document().paragraph(fresh).is_some(),
        "the new cell is a real paragraph"
    );

    ed.undo().expect("undo split");
    ed.undo().expect("undo merge");
    let t = ed.document().table(table).expect("table");
    assert_eq!(t.rows[0].cells[1].text(), "right");
}

#[test]
fn cells_merge_downwards_and_the_caret_skips_the_covered_cell() {
    let mut ed = table_editor();
    let table = caret_in_cell(&mut ed, 1, 0);
    ed.insert_text("below").expect("type");
    let below = cell_paragraph(&ed, table, 1, 0);
    caret_in_cell(&mut ed, 0, 0);
    ed.insert_text("above").expect("type");
    assert!(ed.can_merge_cell_below());
    ed.merge_cell_below().expect("merge");
    let t = ed.document().table(table).expect("table");
    assert!(t.rows[1].cells[0].merged_into_above);
    assert_eq!(t.rows[0].cells[0].text(), "above\nbelow");
    assert_eq!(t.rows[0].cells[0].paragraphs[1].id, below);
    assert_eq!(t.rows[1].cells[0].text(), "", "the covered cell is empty");
    let covered = t.rows[1].cells[0].paragraphs[0].id;
    assert!(!ed.can_merge_cell_below(), "no row left below");
    // A cell beside a vertical merge cannot merge sideways into it.
    assert!(!ed.can_merge_cell_right());

    // Tab goes from the merged cell to (0, 1), then straight to (1, 1).
    caret_in_cell(&mut ed, 0, 0);
    ed.tab_in_table(false).expect("tab");
    assert_eq!(ed.caret().block, cell_paragraph(&ed, table, 0, 1));
    ed.tab_in_table(false).expect("tab");
    assert_eq!(ed.caret().block, cell_paragraph(&ed, table, 1, 1));
    ed.tab_in_table(true).expect("shift-tab");
    assert_eq!(ed.caret().block, cell_paragraph(&ed, table, 0, 1));
    // Arrow keys never rest in the covered cell either.
    let end = ed.document().paragraph(ed.caret().block).expect("p").len();
    ed.set_caret(
        DocPosition {
            block: ed.caret().block,
            offset: end,
        },
        false,
    );
    ed.move_right(false);
    assert_ne!(ed.caret().block, covered);
    assert_eq!(ed.caret().block, cell_paragraph(&ed, table, 1, 1));
    ed.move_left(false);
    assert_ne!(ed.caret().block, covered);

    // Splitting separates the two cells again; the text stays above.
    caret_in_cell(&mut ed, 0, 0);
    assert!(ed.can_split_cell());
    ed.split_cell().expect("split");
    let t = ed.document().table(table).expect("table");
    assert!(!t.rows[1].cells[0].merged_into_above);
    assert_eq!(t.rows[0].cells[0].text(), "above\nbelow");
    ed.undo().expect("undo split");
    ed.undo().expect("undo merge");
    let t = ed.document().table(table).expect("table");
    assert_eq!(t.rows[1].cells[0].text(), "below");
    assert!(!t.rows[1].cells[0].merged_into_above);
}

#[test]
fn merging_down_needs_the_same_columns() {
    let mut ed = table_editor();
    let table = caret_in_cell(&mut ed, 0, 0);
    ed.merge_cell_right().expect("merge the first row");
    // The wide cell covers two columns; the cell below covers one.
    assert!(!ed.can_merge_cell_below());
    assert!(ed.merge_cell_below().expect("no-op").is_empty());
    assert_eq!(
        ed.document().table(table).expect("table").rows[1]
            .cells
            .len(),
        2
    );
}

#[test]
fn a_column_width_changes_without_changing_the_table_width() {
    let mut ed = table_editor();
    let table = caret_in_cell(&mut ed, 0, 0);
    assert_eq!(ed.caret_column_width(), Some(200.0));

    ed.set_caret_column_width(300.0).expect("wider");
    let widths = ed.document().table(table).expect("table").widths_pt.clone();
    assert!((widths[0] - 300.0).abs() < 0.01, "{widths:?}");
    assert!((widths[1] - 100.0).abs() < 0.01, "the neighbour gives way");
    assert!((widths.iter().sum::<f32>() - 400.0).abs() < 0.01);

    ed.undo().expect("undo");
    assert_eq!(ed.caret_column_width(), Some(200.0));

    // The neighbour never goes below the minimum: 400 - 18 = 382 at most.
    ed.set_caret_column_width(5.0).expect("narrower");
    let widths = ed.document().table(table).expect("table").widths_pt.clone();
    assert!((widths[0] - 18.0).abs() < 0.01, "{widths:?}");
    assert!((widths[1] - 382.0).abs() < 0.01);
    assert!((ed.caret_column_width().expect("width") - 18.0).abs() < 0.01);
}

#[test]
fn columns_equalise_and_a_merged_cell_reads_its_first_column() {
    let mut ed = table_editor();
    let table = caret_in_cell(&mut ed, 0, 0);
    ed.set_caret_column_width(300.0).expect("wider");
    ed.merge_cell_right().expect("merge the first row");
    // The wide cell starts at the first column, so that is the width shown.
    assert_eq!(ed.caret_column_width(), Some(300.0));

    ed.equalise_table_columns().expect("equalise");
    let widths = ed.document().table(table).expect("table").widths_pt.clone();
    assert!((widths[0] - 200.0).abs() < 0.01, "{widths:?}");
    assert!((widths[1] - 200.0).abs() < 0.01);
    assert!((widths.iter().sum::<f32>() - 400.0).abs() < 0.01);

    // Equal columns are already equal: no change and no undo step.
    let before = ed.history().undo_len();
    assert!(ed.equalise_table_columns().expect("no-op").is_empty());
    assert_eq!(ed.history().undo_len(), before);
}

// ------------------------------------------------------------------ find and replace

use crate::SearchOptions;

#[test]
fn find_moves_forward_and_backwards_and_wraps() {
    let ed = editor(&["a cat sat", "the cat ran"]);
    let options = SearchOptions::default();
    let first = ed
        .find("cat", options, pos(&ed, 0, 0), true)
        .expect("a match");
    assert_eq!(first.block, ed.document().paragraph_at(0).expect("p").id);
    assert_eq!(first.range, 2..5);
    // From the end of the first match, the next one is in the second paragraph.
    let second = ed
        .find("cat", options, pos(&ed, 0, 5), true)
        .expect("a match");
    assert_eq!(second.block, ed.document().paragraph_at(1).expect("p").id);
    // From the start again, forward wraps back to the first paragraph.
    let wrapped = ed
        .find("cat", options, pos(&ed, 1, 12), true)
        .expect("a match");
    assert_eq!(wrapped.block, first.block);
    // Backwards from the start wraps to the last match.
    let back = ed
        .find("cat", options, pos(&ed, 0, 0), false)
        .expect("a match");
    assert_eq!(back.block, second.block);
    assert!(ed.find("dog", options, pos(&ed, 0, 0), true).is_none());
    assert!(ed.find("", options, pos(&ed, 0, 0), true).is_none());
}

#[test]
fn search_can_match_case_and_whole_words() {
    let ed = editor(&["Cat cat scatter CAT"]);
    let insensitive = SearchOptions::default();
    let hits: Vec<_> = crate::matches_in(
        ed.document().paragraph_at(0).expect("p").text(),
        "cat",
        insensitive,
    )
    .into_iter()
    .map(|r| r.start)
    .collect();
    assert_eq!(hits, vec![0, 4, 9, 16], "every occurrence, either case");

    let words = SearchOptions {
        match_case: false,
        whole_word: true,
    };
    let hits: Vec<_> = crate::matches_in(
        ed.document().paragraph_at(0).expect("p").text(),
        "cat",
        words,
    )
    .into_iter()
    .map(|r| r.start)
    .collect();
    assert_eq!(hits, vec![0, 4, 16], "not inside scatter");

    let cased = SearchOptions {
        match_case: true,
        whole_word: false,
    };
    let hits: Vec<_> = crate::matches_in(
        ed.document().paragraph_at(0).expect("p").text(),
        "cat",
        cased,
    )
    .into_iter()
    .map(|r| r.start)
    .collect();
    assert_eq!(hits, vec![4, 9], "lowercase only");
}

#[test]
fn replace_one_takes_the_old_formatting_and_undoes() {
    let mut ed = editor(&["see the cat here"]);
    ed.set_selection(pos(&ed, 0, 8), pos(&ed, 0, 11));
    ed.toggle_bold().expect("bold the cat");
    let found = ed
        .find("cat", SearchOptions::default(), pos(&ed, 0, 0), true)
        .expect("a match");
    ed.replace_match(&found, "dog").expect("replace");
    assert_eq!(text_at(&ed, 0), "see the dog here");
    let paragraph = ed.document().paragraph_at(0).expect("p");
    assert_eq!(
        paragraph.format_of(8).weight,
        Some(700),
        "the replacement keeps the formatting of the text it replaces"
    );
    ed.undo().expect("undo");
    assert_eq!(text_at(&ed, 0), "see the cat here");
}

#[test]
fn replace_all_is_one_undo_step_and_skips_generated_entries() {
    let mut ed = editor(&["cat cat", "a cat", "no dog"]);
    let (change, count) = ed
        .replace_all("cat", "dog", SearchOptions::default())
        .expect("replace all");
    assert_eq!(count, 3);
    assert!(!change.is_empty());
    assert_eq!(text_at(&ed, 0), "dog dog");
    assert_eq!(text_at(&ed, 1), "a dog");
    assert_eq!(text_at(&ed, 2), "no dog");
    ed.undo().expect("undo");
    assert_eq!(text_at(&ed, 0), "cat cat");
    assert_eq!(text_at(&ed, 1), "a cat");

    // A table of contents entry is generated, so it is never searched or rewritten.
    let mut book = editor(&["Contents", "Chapter One", "Body text"]);
    book.insert_table_of_contents().expect("toc");
    let entries = toc(&book);
    assert!(!entries.is_empty(), "the book has a table of contents");
    let entry_text = book
        .document()
        .paragraph_at(0)
        .expect("entry")
        .text()
        .to_owned();
    let (_, count) = book
        .replace_all("Chapter", "Section", SearchOptions::default())
        .expect("replace all");
    assert!(count >= 1, "the heading was replaced");
    assert_eq!(
        book.document().paragraph_at(0).expect("entry").text(),
        entry_text,
        "the generated entry is untouched"
    );
}

#[test]
fn search_handles_accents_overlaps_and_text_that_changes_length_when_lowercased() {
    let insensitive = SearchOptions::default();
    let starts = |text: &str, needle: &str| -> Vec<std::ops::Range<usize>> {
        crate::matches_in(text, needle, insensitive)
    };
    // Accented text: offsets are the original bytes.
    let text = "Un RÉSUMÉ, un résumé";
    let hits = starts(text, "résumé");
    assert_eq!(hits.len(), 2);
    for range in &hits {
        assert_eq!(text[range.clone()].to_lowercase(), "résumé");
    }
    // Overlapping occurrences are all found.
    assert_eq!(starts("aaaa", "aa"), vec![0..2, 1..3, 2..4]);
    // U+0130 lowercases to two characters: the text after it still maps back exactly.
    let text = "İstanbul and Cat and İ cat";
    let hits = starts(text, "cat");
    assert_eq!(hits.len(), 2);
    for range in &hits {
        assert!(
            text[range.clone()].eq_ignore_ascii_case("cat"),
            "{:?}",
            &text[range.clone()]
        );
    }
    // A needle longer than the text, and an empty one, find nothing.
    assert!(starts("ab", "abc").is_empty());
    assert!(starts("ab", "").is_empty());
    assert!(starts("", "a").is_empty());
}

#[test]
fn the_match_list_follows_edits_undo_and_a_changed_query() {
    let mut ed = editor(&["cat one", "two cat", "three"]);
    let options = SearchOptions::default();
    let start = pos(&ed, 0, 0);
    assert_eq!(ed.match_position("cat", options, None).0, 2);
    let second = ed
        .find("cat", options, pos(&ed, 0, 3), true)
        .expect("second");
    assert_eq!(
        ed.match_position("cat", options, Some(&second)),
        (2, Some(2))
    );
    // Typing a third occurrence: the list is rebuilt, not reused.
    ed.set_caret(pos(&ed, 2, 5), false);
    ed.insert_text(" cat").expect("type");
    assert_eq!(ed.match_position("cat", options, None).0, 3);
    let third = ed
        .find("cat", options, pos(&ed, 2, 0), true)
        .expect("third");
    assert_eq!(third.range, 6..9);
    assert_eq!(
        ed.match_position("cat", options, Some(&third)),
        (3, Some(3))
    );
    // Undo takes it away again.
    ed.undo().expect("undo");
    assert_eq!(ed.match_position("cat", options, None).0, 2);
    // Another query, and other options, never see the old list.
    assert_eq!(ed.match_position("two", options, None).0, 1);
    let cased = SearchOptions {
        match_case: true,
        whole_word: false,
    };
    assert_eq!(ed.match_position("Cat", cased, None).0, 0);
    // Backwards from the very start wraps to the last match; forwards from the end wraps
    // to the first.
    let last = ed.find("cat", options, start, false).expect("wrap back");
    assert_eq!(last.block, ed.document().paragraph_at(1).expect("p").id);
    let first = ed
        .find("cat", options, pos(&ed, 2, 5), true)
        .expect("wrap on");
    assert_eq!(first.block, ed.document().paragraph_at(0).expect("p").id);
    // A match that is not in the list has no position.
    let stray = SearchMatch {
        block: first.block,
        range: 1..4,
    };
    assert_eq!(ed.match_position("cat", options, Some(&stray)), (2, None));
    // A new document is searched afresh even though its revision starts over.
    ed.replace_document(doc(&["no felines here"]));
    assert_eq!(ed.match_position("cat", options, None).0, 0);
}

#[test]
fn a_query_typed_letter_by_letter_finds_what_a_fresh_search_finds() {
    let texts = [
        "Catalogue of cats",
        "a dog",
        "Concatenate the CATALOGUE",
        "cat",
        "",
    ];
    for options in [
        SearchOptions::default(),
        SearchOptions {
            match_case: true,
            whole_word: false,
        },
        SearchOptions {
            match_case: false,
            whole_word: true,
        },
    ] {
        let typed = editor(&texts);
        let query = "catalogue";
        for end in 1..=query.len() {
            let needle = &query[..end];
            // The typed editor narrows from the shorter query; a fresh one reads everything.
            let fresh = editor(&texts);
            let mut from = pos(&typed, 0, 0);
            let mut narrowed = Vec::new();
            let mut whole = Vec::new();
            let total = typed.match_position(needle, options, None).0;
            for _ in 0..total {
                let Some(found) = typed.find(needle, options, from, true) else {
                    break;
                };
                from = DocPosition {
                    block: found.block,
                    offset: found.range.start + 1,
                };
                narrowed.push(found.range);
            }
            let mut from = pos(&fresh, 0, 0);
            for _ in 0..fresh.match_position(needle, options, None).0 {
                let Some(found) = fresh.find(needle, options, from, true) else {
                    break;
                };
                from = DocPosition {
                    block: found.block,
                    offset: found.range.start + 1,
                };
                whole.push(found.range);
            }
            assert_eq!(narrowed, whole, "{needle:?} with {options:?}");
            assert_eq!(total, fresh.match_position(needle, options, None).0);
        }
        // Deleting a letter widens the search again.
        assert_eq!(
            typed.match_position("cat", options, None).0,
            editor(&texts).match_position("cat", options, None).0
        );
    }
}

#[test]
fn a_watermark_goes_on_every_section_and_undoes_in_one_step() {
    let mut ed = editor(&["One", "Two"]);
    let mut d = ed.document().clone();
    d.sections.push(libertum_model::Section::default());
    ed.replace_document(d);
    assert!(ed.watermark().is_none());
    let mark = libertum_model::Watermark::new("DRAFT", true);
    ed.set_watermark(mark.clone()).expect("set");
    assert!(ed.document().sections.iter().all(|s| s.watermark == mark));
    assert_eq!(ed.watermark().map(|w| w.text.as_str()), Some("DRAFT"));
    // The same again is not an undo step.
    let steps = ed.history().undo_len();
    assert!(ed.set_watermark(mark).expect("same").is_empty());
    assert_eq!(ed.history().undo_len(), steps);
    ed.undo().expect("undo");
    assert!(ed.document().sections.iter().all(|s| s.watermark.is_none()));
}

#[test]
fn a_language_goes_on_every_section_and_undoes_in_one_step() {
    let mut ed = editor(&["One", "Two"]);
    let mut d = ed.document().clone();
    d.sections.push(libertum_model::Section::default());
    ed.replace_document(d);
    assert_eq!(ed.language(), None);
    ed.set_language(Some("en_gb")).expect("set");
    assert_eq!(ed.language(), Some("en-GB"), "the tag is normalised");
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| s.language.as_deref() == Some("en-GB"))
    );
    // The same again is not an undo step.
    let steps = ed.history().undo_len();
    assert!(ed.set_language(Some("en-GB")).expect("same").is_empty());
    assert_eq!(ed.history().undo_len(), steps);
    // A tag that is not one clears the language.
    ed.set_language(Some("not a tag!")).expect("clear");
    assert_eq!(ed.language(), None);
    ed.undo().expect("undo");
    assert_eq!(ed.language(), Some("en-GB"));
}

#[test]
fn a_page_colour_goes_on_every_section_opaque_and_undoes_in_one_step() {
    let mut ed = editor(&["One", "Two"]);
    let mut d = ed.document().clone();
    d.sections.push(libertum_model::Section::default());
    ed.replace_document(d);
    assert!(ed.page_colour().is_none());
    ed.set_page_colour(Some([0xF4, 0xB6, 0xB6, 0x10]))
        .expect("set");
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| s.page_colour == Some([0xF4, 0xB6, 0xB6, 0xFF])),
        "every section, stored opaque"
    );
    let steps = ed.history().undo_len();
    assert!(
        ed.set_page_colour(Some([0xF4, 0xB6, 0xB6, 0xFF]))
            .expect("same")
            .is_empty()
    );
    assert_eq!(ed.history().undo_len(), steps);
    ed.undo().expect("undo");
    assert!(
        ed.document()
            .sections
            .iter()
            .all(|s| s.page_colour.is_none())
    );
}

#[test]
fn replacing_ranges_in_a_paragraph_is_one_undo_step() {
    let mut ed = editor(&["The quick brown fox"]);
    let block = ed.document().paragraph_at(0).expect("p").id;
    ed.replace_in_paragraph(block, &[(4..9, "slow".into()), (16..19, "dog".into())])
        .expect("replace");
    assert_eq!(
        ed.document().paragraph_at(0).expect("p").text(),
        "The slow brown dog"
    );
    ed.undo().expect("undo");
    assert_eq!(
        ed.document().paragraph_at(0).expect("p").text(),
        "The quick brown fox"
    );
}

#[test]
fn replacing_paragraphs_puts_them_back_in_place_and_undoes_in_one_step() {
    let mut ed = editor(&["One", "Two", "Three"]);
    let ids: Vec<_> = ed.document().paragraphs().map(|p| p.id).collect();
    ed.replace_paragraphs(&[ids[1]], None, None, &["Deux".into(), "Zwei".into()])
        .expect("replace");
    let texts = |ed: &Editor| -> Vec<String> {
        ed.document()
            .paragraphs()
            .map(|p| p.text().to_owned())
            .collect()
    };
    assert_eq!(texts(&ed), ["One", "Deux", "Zwei", "Three"]);
    ed.undo().expect("undo");
    assert_eq!(texts(&ed), ["One", "Two", "Three"]);
    // Nothing removed: the posted paragraph goes after `after`.
    ed.replace_paragraphs(&[], Some(ids[2]), None, &["Four".into()])
        .expect("insert");
    assert_eq!(texts(&ed), ["One", "Two", "Three", "Four"]);
    // Removing everything leaves one empty paragraph.
    let all: Vec<_> = ed.document().paragraphs().map(|p| p.id).collect();
    ed.replace_paragraphs(&all, None, None, &[]).expect("clear");
    assert_eq!(texts(&ed), [""]);
}

#[test]
fn an_imported_document_goes_after_the_caret_with_fresh_ids_and_undoes_in_one_step() {
    let mut ed = editor(&["One", "Two"]);
    let first = ed.document().paragraph_at(0).expect("p").id;
    ed.set_caret(
        DocPosition {
            block: first,
            offset: 0,
        },
        false,
    );
    let mut source = Document::from_paragraphs(vec![
        Paragraph::new(BlockId(1), "Chapter").with_props(ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        }),
        Paragraph::new(BlockId(2), "Imported"),
    ]);
    source.styles = StyleSheet::default();
    let before: Vec<BlockId> = ed.document().paragraphs().map(|p| p.id).collect();
    ed.insert_document(&source).expect("import");
    let texts: Vec<String> = ed
        .document()
        .paragraphs()
        .map(|p| p.text().to_owned())
        .collect();
    assert_eq!(texts, ["One", "Chapter", "Imported", "Two"]);
    let ids: std::collections::HashSet<BlockId> =
        ed.document().paragraphs().map(|p| p.id).collect();
    assert_eq!(ids.len(), 4, "every id is unique");
    let chapter = ed.document().paragraph_at(1).expect("p");
    assert_eq!(
        chapter.props.style,
        StyleId::HEADING_1,
        "styles match by name"
    );
    assert_eq!(ed.caret().block, chapter.id);
    ed.undo().expect("undo");
    let after: Vec<BlockId> = ed.document().paragraphs().map(|p| p.id).collect();
    assert_eq!(before, after);
}

// ---------------------------------------------------------------- date and time fields

#[test]
fn an_inserted_date_field_shows_its_format_and_undoes() {
    let mut ed = editor(&["Updated "]);
    ed.set_caret(pos(&ed, 0, 8), false);
    let now = 1_760_000_000;
    ed.insert_date_time(now, DateTimeSource::Inserted, DateTimeFormat::DateLong)
        .expect("insert");
    let expected = format_date_time(now, DateTimeFormat::DateLong);
    assert_eq!(text_at(&ed, 0), format!("Updated {expected}"));
    let paragraph = ed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.date_fields().len(), 1);
    let field = &paragraph.date_fields()[0];
    assert_eq!(field.range, 8..8 + expected.len());
    assert_eq!(field.source, DateTimeSource::Inserted);
    assert_eq!(field.format, DateTimeFormat::DateLong);
    ed.undo().expect("undo the insertion");
    assert_eq!(text_at(&ed, 0), "Updated ");
    assert!(
        ed.document()
            .paragraph_at(0)
            .expect("paragraph")
            .date_fields()
            .is_empty()
    );
}

#[test]
fn an_updated_date_field_follows_the_documents_save_time() {
    let mut doc = doc(&["Saved on "]);
    doc.set_saved_at(1_700_000_000);
    let mut ed = Editor::new(doc);
    ed.set_caret(pos(&ed, 0, 9), false);
    ed.insert_date_time(
        1_700_000_000,
        DateTimeSource::Updated,
        DateTimeFormat::DateLong,
    )
    .expect("insert");
    let first = format_date_time(1_700_000_000, DateTimeFormat::DateLong);
    assert_eq!(text_at(&ed, 0), format!("Saved on {first}"));

    // The document is saved later: the new save time is recorded, then the field follows it.
    ed.mark_saved_at(1_800_000_000);
    ed.refresh_date_fields(1_800_000_000).expect("refresh");
    let second = format_date_time(1_800_000_000, DateTimeFormat::DateLong);
    assert_eq!(text_at(&ed, 0), format!("Saved on {second}"));
    assert_eq!(
        ed.document()
            .paragraph_at(0)
            .expect("paragraph")
            .date_fields()[0]
            .range,
        9..9 + second.len()
    );
    // A second refresh changes nothing and is not an undo step.
    assert!(
        ed.refresh_date_fields(1_800_000_000)
            .expect("refresh again")
            .dirty
            .is_empty(),
        "a second refresh changes nothing"
    );
    // Undo puts the earlier text and its field back.
    ed.undo().expect("undo the refresh");
    assert_eq!(text_at(&ed, 0), format!("Saved on {first}"));
    assert_eq!(
        ed.document()
            .paragraph_at(0)
            .expect("paragraph")
            .date_fields()[0]
            .range,
        9..9 + first.len()
    );
}

#[test]
fn date_and_time_formats_show_the_parts_they_name() {
    let stamp = 1_760_000_000;
    let date = format_date_time(stamp, DateTimeFormat::DateIso);
    let time = format_date_time(stamp, DateTimeFormat::Time24);
    let both = format_date_time(stamp, DateTimeFormat::DateTimeIso);
    assert_eq!(date.len(), 10, "2026-11-12");
    assert!(date.starts_with("20") && date.matches('-').count() == 2);
    assert_eq!(time.len(), 5, "14:35");
    assert_eq!(both, format!("{date} {time}"));
    assert_eq!(DateTimeFormat::DateLong.content(), DateTimeContent::Date);
    assert_eq!(DateTimeFormat::Time12.content(), DateTimeContent::Time);
    assert_eq!(
        DateTimeFormat::DateTimeLong.content(),
        DateTimeContent::DateAndTime
    );
}

#[test]
fn the_toc_lists_every_heading_level_and_updates_with_the_document() {
    let mut doc = doc(&[
        "Contents", "", "Chapter", "", "Section", "", "Detail", "", "Note",
    ]);
    let ids: Vec<BlockId> = (0..9)
        .map(|i| doc.paragraph_at(i).expect("paragraph").id)
        .collect();
    // Heading levels 1 to 4.
    for (index, level) in [(2usize, 1u8), (4, 2), (6, 3), (8, 4)] {
        let mut props = doc.paragraph(ids[index]).expect("paragraph").props;
        props.outline_level = Some(level);
        let mut b = EditBuilder::default();
        b.apply(
            &mut doc,
            Operation::SetParaProps {
                block: ids[index],
                props,
            },
        )
        .expect("heading");
        b.record(&mut History::default(), EditKind::Other, Instant::now());
    }
    let mut ed = Editor::new(doc);
    ed.set_caret(pos(&ed, 1, 0), false);
    ed.insert_table_of_contents().expect("toc");
    let entries: Vec<(String, Option<f32>)> = ed
        .document()
        .paragraphs()
        .filter(|p| p.props.toc_target.is_some())
        .map(|p| (p.text().to_owned(), p.props.format.indent_left_pt))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("Chapter".to_owned(), None),
            ("Section".to_owned(), Some(18.0)),
            ("Detail".to_owned(), Some(36.0)),
            ("Note".to_owned(), Some(54.0)),
        ],
        "every level is listed, indented by depth"
    );

    // Renaming a heading updates its entry in the same undo step as the edit.
    let section = ids[4];
    let offset = ed
        .document()
        .paragraph(section)
        .expect("section")
        .text()
        .len();
    ed.set_caret(
        DocPosition {
            block: section,
            offset,
        },
        false,
    );
    ed.insert_text(" Two").expect("rename");
    let renamed = ed
        .document()
        .paragraphs()
        .find(|p| p.props.toc_target == Some(section))
        .expect("entry");
    assert_eq!(renamed.text(), "Section Two");

    // Promoting the section to Heading 1 removes its indent.
    let mut props = ed.document().paragraph(section).expect("section").props;
    props.outline_level = Some(1);
    let mut b = EditBuilder::default();
    b.apply(
        &mut ed.doc,
        Operation::SetParaProps {
            block: section,
            props,
        },
    )
    .expect("level");
    b.toc = true;
    ed.commit(&mut b, EditKind::Other, Instant::now())
        .expect("sync");
    let promoted = ed
        .document()
        .paragraphs()
        .find(|p| p.props.toc_target == Some(section))
        .expect("entry");
    assert_eq!(
        promoted.props.format.indent_left_pt, None,
        "level 1 has no indent"
    );
}

#[test]
fn page_setup_at_the_caret_changes_only_its_section() {
    let mut ed = book();
    assert_eq!(ed.document().sections.len(), 2);
    let second = ed
        .document()
        .paragraphs()
        .find(|p| {
            ed.document()
                .index_of(p.id)
                .is_some_and(|index| ed.document().section_of(index) == 1)
        })
        .map(|p| p.id)
        .expect("a paragraph in the second section");
    ed.set_caret(
        DocPosition {
            block: second,
            offset: 0,
        },
        false,
    );
    let letter = PageSetup {
        paper: PaperSize::Letter,
        orientation: Orientation::Portrait,
        margins: Margins::uniform(54.0),
    };
    ed.set_page_setup_at_caret(letter).expect("section setup");
    assert_eq!(ed.document().sections[0].page, PageSetup::a4());
    assert_eq!(ed.document().sections[1].page, letter);
    ed.undo().expect("undo");
    assert_eq!(ed.document().sections[1].page, PageSetup::a4());
}

// ---------------------------------------------------------------- outside edits

#[test]
fn replace_range_is_one_undo_step_and_undo_is_exact() {
    let mut ed = editor(&["hello world", "second"]);
    let before = ed.document().clone();
    let change = ed
        .replace_range(pos(&ed, 0, 6), pos(&ed, 0, 11), "there")
        .unwrap();
    assert!(!change.dirty.is_empty());
    assert_eq!(text_at(&ed, 0), "hello there");
    assert_eq!(ed.history().undo_len(), 1);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&before));
    ed.redo().unwrap();
    assert_eq!(text_at(&ed, 0), "hello there");
}

#[test]
fn replace_range_with_empty_text_deletes_and_across_paragraphs_joins() {
    let mut ed = editor(&["alpha", "beta"]);
    ed.replace_range(pos(&ed, 0, 2), pos(&ed, 1, 2), "")
        .unwrap();
    assert_eq!(ed.document().len(), 1);
    assert_eq!(text_at(&ed, 0), "alta");
    assert_eq!(ed.history().undo_len(), 1);
    // Nothing to delete is not an edit.
    let n = ed.history().undo_len();
    ed.replace_range(pos(&ed, 0, 1), pos(&ed, 0, 1), "")
        .unwrap();
    assert_eq!(ed.history().undo_len(), n);
}

#[test]
fn replace_range_does_not_coalesce_with_the_writers_typing() {
    let mut ed = editor(&["abc", ""]);
    let t0 = Instant::now();
    ed.set_caret(pos(&ed, 1, 0), false);
    ed.insert_text_at(t0, "x").unwrap();
    // A single character inserted by someone else must not join the typing run.
    ed.replace_range(pos(&ed, 0, 0), pos(&ed, 0, 0), "y")
        .unwrap();
    assert_eq!(ed.history().undo_len(), 2);
    ed.undo().unwrap();
    assert_eq!(text_at(&ed, 0), "abc");
    assert_eq!(text_at(&ed, 1), "x");
}

#[test]
fn replace_range_keeps_the_writers_selection_and_pending_format() {
    let mut ed = editor(&["first line", "second line"]);
    ed.set_selection(pos(&ed, 1, 2), pos(&ed, 1, 6));
    let saved = ed.selection();
    ed.replace_range(pos(&ed, 0, 0), pos(&ed, 0, 5), "FIRST")
        .unwrap();
    assert_eq!(ed.selection(), saved);

    // Pending bold for the writer's caret survives an outside edit elsewhere.
    ed.set_caret(pos(&ed, 1, 0), false);
    ed.format_selection(&CharFormat {
        weight: Some(700),
        ..CharFormat::default()
    })
    .unwrap();
    ed.replace_range(pos(&ed, 0, 0), pos(&ed, 0, 0), "z")
        .unwrap();
    ed.insert_text("B").unwrap();
    let p = ed.document().paragraph_at(1).unwrap();
    assert!(p.runs().iter().any(|r| r.format.weight == Some(700)));
}

#[test]
fn replace_range_moves_the_caret_when_it_edits_the_writers_paragraph() {
    let mut ed = editor(&["abcdef"]);
    ed.set_caret(pos(&ed, 0, 4), false);
    ed.replace_range(pos(&ed, 0, 0), pos(&ed, 0, 6), "xy")
        .unwrap();
    assert_eq!(ed.caret(), pos(&ed, 0, 2));
}

#[test]
fn replace_range_rejects_bad_ranges_without_changing_anything() {
    let mut ed = editor(&["héllo", "two"]);
    let before = ed.document().clone();
    let bad = [
        (pos(&ed, 0, 0), pos(&ed, 0, 99)),
        (pos(&ed, 0, 2), pos(&ed, 0, 3)), // inside the two-byte é
        (pos(&ed, 1, 0), pos(&ed, 0, 0)), // start after end
        (
            DocPosition {
                block: BlockId(9999),
                offset: 0,
            },
            pos(&ed, 0, 0),
        ),
    ];
    for (a, b) in bad {
        assert!(matches!(
            ed.replace_range(a, b, "x"),
            Err(EditorError::InvalidRange(_))
        ));
    }
    assert!(ed.document().content_eq(&before));
    assert_eq!(ed.history().undo_len(), 0);
}

#[test]
fn replace_range_inserts_paragraph_breaks_even_on_an_empty_list_item() {
    let mut ed = editor(&["item"]);
    ed.set_caret(pos(&ed, 0, 4), false);
    ed.set_list(Some(ListStyle::Bullet)).ok();
    ed.replace_range(pos(&ed, 0, 4), pos(&ed, 0, 4), "\nmore")
        .unwrap();
    assert_eq!(ed.document().len(), 2);
    assert_eq!(text_at(&ed, 1), "more");
}

#[test]
fn format_range_is_one_undo_step_and_leaves_the_selection() {
    let mut ed = editor(&["hello world"]);
    ed.set_selection(pos(&ed, 0, 1), pos(&ed, 0, 2));
    let saved = ed.selection();
    let before = ed.document().clone();
    ed.format_range(
        pos(&ed, 0, 0),
        pos(&ed, 0, 5),
        &CharFormat {
            weight: Some(700),
            ..CharFormat::default()
        },
    )
    .unwrap();
    assert_eq!(ed.selection(), saved);
    assert_eq!(ed.history().undo_len(), 1);
    ed.undo().unwrap();
    assert!(ed.document().content_eq(&before));
}

#[test]
fn format_range_on_an_empty_range_changes_nothing() {
    let mut ed = editor(&["hello"]);
    let change = ed
        .format_range(
            pos(&ed, 0, 2),
            pos(&ed, 0, 2),
            &CharFormat {
                italic: Some(true),
                ..CharFormat::default()
            },
        )
        .unwrap();
    assert!(change.dirty.is_empty());
    assert_eq!(ed.history().undo_len(), 0);
    // No pending format was left for the next typing either.
    ed.set_caret(pos(&ed, 0, 2), false);
    ed.insert_text("x").unwrap();
    let p = ed.document().paragraph_at(0).unwrap();
    assert!(p.runs().iter().all(|r| r.format.italic.is_none()));
}

#[test]
fn set_paragraph_style_at_changes_only_that_paragraph() {
    let mut ed = editor(&["one", "two"]);
    let style = StyleId(
        ed.document()
            .styles
            .paragraph
            .iter()
            .position(|s| s.name.starts_with("Heading"))
            .expect("a heading style") as _,
    );
    ed.set_caret(pos(&ed, 0, 1), false);
    let saved = ed.selection();
    ed.set_paragraph_style_at(1, style).unwrap();
    assert_eq!(ed.selection(), saved);
    assert_eq!(ed.document().paragraph_at(1).unwrap().props.style, style);
    assert_ne!(ed.document().paragraph_at(0).unwrap().props.style, style);
    assert_eq!(ed.history().undo_len(), 1);
    assert!(matches!(
        ed.set_paragraph_style_at(9, style),
        Err(EditorError::InvalidRange(_))
    ));
}

#[test]
fn separate_insert_is_its_own_undo_step_and_is_not_made_bold_by_pending_format() {
    let mut ed = editor(&["abc"]);
    let t0 = Instant::now();
    ed.set_caret(pos(&ed, 0, 3), false);
    ed.insert_text_at(t0, "x").unwrap();
    // One character right after typing would normally join the typing run.
    ed.insert_text_separately("y").unwrap();
    assert_eq!(text_at(&ed, 0), "abcxy");
    assert_eq!(ed.history().undo_len(), 2);
    ed.undo().unwrap();
    assert_eq!(text_at(&ed, 0), "abcx");

    // Pending bold is for the writer's next keystroke, not for a tool's output.
    ed.format_selection(&CharFormat {
        weight: Some(700),
        ..CharFormat::default()
    })
    .unwrap();
    ed.insert_text_separately("Z").unwrap();
    let p = ed.document().paragraph_at(0).unwrap();
    let whole = text_at(&ed, 0);
    assert!(
        p.runs()
            .iter()
            .filter(|r| r.format.weight == Some(700))
            .all(|r| !whole[r.range.clone()].contains('Z')),
        "the inserted text is not bold"
    );
}

/// The segments a run of word movements visits, from the start of `text`.
fn words_of(text: &str) -> Vec<&str> {
    let mut at = 0;
    let mut parts = Vec::new();
    while at < text.len() {
        let next = next_word_boundary(text, at);
        parts.push(&text[at..next]);
        at = next;
    }
    parts
}

#[test]
fn chinese_and_japanese_are_split_into_words_by_a_dictionary() {
    // By the plain Unicode rules each of these would be one word per character.
    assert_eq!(words_of("北京欢迎你"), ["北京", "欢迎", "你"]);
    assert_eq!(
        words_of("我喜欢北京天安门"),
        ["我", "喜欢", "北京", "天安门"]
    );
    assert_eq!(
        words_of("私は東京大学に行きます"),
        ["私", "は", "東京", "大学", "に", "行き", "ます"]
    );
    assert_eq!(
        words_of("今天天气很好，我们去公园玩。"),
        [
            "今天", "天气", "很好", "，", "我们", "去", "公园", "玩", "。"
        ]
    );
    // Latin text around East Asian text keeps its words and spaces.
    assert_eq!(
        words_of("Hello 世界 foo"),
        ["Hello", " ", "世界", " ", "foo"]
    );
    // Korean has spaces, Latin and Cyrillic are unchanged.
    assert_eq!(words_of("동해물과 백두산이"), ["동해물과", " ", "백두산이"]);
    assert_eq!(words_of("Доброго ранку"), ["Доброго", " ", "ранку"]);
}

#[test]
fn word_boundaries_agree_going_backwards_and_forwards_in_cjk_text() {
    let text = "我喜欢北京天安门";
    let ends = [3, 9, 15, 24];
    assert_eq!(next_word_boundary(text, 0), ends[0]);
    assert_eq!(next_word_boundary(text, ends[0]), ends[1]);
    // Mid-word, forward goes to the end of the word and backward to its start.
    assert_eq!(next_word_boundary(text, 4), ends[1]);
    assert_eq!(prev_word_boundary(text, 7), ends[0]);
    assert_eq!(prev_word_boundary(text, ends[1]), ends[0]);
    assert_eq!(prev_word_boundary(text, 0), 0);
    assert_eq!(next_word_boundary(text, text.len()), text.len());
    // Always on a character boundary, even when asked from inside a character.
    for offset in 0..=text.len() {
        assert!(text.is_char_boundary(next_word_boundary(text, offset)));
        assert!(text.is_char_boundary(prev_word_boundary(text, offset)));
    }
}

#[test]
fn word_movement_and_deleting_a_word_work_on_chinese_words() {
    let mut ed = editor(&["北京欢迎你"]);
    ed.move_doc_start(false);
    ed.move_word_right(false);
    assert_eq!(ed.caret().offset, "北京".len());
    ed.move_word_right(true);
    assert_eq!(ed.selected_text().as_deref(), Some("欢迎"));
    ed.move_doc_end(false);
    ed.delete_word_backward().expect("delete");
    assert_eq!(ed.document().paragraph_at(0).expect("p").text(), "北京欢迎");
}

#[test]
fn vertical_text_goes_on_every_section_relays_out_and_undoes_in_one_step() {
    let mut ed = editor(&["One", "Two"]);
    let mut d = ed.document().clone();
    d.sections.push(libertum_model::Section::default());
    ed.replace_document(d);
    assert!(!ed.vertical_text());
    let change = ed.set_vertical_text(true).expect("set");
    assert!(change.all_styles, "every paragraph is laid out again");
    assert!(ed.vertical_text());
    assert!(ed.document().sections.iter().all(|s| s.vertical));
    // The same again is not an undo step.
    let steps = ed.history().undo_len();
    assert!(ed.set_vertical_text(true).expect("same").is_empty());
    assert_eq!(ed.history().undo_len(), steps);
    ed.undo().expect("undo");
    assert!(!ed.vertical_text());
    assert!(ed.document().sections.iter().all(|s| !s.vertical));
}

#[test]
fn ruby_is_set_over_the_selection_and_undone() {
    let mut ed = editor(&["漢字を読む"]);
    // Nothing selected, no ruby under the caret: nothing to set.
    assert!(ed.ruby_target().is_none());
    ed.set_selection(pos(&ed, 0, 0), pos(&ed, 0, 6));
    let (_, range, current) = ed.ruby_target().expect("target");
    assert_eq!(range, 0..6);
    assert_eq!(current, "");
    ed.set_ruby("かんじ").expect("set");
    assert_eq!(
        ed.document().paragraph_at(0).unwrap().ruby()[0].text,
        "かんじ"
    );
    // The caret inside it finds the same ruby.
    ed.set_caret(pos(&ed, 0, 3), false);
    let (_, range, current) = ed.ruby_target().expect("target at the caret");
    assert_eq!((range, current.as_str()), (0..6, "かんじ"));
    // An empty reading removes it, and undo brings it back.
    ed.set_ruby("").expect("clear");
    assert!(ed.document().paragraph_at(0).unwrap().ruby().is_empty());
    ed.undo().expect("undo");
    assert_eq!(ed.document().paragraph_at(0).unwrap().ruby().len(), 1);
}

#[test]
fn two_languages_are_set_together_and_the_second_never_equals_the_first() {
    let mut ed = editor(&["Hello"]);
    ed.set_languages(Some("en-GB"), Some("uk")).expect("set");
    assert_eq!((ed.language(), ed.language2()), (Some("en-GB"), Some("uk")));
    // Changing the first keeps the second.
    ed.set_language(Some("en-US")).expect("set");
    assert_eq!((ed.language(), ed.language2()), (Some("en-US"), Some("uk")));
    // A second that equals the first is dropped; one undo brings the pair back.
    ed.set_languages(Some("uk"), Some("uk-UA")).expect("set");
    assert_eq!(ed.language(), Some("uk"));
    assert_eq!(
        ed.language2(),
        Some("uk-UA"),
        "regional forms differ as tags"
    );
    ed.set_languages(Some("uk"), Some("uk")).expect("set");
    assert_eq!(ed.language2(), None);
    ed.undo().expect("undo");
    assert_eq!(ed.language2(), Some("uk-UA"));
}
