//! Round-trip and fixture tests for `libertum-markdown` (PLAN §7).

use libertum_markdown::Markdown;
use libertum_model::{Block, ListStyle, Operation, StyleId};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../tests/fixtures/md/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).unwrap_or_else(|error| panic!("fixture {name}: {error}"))
}

#[test]
fn a_clean_open_then_save_is_byte_identical() {
    for name in [
        "conventions.md",
        "setext.md",
        "html.md",
        "crlf.md",
        "bom.md",
        "empty.md",
        "no-final-newline.md",
        "underscore.md",
        "image.md",
    ] {
        let bytes = fixture(name);
        let markdown = Markdown::parse(&bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        let saved = markdown.to_bytes().expect("save");
        assert_eq!(
            saved,
            bytes,
            "{name} changed on a save without edits:\n{}",
            String::from_utf8_lossy(&saved)
        );
    }
}

#[test]
fn reads_gfm_blocks_into_the_model() {
    let markdown = Markdown::parse(&fixture("conventions.md")).expect("parse");
    let document = markdown.document();

    let atx = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "ATX Heading")
        .expect("ATX heading");
    assert_eq!(atx.props.style, StyleId::HEADING_1);
    assert_eq!(atx.props.outline_level, Some(1));

    let setext = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "Setext Heading")
        .expect("Setext heading");
    assert_eq!(setext.props.outline_level, Some(1));

    let paragraph = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains("A paragraph with"))
        .expect("paragraph");
    assert!(
        paragraph
            .runs()
            .iter()
            .any(|run| run.format.italic == Some(true))
    );
    assert!(
        paragraph
            .runs()
            .iter()
            .any(|run| run.format.weight.is_some_and(|weight| weight >= 600))
    );
    assert!(
        paragraph
            .runs()
            .iter()
            .any(|run| run.format.strikethrough == Some(true))
    );
    assert!(paragraph.runs().iter().any(|run| {
        run.format
            .family
            .as_deref()
            .is_some_and(|f| f.contains("Courier"))
    }));

    let bullet = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "star bullet")
        .expect("bullet");
    let item = bullet.props.list.expect("list item");
    assert_eq!(item.style, ListStyle::Bullet);
    assert_eq!(item.level, 0);
    let nested = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "nested item")
        .expect("nested item");
    let nested_item = nested.props.list.expect("nested item");
    assert_eq!(nested_item.level, 1);
    assert_eq!(nested_item.group, item.group, "same list");

    let ordered = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "first ordered")
        .expect("ordered item");
    assert_eq!(ordered.props.list.expect("list").style, ListStyle::Numbered);

    let quote = document
        .paragraphs()
        .find(|paragraph| paragraph.text().starts_with("A quote with"))
        .expect("quote");
    assert_eq!(quote.props.style, StyleId::QUOTE);

    let code = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains("println!"))
        .expect("code block");
    assert_eq!(
        document
            .styles
            .get(code.props.style)
            .map(|style| style.name.as_str()),
        Some("Markdown Code")
    );

    let task_open = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "[ ] task open")
        .expect("open task");
    assert_eq!(task_open.props.list.expect("list").style, ListStyle::Bullet);
    assert!(
        document
            .paragraphs()
            .any(|paragraph| paragraph.text() == "[x] task done")
    );

    let table = document
        .body()
        .iter()
        .find_map(|block| block.as_table())
        .expect("table");
    assert_eq!(table.widths_pt.len(), 2);
    assert_eq!(table.header_rows, 1);
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.rows[0].cells[0].text(), "Left");

    let raw = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains("raw html"))
        .expect("html block");
    assert_eq!(
        document
            .styles
            .get(raw.props.style)
            .map(|style| style.name.as_str()),
        Some("Markdown Raw")
    );

    let footnote = document
        .paragraphs()
        .find(|paragraph| paragraph.text().starts_with("[^1]:"))
        .expect("footnote definition");
    assert!(footnote.text().contains("The footnote text."));

    assert!(
        markdown
            .warnings()
            .iter()
            .any(|warning| warning.contains("hyperlink")),
        "a link is flattened with a warning: {:?}",
        markdown.warnings()
    );
    assert!(
        document
            .paragraphs()
            .any(|paragraph| paragraph.text().contains("[^1]")),
        "the footnote reference is kept as text"
    );
}

#[test]
fn editing_one_block_keeps_the_other_source_verbatim() {
    let bytes = fixture("conventions.md");
    let mut markdown = Markdown::parse(&bytes).expect("parse");
    let mut document = markdown.document().clone();
    let paragraph = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains("A paragraph with"))
        .expect("paragraph");
    let id = paragraph.id;
    let offset = paragraph.text().len();
    document
        .apply(Operation::InsertText {
            block: id,
            offset,
            text: " EDITED.".to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");

    assert!(saved.contains("EDITED."), "{saved}");
    assert!(
        saved.starts_with("---\ntitle: Conventions\n---\n"),
        "front matter kept"
    );
    assert!(
        saved.contains("# ATX Heading"),
        "the untouched heading keeps ATX"
    );
    assert!(saved.contains("* star bullet"), "the star bullets are kept");
    assert!(
        saved.contains("1) first ordered"),
        "the paren delimiter is kept"
    );
    assert!(saved.contains("~~~rust"), "the tilde fence is kept");
    assert!(saved.contains("| Left | Right |"), "the table is kept");
}

#[test]
fn editing_a_setext_heading_follows_the_file() {
    let mut markdown = Markdown::parse(&fixture("setext.md")).expect("parse");
    let mut document = markdown.document().clone();
    let heading = document
        .paragraphs()
        .find(|paragraph| paragraph.text() == "Setext One")
        .expect("heading");
    let id = heading.id;
    document
        .apply(Operation::InsertText {
            block: id,
            offset: heading.text().len(),
            text: " Two".to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert!(
        saved.contains("Setext One Two\n=============="),
        "a setext file keeps setext headings:\n{saved}"
    );
}

#[test]
fn edited_italics_follow_the_underscore_convention() {
    let mut markdown = Markdown::parse(&fixture("underscore.md")).expect("parse");
    let mut document = markdown.document().clone();
    let paragraph = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains("italic"))
        .expect("paragraph");
    let id = paragraph.id;
    let offset = paragraph.text().len();
    document
        .apply(Operation::InsertText {
            block: id,
            offset,
            text: " More.".to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert!(
        saved.contains("_italic_"),
        "underscore emphasis is kept:\n{saved}"
    );
    assert!(saved.contains(String::from(" More.").as_str()));
}

#[test]
fn a_new_document_is_written_as_gfm() {
    let document = Markdown::parse(&fixture("conventions.md"))
        .expect("parse")
        .into_document();
    let markdown = Markdown::new(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("write")).expect("utf-8");
    assert!(saved.contains("# ATX Heading"), "{saved}");
    assert!(saved.contains("- star bullet"), "{saved}");
    assert!(saved.contains("1. first ordered"), "{saved}");
    assert!(saved.contains("| Left | Right |"), "{saved}");
    assert!(saved.contains("> A quote with"), "{saved}");
    assert!(saved.contains("```"), "code uses a fence");
    assert!(
        saved.contains("The footnote text."),
        "footnote text is kept"
    );
}

#[test]
fn tables_serialise_with_a_header_row_and_escaped_pipes() {
    let document = Markdown::parse(b"| a | b |\n|---|---|\n| x\\|y | z |\n")
        .expect("parse")
        .into_document();
    let markdown = Markdown::new(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("write")).expect("utf-8");
    assert!(saved.contains("| a | b |"), "{saved}");
    assert!(saved.contains("| --- | --- |"), "{saved}");
    assert!(saved.contains("x\\|y"), "{saved}");
}

#[test]
fn an_empty_document_round_trips() {
    let markdown = Markdown::parse(b"").expect("parse");
    assert_eq!(markdown.to_bytes().expect("save"), b"");
    assert_eq!(
        markdown.document().body().iter().count(),
        1,
        "the model always has one empty paragraph"
    );
    let _ = Block::Paragraph;
}
