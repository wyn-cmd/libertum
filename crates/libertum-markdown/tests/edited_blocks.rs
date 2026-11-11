//! What an edited block keeps through the crate's own per-block notes: ordered-list start
//! numbers, hyperlinks, code block lines and fences, quote depth, heading levels and table
//! column alignment.

use libertum_markdown::Markdown;
use libertum_model::{Block, Operation};

/// Opens `source`, inserts `text` at `offset` of the first paragraph containing `containing`
/// (or at its end), and returns what saving writes.
fn edit(source: &str, containing: &str, offset: Option<usize>, text: &str) -> String {
    let mut markdown = Markdown::parse(source.as_bytes()).expect("parse");
    let mut document = markdown.document().clone();
    let paragraph = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains(containing))
        .unwrap_or_else(|| panic!("no paragraph containing {containing:?}"));
    let (id, end) = (paragraph.id, paragraph.text().len());
    document
        .apply(Operation::InsertText {
            block: id,
            offset: offset.unwrap_or(end),
            text: text.to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
    String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8")
}

#[test]
fn an_ordered_list_keeps_its_start_number() {
    let saved = edit("3. alpha\n4. beta\n5. gamma\n", "beta", None, "!");
    assert_eq!(saved, "3. alpha\n4. beta!\n5. gamma\n");
}

#[test]
fn a_nested_ordered_list_keeps_its_own_start_number() {
    let source = "1. one

   7. deep a
   8. deep b

2. two
";
    let saved = edit(source, "deep b", None, "!");
    assert!(saved.contains("1. one\n"), "{saved}");
    assert!(saved.contains("    7. deep a\n"), "{saved}");
    assert!(saved.contains("    8. deep b!\n"), "{saved}");
    assert!(saved.contains("2. two\n"), "{saved}");
}

#[test]
fn a_link_survives_an_edit_elsewhere_in_the_paragraph() {
    let saved = edit(
        "See [the site](https://example.com \"Home\") now.\n",
        "See",
        None,
        " More.",
    );
    assert_eq!(
        saved,
        "See [the site](https://example.com \"Home\") now. More.\n"
    );
}

#[test]
fn a_link_survives_typing_inside_its_text() {
    let saved = edit(
        "See [the site](https://example.com) now.\n",
        "See",
        Some("See the".len()),
        "XY",
    );
    assert_eq!(saved, "See [theXY site](https://example.com) now.\n");
}

#[test]
fn text_typed_in_front_of_a_link_stays_outside_it() {
    let saved = edit(
        "See [the site](https://example.com) now.\n",
        "See",
        Some("See ".len()),
        "really ",
    );
    assert_eq!(saved, "See really [the site](https://example.com) now.\n");
}

#[test]
fn text_typed_at_the_end_of_a_link_stays_outside_it() {
    let saved = edit(
        "See [the site](https://example.com) now.\n",
        "See",
        Some("See the site".len()),
        "s",
    );
    assert_eq!(saved, "See [the site](https://example.com)s now.\n");
}

#[test]
fn a_link_with_a_formatted_part_keeps_both() {
    let saved = edit(
        "A [bold **word** link](http://a.example/x) end.\n",
        "A bold",
        None,
        "!",
    );
    assert_eq!(saved, "A [bold **word** link](http://a.example/x) end.!\n");
}

#[test]
fn an_autolink_is_written_as_one() {
    let saved = edit(
        "Go to <https://example.org/a_b> today.\n",
        "Go to",
        None,
        "!",
    );
    assert_eq!(saved, "Go to <https://example.org/a_b> today.!\n");
}

#[test]
fn a_link_whose_text_is_deleted_is_dropped() {
    let mut markdown = Markdown::parse(b"A [x](http://e.example) b\n").expect("parse");
    let mut document = markdown.document().clone();
    let id = document.first_id().expect("paragraph");
    document
        .apply(Operation::DeleteRange {
            block: id,
            range: 2..3,
        })
        .expect("delete");
    markdown.set_document(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert_eq!(saved, "A  b\n");
}

#[test]
fn a_code_block_keeps_its_fence_info_string_and_lines_when_edited() {
    let source = "~~~rust\nfn a() {\n    one();\n    two();\n}\n~~~\n";
    let saved = edit(
        source,
        "one();",
        Some("fn a() {     one();     ".len()),
        "x",
    );
    assert_eq!(
        saved,
        "~~~rust\nfn a() {\n    one();\n    xtwo();\n}\n~~~\n"
    );
}

#[test]
fn a_code_block_that_holds_a_fence_gets_a_longer_one() {
    let source = "```\nplain\n```\n";
    let saved = edit(source, "plain", Some(0), "``` ");
    assert_eq!(saved, "````\n``` plain\n````\n");
}

#[test]
fn a_nested_quote_keeps_its_depth() {
    let saved = edit("> > deep\n", "deep", None, "er");
    assert_eq!(saved, "> > deeper\n");
    let saved = edit("> one\n", "one", None, "!");
    assert_eq!(saved, "> one!\n");
}

#[test]
fn a_heading_keeps_its_real_level_when_edited() {
    for level in 1..=6usize {
        let source = format!("{} Heading\n", "#".repeat(level));
        let saved = edit(&source, "Heading", None, "!");
        assert_eq!(saved, format!("{} Heading!\n", "#".repeat(level)));
    }
}

#[test]
fn table_column_alignment_is_kept_when_a_cell_is_edited() {
    let source = "| a | b | c | d |\n|:---|:---:|---:|---|\n| 1 | 2 | 3 | 4 |\n";
    let mut markdown = Markdown::parse(source.as_bytes()).expect("parse");
    let mut document = markdown.document().clone();
    let table = document
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("table");
    let id = table.rows[1].cells[0].paragraphs[0].id;
    document
        .apply(Operation::InsertText {
            block: id,
            offset: 1,
            text: "0".to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert!(saved.contains("| :--- | :---: | ---: | --- |"), "{saved}");
    assert!(saved.contains("| 10 | 2 | 3 | 4 |"), "{saved}");
}

#[test]
fn bold_text_inside_a_code_span_is_written() {
    // A code span that is also bold: both survive, which the first version dropped.
    let saved = edit("Use **`cargo`** now.\n", "Use", None, "!");
    assert_eq!(saved, "Use **`cargo`** now.!\n");
}
