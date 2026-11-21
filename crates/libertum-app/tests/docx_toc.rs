//! A table of contents inserted into a document that came from Word, where headings are marked
//! by style and not by anything on the paragraph.

use std::collections::HashMap;

use libertum_docx::Docx;
use libertum_editor::Editor;
use libertum_model::BlockId;

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/docx/rich.docx"
    ))
    .expect("rich.docx fixture")
}

#[test]
fn a_table_of_contents_inserted_into_a_word_document_lists_its_headings() {
    let docx = Docx::parse(&fixture()).expect("parse");
    let before: Vec<String> = docx
        .document()
        .paragraphs()
        .map(|p| p.text().to_owned())
        .collect();
    let mut editor = Editor::new(docx.document().clone());
    editor.insert_table_of_contents().expect("insert");

    let after: Vec<(String, bool)> = editor
        .document()
        .paragraphs()
        .map(|p| (p.text().to_owned(), p.props.toc_target.is_some()))
        .collect();
    // The title and one entry for "Chapter One", not the empty placeholder.
    assert_eq!(after[0], ("Contents".to_owned(), false));
    assert_eq!(after[1], ("Chapter One".to_owned(), true));
    // Every paragraph the document had is still there, in order, after the new ones:
    // nothing was overwritten.
    let kept: Vec<String> = after[2..].iter().map(|(text, _)| text.clone()).collect();
    assert_eq!(kept, before);
}

fn word_toc_fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/docx/word-toc.docx"
    ))
    .expect("word-toc.docx fixture")
}

fn entries(editor: &Editor) -> Vec<(String, Option<u64>)> {
    editor
        .document()
        .paragraphs()
        .filter(|p| p.props.toc_target.is_some())
        .map(|p| (p.text().to_owned(), p.props.toc_target.map(|t| t.0)))
        .collect()
}

#[test]
fn a_word_table_of_contents_is_rebuilt_saved_reopened_and_still_updates() {
    // Open: the Word TOC becomes a placeholder, and refreshing fills it from the headings.
    let docx = Docx::parse(&word_toc_fixture()).expect("a Word TOC opens");
    let mut editor = Editor::new(docx.document().clone());
    editor.refresh_table_of_contents().expect("refresh");
    let first = entries(&editor);
    assert_eq!(
        first
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        ["Chapter One", "Chapter Two"]
    );
    // Each entry points at its heading.
    for (text, target) in &first {
        let heading = editor
            .document()
            .paragraphs()
            .find(|p| p.props.toc_target.is_none() && p.text() == text && p.props.is_heading())
            .expect("the heading");
        assert_eq!(*target, Some(heading.id.0));
    }

    // Save with page numbers, as the app does.
    let pages: HashMap<BlockId, u32> = first
        .iter()
        .enumerate()
        .map(|(i, (_, target))| (BlockId(target.expect("target")), i as u32 + 1))
        .collect();
    let mut out = Docx::new(editor.document().clone());
    out.set_toc_pages(pages);
    let saved = out.to_bytes().expect("save");

    // Reopen: no warning this time, because the table of contents is Libertum's own.
    let reopened = Docx::parse(&saved).expect("reopen");
    assert!(reopened.warnings().is_empty(), "{:?}", reopened.warnings());
    let mut editor = Editor::new(reopened.document().clone());
    editor.refresh_table_of_contents().expect("refresh");
    let again = entries(&editor);
    assert_eq!(
        again
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        ["Chapter One", "Chapter Two"]
    );

    // And it still updates: a new heading joins the list.
    let index = editor
        .document()
        .paragraphs()
        .position(|p| p.text() == "The morning came.")
        .expect("a body paragraph");
    editor
        .set_paragraph_style_at(index, libertum_model::StyleId::HEADING_1)
        .expect("make it a heading");
    let updated = entries(&editor);
    assert_eq!(
        updated
            .iter()
            .map(|(text, _)| text.as_str())
            .collect::<Vec<_>>(),
        ["Chapter One", "Chapter Two", "The morning came."]
    );
}
