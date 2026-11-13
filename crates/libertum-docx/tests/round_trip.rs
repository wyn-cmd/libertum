//! Round-trip and fixture tests for `libertum-docx` (PLAN §6.2, §8).

use std::io::{Cursor, Read, Write};

use libertum_docx::{Docx, DocxError};
use libertum_formats_common::media::sample_png;
use libertum_model::{
    Align, Block, BlockId, Bookmark, CharFormat, CrossRef, CrossRefKind, Document, InlineImage,
    LineSpacing, ListItem, ListStyle, Margins, Media, Note, NoteKind, Orientation, PageSetup,
    PaperSize, ParaFormat, ParaProps, Paragraph, Run, Section, StyleId, Table, TableBorders,
    TableCell, TableRow, TableStyle,
};

fn fixture() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/docx/rich.docx"
    );
    std::fs::read(path).expect("rich.docx fixture")
}

fn table_fixture() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/docx/table.docx"
    );
    std::fs::read(path).expect("table.docx fixture")
}

fn zip_text(bytes: &[u8], name: &str) -> Option<String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut file = archive.by_name(name).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    Some(text)
}

fn zip_package(parts: &[(&str, &str)]) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        for (name, text) in parts {
            writer.start_file(*name, options).expect("zip entry");
            writer.write_all(text.as_bytes()).expect("write");
        }
        writer.finish().expect("finish");
    }
    buffer.into_inner()
}

#[test]
fn reads_the_rich_fixture() {
    let parsed = Docx::parse(&fixture()).expect("parse");
    let doc = parsed.document();
    assert_eq!(doc.len(), 5, "five paragraphs");
    assert_eq!(doc.paragraph_at(0).expect("p0").text(), "Chapter One");
    assert_eq!(
        doc.paragraph_at(0).expect("p0").props.style,
        StyleId::HEADING_1
    );

    let bold = doc.paragraph_at(1).expect("p1");
    assert_eq!(bold.text(), "Bold plain text");
    let runs = bold.runs();
    assert_eq!(runs.len(), 2, "the character style and its override");
    assert_eq!(runs[0].range, 0..4);
    assert_eq!(
        runs[0].format.weight,
        Some(700),
        "bold comes from the Strong character style"
    );
    assert_eq!(
        runs[1].format.weight,
        Some(400),
        "direct formatting overrides the character style"
    );

    let list = doc
        .paragraph_at(2)
        .expect("p2")
        .props
        .list
        .expect("list item");
    assert_eq!(list.style, ListStyle::Bullet);
    assert_eq!(list.level, 0);

    assert_eq!(
        doc.paragraph_at(3).expect("p3").text(),
        "Two columns follow."
    );
    let second = doc.paragraph_at(4).expect("p4");
    assert_eq!(second.text(), "Second section.");
    assert!(second.props.section_start, "the second section opens here");

    assert_eq!(doc.sections.len(), 2);
    let first = &doc.sections[0];
    assert_eq!(first.columns, 2);
    assert!((first.column_gutter_pt - 36.0).abs() < 0.01);
    assert_eq!(first.first_page_number, Some(3));
    assert_eq!(first.page.paper, PaperSize::A4);
    assert_eq!(first.page.orientation, Orientation::Landscape);
    assert_eq!(doc.sections[1].columns, 1);
    assert_eq!(doc.sections[1].page.orientation, Orientation::Portrait);

    // Document defaults and the heading style come from styles.xml.
    assert_eq!(doc.styles.defaults.family.as_ref(), "Georgia");
    assert!((doc.styles.defaults.size_pt - 12.0).abs() < 0.01);
    assert!((doc.styles.para_defaults.space_after_pt - 6.0).abs() < 0.01);
    let heading = doc.styles.get(StyleId::HEADING_1).expect("heading style");
    assert_eq!(heading.char.weight, Some(700));
    assert_eq!(heading.char.size_pt, Some(16.0));
    assert_eq!(heading.outline_level, Some(1));
    assert_eq!(heading.base, Some(StyleId::BODY));
}

#[test]
fn saving_keeps_package_parts_libertum_does_not_model() {
    let parsed = Docx::parse(&fixture()).expect("parse");
    let saved = parsed.to_bytes().expect("save");
    let settings = zip_text(&saved, "word/settings.xml").expect("settings part");
    assert!(
        settings.contains("evenAndOddHeaders"),
        "settings survive a save: {settings}"
    );
    let custom = zip_text(&saved, "customXml/item1.xml").expect("custom part");
    assert!(custom.contains("preserve me"), "{custom}");

    // The saved document is itself readable and keeps the same content.
    let again = Docx::parse(&saved).expect("read the saved file");
    assert_eq!(
        again.document().paragraph_at(1).expect("p1").text(),
        "Bold plain text"
    );
    assert_eq!(again.document().sections[0].columns, 2);
}

#[test]
fn round_trip_keeps_text_runs_formatting_lists_and_sections() {
    let mut doc = Document::from_paragraphs(vec![
        Paragraph::with_runs(
            BlockId(1),
            "Hello world",
            vec![Run {
                range: 0..5,
                format: CharFormat::bold(),
            }],
        )
        .with_props(ParaProps {
            style: StyleId::HEADING_1,
            ..ParaProps::default()
        }),
        Paragraph::new(BlockId(2), "A\tB").with_props(ParaProps {
            format: ParaFormat {
                align: Some(Align::Centre),
                indent_left_pt: Some(18.0),
                space_after_pt: Some(6.0),
                line_spacing: Some(LineSpacing::Multiple(1.5)),
                keep_together: Some(true),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        }),
        Paragraph::new(BlockId(3), "Item one").with_props(ParaProps {
            list: Some(ListItem {
                style: ListStyle::Numbered,
                level: 1,
                group: 4,
            }),
            page_break_before: true,
            ..ParaProps::default()
        }),
    ]);
    doc.sections = vec![Section {
        columns: 2,
        column_gutter_pt: 24.0,
        first_page_number: Some(2),
        page: PageSetup {
            paper: PaperSize::Letter,
            orientation: Orientation::Landscape,
            margins: Margins::uniform(36.0),
        },
        ..Section::default()
    }];

    let bytes = Docx::new(doc).to_bytes().expect("write");
    let parsed = Docx::parse(&bytes).expect("read");
    let back = parsed.document();

    assert_eq!(back.len(), 3);
    let first = back.paragraph_at(0).expect("p0");
    assert_eq!(first.text(), "Hello world");
    assert_eq!(first.props.style, StyleId::HEADING_1);
    assert_eq!(first.runs().len(), 1);
    assert_eq!(first.runs()[0].range, 0..5);
    assert_eq!(first.runs()[0].format.weight, Some(700));

    let tabs = back.paragraph_at(1).expect("p1");
    assert_eq!(tabs.text(), "A\tB", "the tab survives");
    assert_eq!(tabs.props.format.align, Some(Align::Centre));
    assert_eq!(tabs.props.format.indent_left_pt, Some(18.0));
    assert_eq!(tabs.props.format.space_after_pt, Some(6.0));
    assert!(matches!(
        tabs.props.format.line_spacing,
        Some(LineSpacing::Multiple(m)) if (m - 1.5).abs() < 0.001
    ));
    assert_eq!(tabs.props.format.keep_together, Some(true));

    let item = back.paragraph_at(2).expect("p2");
    assert!(item.props.page_break_before);
    let list = item.props.list.expect("list");
    assert_eq!(list.style, ListStyle::Numbered);
    assert_eq!(list.level, 1);

    let section = &back.sections[0];
    assert_eq!(section.columns, 2);
    assert!((section.column_gutter_pt - 24.0).abs() < 0.01);
    assert_eq!(section.first_page_number, Some(2));
    assert_eq!(section.page.paper, PaperSize::Letter);
    assert_eq!(section.page.orientation, Orientation::Landscape);
    assert!((section.page.margins.left - 36.0).abs() < 0.01);

    // The package is valid enough to re-read, and the XML keeps the schema's child order
    // (Word repairs parts whose `w:pPr`/`w:rPr` children are out of sequence).
    let document_xml = zip_text(&bytes, "word/document.xml").expect("document part");
    assert!(
        !document_xml.contains("<w:rPr><w:rPr>"),
        "character properties are not nested twice"
    );
    assert!(
        document_xml.contains("<w:pStyle w:val=\"Normal\"/><w:pageBreakBefore/>"),
        "pageBreakBefore follows pStyle and precedes the rest: {document_xml}"
    );
    let styles_xml = zip_text(&bytes, "word/styles.xml").expect("styles part");
    assert!(!styles_xml.contains("<w:rPr><w:rPr>"));
    assert!(styles_xml.contains("<w:pPr><w:keepNext/>"));
}

#[test]
fn reads_a_table_fixture() {
    let parsed = Docx::parse(&table_fixture()).expect("parse");
    let doc = parsed.document();
    assert_eq!(doc.body().len(), 3, "paragraph, table, paragraph");
    let table = doc
        .body()
        .at(1)
        .and_then(|block| block.as_table())
        .expect("table");
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.columns(), 2);
    assert_eq!(table.header_rows, 1);
    assert!((table.widths_pt[0] - 150.0).abs() < 0.01);
    assert_eq!(
        table.cell(0, 0).expect("cell").paragraphs[0].text(),
        "Header A"
    );
    assert!(table.cell(0, 0).expect("cell").fill.is_some());
    assert_eq!(
        table.cell(1, 1).expect("cell").paragraphs[0].text(),
        "Cell B"
    );
    // Cell paragraphs join the flat paragraph order and can be edited.
    let cell_id = table.cell(0, 0).expect("cell").paragraphs[0].id;
    assert_eq!(doc.index_of(cell_id), Some(1));
    assert!(doc.location_of(cell_id).expect("location").table.is_some());
    assert_eq!(doc.paragraph_count(), 6, "before + 4 cells + after");
}

#[test]
fn a_table_round_trips() {
    let parsed = Docx::parse(&table_fixture()).expect("parse");
    let saved = parsed.to_bytes().expect("save");
    let xml = zip_text(&saved, "word/document.xml").expect("document.xml");
    assert!(xml.contains("<w:tbl>"), "{xml}");
    assert!(xml.contains("<w:tblHeader/>"), "{xml}");
    assert!(xml.contains("Cell B"), "{xml}");
    let again = Docx::parse(&saved).expect("re-read");
    let table = again
        .document()
        .body()
        .at(1)
        .and_then(|block| block.as_table())
        .expect("table");
    assert_eq!(
        table.cell(1, 1).expect("cell").paragraphs[0].text(),
        "Cell B"
    );
    assert_eq!(table.header_rows, 1);
    assert!(table.cell(0, 0).expect("cell").fill.is_some());
}

#[test]
fn a_footnote_round_trips() {
    let mut paragraph = Paragraph::new(BlockId(1), "Text with a note");
    paragraph.set_note(Some("The note text".into()));
    let bytes = Docx::new(Document::from_paragraphs(vec![paragraph]))
        .to_bytes()
        .expect("write");

    let document_xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    let reference = document_xml
        .find("<w:footnoteReference")
        .expect("footnote reference");
    let text = document_xml
        .find(">Text with a note<")
        .expect("paragraph text");
    assert!(reference < text, "offset 0 puts the reference first");
    assert!(
        document_xml.contains("<w:footnoteReference w:id=\"1\"/>"),
        "{document_xml}"
    );
    let footnotes = zip_text(&bytes, "word/footnotes.xml").expect("footnotes.xml");
    assert!(footnotes.contains("w:type=\"separator\""), "{footnotes}");
    assert!(
        footnotes.contains("w:type=\"continuationSeparator\""),
        "{footnotes}"
    );
    assert!(footnotes.contains("The note text"), "{footnotes}");

    let back = Docx::parse(&bytes).expect("read");
    let paragraph = back.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Text with a note");
    assert_eq!(paragraph.note(), Some("The note text"));
    assert_eq!(paragraph.notes()[0].offset, 0);
}

#[test]
fn two_footnotes_and_an_endnote_round_trip() {
    let mut paragraph = Paragraph::new(BlockId(1), "Alpha beta gamma");
    paragraph.set_notes(vec![
        Note {
            kind: NoteKind::Footnote,
            offset: 5,
            text: "First note".into(),
        },
        Note {
            kind: NoteKind::Footnote,
            offset: 10,
            text: "Second note".into(),
        },
        Note {
            kind: NoteKind::Endnote,
            offset: 3,
            text: "End note".into(),
        },
    ]);
    let bytes = Docx::new(Document::from_paragraphs(vec![paragraph]))
        .to_bytes()
        .expect("write");

    let document_xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert!(
        document_xml.contains("<w:footnoteReference w:id=\"1\"/>"),
        "first footnote: {document_xml}"
    );
    assert!(
        document_xml.contains("<w:footnoteReference w:id=\"2\"/>"),
        "second footnote: {document_xml}"
    );
    assert!(
        document_xml.contains("<w:endnoteReference w:id=\"1\"/>"),
        "endnote: {document_xml}"
    );
    // The references sit at their byte offsets: the endnote after "Alp", the first footnote
    // after "Alpha", and the second after "Alpha beta".
    let alp = document_xml.find(">Alp<").expect("text before the endnote");
    let endnote = document_xml
        .find("<w:endnoteReference")
        .expect("endnote reference");
    let ha = document_xml.find(">ha<").expect("text after the endnote");
    let first = document_xml
        .find("<w:footnoteReference w:id=\"1\"")
        .expect("first footnote");
    let beta = document_xml.find("> beta<").expect("text after the first");
    let second = document_xml
        .find("<w:footnoteReference w:id=\"2\"")
        .expect("second footnote");
    let gamma = document_xml
        .find("> gamma<")
        .expect("text after the second");
    assert!(
        alp < endnote
            && endnote < ha
            && ha < first
            && first < beta
            && beta < second
            && second < gamma,
        "reference order: {document_xml}"
    );

    let footnotes = zip_text(&bytes, "word/footnotes.xml").expect("footnotes.xml");
    assert!(footnotes.contains("w:type=\"separator\""), "{footnotes}");
    assert!(
        footnotes.contains("w:type=\"continuationSeparator\""),
        "{footnotes}"
    );
    assert!(footnotes.contains("First note"), "{footnotes}");
    assert!(footnotes.contains("Second note"), "{footnotes}");
    let endnotes = zip_text(&bytes, "word/endnotes.xml").expect("endnotes.xml");
    assert!(endnotes.contains("w:type=\"separator\""), "{endnotes}");
    assert!(endnotes.contains("End note"), "{endnotes}");

    let content_types = zip_text(&bytes, "[Content_Types].xml").expect("content types");
    assert!(
        content_types.contains("PartName=\"/word/footnotes.xml\""),
        "{content_types}"
    );
    assert!(
        content_types.contains("PartName=\"/word/endnotes.xml\""),
        "{content_types}"
    );
    let rels = zip_text(&bytes, "word/_rels/document.xml.rels").expect("relationships");
    assert!(
        rels.contains("relationships/footnotes\" Target=\"footnotes.xml\""),
        "{rels}"
    );
    assert!(
        rels.contains("relationships/endnotes\" Target=\"endnotes.xml\""),
        "{rels}"
    );

    let back = Docx::parse(&bytes).expect("read");
    let paragraph = back.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Alpha beta gamma");
    let notes = paragraph.notes();
    assert_eq!(notes.len(), 3, "all references survive");
    assert_eq!(
        notes
            .iter()
            .filter(|note| note.kind == NoteKind::Footnote)
            .count(),
        2
    );
    assert_eq!(
        notes
            .iter()
            .filter(|note| note.kind == NoteKind::Endnote)
            .count(),
        1
    );
    assert_eq!(notes[0].kind, NoteKind::Endnote);
    assert_eq!(notes[0].offset, 3);
    assert_eq!(notes[0].text, "End note");
    assert_eq!(notes[1].offset, 5);
    assert_eq!(notes[1].text, "First note");
    assert_eq!(notes[2].offset, 10);
    assert_eq!(notes[2].text, "Second note");
}

#[test]
fn reads_notes_from_the_relationship_target_and_flattens_rich_bodies() {
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">\
        <w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p></w:body></w:document>"#;
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">\
        <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="notes/my-footnotes.xml"/>\
        </Relationships>"#;
    let footnotes = r#"<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">\
        <w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>\
        <w:footnote w:type="continuationSeparator" w:id="0"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:footnote>\
        <w:footnote w:id="1"><w:p>\
        <w:r><w:rPr><w:rStyle w:val="FootnoteReference"/><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteRef/></w:r>\
        <w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve"> Bold start</w:t></w:r>\
        <w:r><w:t xml:space="preserve"> and rest</w:t></w:r>\
        </w:p></w:footnote></w:footnotes>"#;
    let bytes = zip_package(&[
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", rels),
        ("word/notes/my-footnotes.xml", footnotes),
    ]);

    let parsed = Docx::parse(&bytes).expect("parse");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Hello");
    let notes = paragraph.notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].kind, NoteKind::Footnote);
    assert_eq!(notes[0].offset, 5);
    assert_eq!(notes[0].text, "Bold start and rest");
    assert!(
        parsed
            .warnings()
            .iter()
            .any(|warning| warning.contains("flattened to plain text")),
        "{:?}",
        parsed.warnings()
    );

    // Saving keeps the part at the relationship target instead of inventing a second
    // footnotes part and a duplicate relationship.
    let saved = parsed.to_bytes().expect("save");
    assert!(
        zip_text(&saved, "word/footnotes.xml").is_none(),
        "no second footnotes part"
    );
    let saved_footnotes = zip_text(&saved, "word/notes/my-footnotes.xml").expect("custom part");
    assert!(
        saved_footnotes.contains("Bold start and rest"),
        "{saved_footnotes}"
    );
    let saved_rels = zip_text(&saved, "word/_rels/document.xml.rels").expect("relationships");
    assert_eq!(
        saved_rels.matches("relationships/footnotes").count(),
        1,
        "{saved_rels}"
    );
    let again = Docx::parse(&saved).expect("re-read the saved package");
    let notes = again.document().paragraph_at(0).expect("paragraph").notes();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].offset, 5);
    assert_eq!(notes[0].text, "Bold start and rest");
}

#[test]
fn a_comment_reference_is_still_refused() {
    let document = r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">\
        <w:body><w:p><w:r><w:t>Anchored</w:t></w:r><w:r><w:commentReference w:id="0"/></w:r></w:p></w:body></w:document>"#;
    let bytes = zip_package(&[("word/document.xml", document)]);
    let error = match Docx::parse(&bytes) {
        Ok(_) => panic!("a comment reference must be refused"),
        Err(error) => error,
    };
    assert!(
        matches!(error, DocxError::Unsupported { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_nested_table_is_refused() {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        writer
            .start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .expect("zip");
        writer
            .write_all(
                b"<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">                  <w:body><w:tbl><w:tr><w:tc><w:tbl/></w:tc></w:tr></w:tbl></w:body></w:document>",
            )
            .expect("write");
        writer.finish().expect("finish");
    }
    let error = match Docx::parse(&buffer.into_inner()) {
        Ok(_) => panic!("a nested table must be refused"),
        Err(error) => error,
    };
    assert!(
        matches!(error, DocxError::Unsupported { .. }),
        "got {error:?}"
    );
}

#[test]
fn an_empty_table_is_refused_rather_than_dropped() {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("word/document.xml", options)
            .expect("zip");
        writer
            .write_all(
                b"<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
                  <w:body><w:tbl/></w:body></w:document>",
            )
            .expect("write");
        writer.finish().expect("finish");
    }
    let error = match Docx::parse(&buffer.into_inner()) {
        Ok(_) => panic!("a table must be refused"),
        Err(error) => error,
    };
    assert!(
        matches!(error, DocxError::Unsupported { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_document_without_document_xml_is_an_error() {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        writer
            .start_file("word/styles.xml", zip::write::SimpleFileOptions::default())
            .expect("zip");
        writer.write_all(b"<w:styles/>").expect("write");
        writer.finish().expect("finish");
    }
    let error = match Docx::parse(&buffer.into_inner()) {
        Ok(_) => panic!("a package without document.xml must fail"),
        Err(error) => error,
    };
    assert!(matches!(error, DocxError::NoDocument), "got {error:?}");
}

#[test]
fn bookmarks_round_trip_at_their_offsets() {
    let mut paragraph = Paragraph::new(BlockId(1), "Hello world");
    paragraph.set_bookmarks(vec![
        Bookmark {
            name: "Start".into(),
            offset: 0,
        },
        Bookmark {
            name: "Middle".into(),
            offset: 6,
        },
        Bookmark {
            name: "End".into(),
            offset: 11,
        },
    ]);
    let doc = Document::from_paragraphs(vec![paragraph]);
    let bytes = Docx::new(doc).to_bytes().expect("write");

    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert!(xml.contains("w:bookmarkStart"));
    assert!(xml.contains("w:name=\"Middle\""));
    assert!(xml.contains("w:bookmarkEnd"));

    let back = Docx::parse(&bytes).expect("read");
    let anchors = back.document().paragraph_at(0).expect("p0").bookmarks();
    assert_eq!(
        anchors,
        [
            Bookmark {
                name: "Start".into(),
                offset: 0,
            },
            Bookmark {
                name: "Middle".into(),
                offset: 6,
            },
            Bookmark {
                name: "End".into(),
                offset: 11,
            },
        ]
    );
}

#[test]
fn reads_bookmarks_and_ignores_word_hidden_ones() {
    let bytes = zip_package(&[
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        ),
        (
            "word/document.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:bookmarkStart w:id="0" w:name="_GoBack"/><w:bookmarkStart w:id="1" w:name="Chapter"/><w:r><w:t>Text</w:t></w:r><w:bookmarkEnd w:id="1"/><w:bookmarkEnd w:id="0"/></w:p></w:body></w:document>"#,
        ),
    ]);
    let parsed = Docx::parse(&bytes).expect("parse");
    let paragraph = parsed.document().paragraph_at(0).expect("p0");
    assert_eq!(paragraph.text(), "Text");
    assert_eq!(
        paragraph.bookmarks(),
        [Bookmark {
            name: "Chapter".into(),
            offset: 0,
        }]
    );
}

// ------------------------------------------------------------------ Phase 6: images,
// cross-references and table styles

fn zip_names(bytes: &[u8]) -> Vec<String> {
    let archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok().map(|name| name.into_owned()))
        .collect();
    names.sort();
    names
}

fn zip_parts(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in parts {
            writer.start_file(*name, options).expect("zip entry");
            writer.write_all(bytes).expect("write");
        }
        writer.finish().expect("finish");
    }
    buffer.into_inner()
}

/// A JPEG header with a start-of-frame segment: enough for `Media::new`, not decodable.
fn fake_jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
    bytes
}

const W_DOC_OPEN: &str = r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>"#;

fn body_package(body: &str) -> Vec<u8> {
    let document = format!("{W_DOC_OPEN}{body}</w:body></w:document>");
    zip_parts(&[("word/document.xml", document.into_bytes())])
}

fn small_table(
    first_id: u64,
    style: Option<TableStyle>,
    rows: usize,
    header_rows: u8,
    own_fill: Option<(usize, [u8; 4])>,
) -> Table {
    let mut next = first_id + 1;
    let rows: Vec<TableRow> = (0..rows)
        .map(|row| {
            let cells = (0..2)
                .map(|column| {
                    let mut cell =
                        TableCell::new(Paragraph::new(BlockId(next), format!("r{row}c{column}")));
                    next += 1;
                    if let Some((fill_row, fill)) = own_fill {
                        if fill_row == row && column == 0 {
                            cell.fill = Some(fill);
                        }
                    }
                    cell
                })
                .collect();
            TableRow {
                cells,
                min_height_pt: 0.0,
                keep_together: true,
            }
        })
        .collect();
    Table {
        id: BlockId(first_id),
        widths_pt: vec![100.0, 100.0],
        rows,
        header_rows,
        borders: TableBorders::default(),
        style,
    }
}

fn banded() -> TableStyle {
    TableStyle::builtin("Banded Rows").expect("built-in").0
}

/// One document with a picture, a bookmark with a text and a page cross-reference, and a
/// banded table.
fn phase6_document() -> Document {
    let png = sample_png(16, 8).expect("png");
    let mut heading = Paragraph::new(BlockId(1), "Introduction");
    heading.set_bookmarks(vec![Bookmark {
        name: "Intro".into(),
        offset: 0,
    }]);
    let mut body = Paragraph::with_runs(
        BlockId(2),
        "See Introduction on page 3 for the picture here.",
        vec![Run {
            range: 4..16,
            format: CharFormat::bold(),
        }],
    );
    body.set_cross_refs(vec![
        CrossRef {
            range: 4..16,
            target: "Intro".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 25..26,
            target: "Intro".into(),
            kind: CrossRefKind::Page,
        },
    ]);
    body.set_images(vec![InlineImage {
        offset: 42,
        media: png,
        width_pt: 96.0,
        height_pt: 48.0,
        alt: "A checker pattern".into(),
        name: "checker.png".into(),
    }]);
    Document::from_blocks(vec![
        Block::Paragraph(heading),
        Block::Paragraph(body),
        Block::Table(small_table(10, Some(banded()), 5, 1, None)),
    ])
}

fn assert_same_anchors(expected: &Document, actual: &Document) {
    assert_eq!(expected.paragraph_count(), actual.paragraph_count());
    for (expected, actual) in expected.paragraphs().zip(actual.paragraphs()) {
        assert_eq!(expected.text(), actual.text());
        assert!(
            expected.anchors_eq(actual),
            "anchors differ in {:?}: {:?} vs {:?} / {:?} vs {:?} / {:?} vs {:?}",
            expected.text(),
            expected.images(),
            actual.images(),
            expected.cross_refs(),
            actual.cross_refs(),
            expected.bookmarks(),
            actual.bookmarks()
        );
    }
}

#[test]
fn images_round_trip_with_offset_size_alt_and_bytes() {
    let png = sample_png(16, 8).expect("png");
    let jpeg = std::sync::Arc::new(Media::new(fake_jpeg(40, 20)).expect("jpeg"));
    let mut first = Paragraph::new(BlockId(1), "Before after");
    first.set_images(vec![
        InlineImage {
            offset: 7,
            media: png.clone(),
            width_pt: 120.5,
            height_pt: 60.25,
            alt: "A \"checker\" <1> & 2".into(),
            name: "checker.png".into(),
        },
        InlineImage {
            offset: 7,
            media: jpeg.clone(),
            width_pt: 40.0,
            height_pt: 20.0,
            alt: String::new(),
            name: String::new(),
        },
        InlineImage {
            offset: 12,
            media: png.clone(),
            width_pt: 30.0,
            height_pt: 15.0,
            alt: "again".into(),
            name: "checker.png".into(),
        },
    ]);
    let mut empty = Paragraph::new(BlockId(2), "");
    empty.set_images(vec![InlineImage::fitted(0, jpeg.clone(), 500.0)]);
    let mut cell_paragraph = Paragraph::new(BlockId(12), "in a cell");
    cell_paragraph.set_images(vec![InlineImage {
        offset: 3,
        media: png.clone(),
        width_pt: 24.0,
        height_pt: 12.0,
        alt: "cell picture".into(),
        name: "cell.png".into(),
    }]);
    let mut table = small_table(10, None, 1, 0, None);
    table.rows[0].cells[0].paragraphs = vec![cell_paragraph];
    let doc = Document::from_blocks(vec![
        Block::Paragraph(first),
        Block::Paragraph(empty),
        Block::Table(table),
    ]);

    let bytes = Docx::new(doc.clone()).to_bytes().expect("write");
    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert!(xml.contains("<wp:inline"), "{xml}");
    assert!(
        xml.contains("<wp:extent cx=\"1530350\" cy=\"765175\"/>"),
        "{xml}"
    );
    assert!(
        xml.contains("descr=\"A &quot;checker&quot; &lt;1&gt; &amp; 2\""),
        "{xml}"
    );
    assert_eq!(xml.matches("<wp:docPr id=").count(), 5, "one per picture");
    let ids: Vec<&str> = xml
        .split("<wp:docPr id=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "drawing ids are unique: {ids:?}");

    let media: Vec<String> = zip_names(&bytes)
        .into_iter()
        .filter(|name| name.starts_with("word/media/"))
        .collect();
    assert_eq!(media.len(), 2, "one part per distinct image: {media:?}");
    assert!(media.iter().any(|name| name.ends_with(".png")));
    assert!(media.iter().any(|name| name.ends_with(".jpeg")));
    let types = zip_text(&bytes, "[Content_Types].xml").expect("content types");
    assert!(
        types.contains("Extension=\"png\" ContentType=\"image/png\""),
        "{types}"
    );
    assert!(
        types.contains("Extension=\"jpeg\" ContentType=\"image/jpeg\""),
        "{types}"
    );
    let rels = zip_text(&bytes, "word/_rels/document.xml.rels").expect("rels");
    assert_eq!(rels.matches("relationships/image").count(), 2, "{rels}");

    let back = Docx::parse(&bytes).expect("read");
    assert_same_anchors(&doc, back.document());
    let read = back.document().paragraph_at(0).expect("p0");
    let images = read.images();
    assert_eq!(images.len(), 3);
    assert_eq!(images[0].offset, 7);
    assert!((images[0].width_pt - 120.5).abs() < 0.001);
    assert!((images[0].height_pt - 60.25).abs() < 0.001);
    assert_eq!(images[0].alt, "A \"checker\" <1> & 2");
    assert_eq!(images[0].name, "checker.png");
    assert_eq!(images[0].media.bytes(), png.bytes());
    assert_eq!(images[1].media.bytes(), jpeg.bytes());
    assert_eq!(images[1].name, "", "an empty name stays empty");
    assert_eq!(images[2].offset, 12);
    let in_cell = back
        .document()
        .paragraphs()
        .find(|paragraph| paragraph.text() == "in a cell")
        .expect("cell paragraph");
    assert_eq!(in_cell.images().len(), 1);
    assert_eq!(in_cell.images()[0].alt, "cell picture");
}

#[test]
fn cross_references_round_trip_as_fields() {
    let mut heading = Paragraph::new(BlockId(1), "Introduction");
    heading.set_bookmarks(vec![
        Bookmark {
            name: "Intro".into(),
            offset: 0,
        },
        Bookmark {
            name: "Other".into(),
            offset: 4,
        },
    ]);
    let mut body = Paragraph::new(BlockId(2), "See Introduction on page 3.");
    body.set_cross_refs(vec![
        CrossRef {
            range: 4..16,
            target: "Intro".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 25..26,
            target: "Intro".into(),
            kind: CrossRefKind::Page,
        },
    ]);
    // Adjacent references, one at the very start and one at the very end.
    let mut adjacent = Paragraph::new(BlockId(3), "ABCDEF");
    adjacent.set_cross_refs(vec![
        CrossRef {
            range: 0..3,
            target: "Other".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 3..6,
            target: "Other".into(),
            kind: CrossRefKind::Page,
        },
    ]);
    let doc = Document::from_paragraphs(vec![heading, body, adjacent]);
    let bytes = Docx::new(doc.clone()).to_bytes().expect("write");

    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert!(xml.contains("> REF Intro \\h <"), "{xml}");
    assert!(xml.contains("> PAGEREF Intro \\h <"), "{xml}");
    assert_eq!(xml.matches("w:fldCharType=\"begin\"").count(), 4, "{xml}");
    assert_eq!(xml.matches("w:fldCharType=\"separate\"").count(), 4);
    assert_eq!(xml.matches("w:fldCharType=\"end\"").count(), 4);
    // A bookmark that a text reference points at covers its paragraph, so Word's own refresh
    // of the field gives the paragraph text.
    let start = xml.find("w:name=\"Intro\"").expect("bookmark");
    let text = xml.find(">Intr<").expect("text");
    let end = xml[start..].find("<w:bookmarkEnd").expect("end") + start;
    assert!(start < text && text < end, "{xml}");

    let back = Docx::parse(&bytes).expect("read");
    assert_same_anchors(&doc, back.document());
    let refs = back.document().paragraph_at(1).expect("p1").cross_refs();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0].range, 4..16);
    assert_eq!(refs[0].target, "Intro");
    assert_eq!(refs[0].kind, CrossRefKind::Text);
    assert_eq!(refs[1].range, 25..26);
    assert_eq!(refs[1].kind, CrossRefKind::Page);
    assert_eq!(
        back.document()
            .paragraph_at(0)
            .expect("p0")
            .bookmarks()
            .len(),
        2
    );
}

#[test]
fn a_cross_reference_to_a_name_with_spaces_is_quoted() {
    let mut paragraph = Paragraph::new(BlockId(1), "see this");
    paragraph.set_cross_refs(vec![CrossRef {
        range: 4..8,
        target: "My Mark".into(),
        kind: CrossRefKind::Text,
    }]);
    let doc = Document::from_paragraphs(vec![paragraph]);
    let bytes = Docx::new(doc.clone()).to_bytes().expect("write");
    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert!(xml.contains("> REF \"My Mark\" \\h <"), "{xml}");
    let back = Docx::parse(&bytes).expect("read");
    assert_same_anchors(&doc, back.document());
}

#[test]
fn reads_word_style_fields_and_keeps_the_hidden_bookmark_they_use() {
    let bytes = body_package(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="_Ref42"/><w:r><w:t>Target para</w:t></w:r><w:bookmarkEnd w:id="0"/></w:p>
<w:p><w:bookmarkStart w:id="1" w:name="_GoBack"/><w:r><w:t xml:space="preserve">Go to page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGEREF _Ref42 \h \* MERGEFORMAT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>7</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> and </w:t></w:r><w:fldSimple w:instr=" REF _Ref42 \h "><w:r><w:t>Target para</w:t></w:r></w:fldSimple><w:r><w:t>.</w:t></w:r></w:p>"#,
    );
    let parsed = Docx::parse(&bytes).expect("parse");
    let target = parsed.document().paragraph_at(0).expect("p0");
    assert_eq!(
        target.bookmarks(),
        [Bookmark {
            name: "_Ref42".into(),
            offset: 0
        }],
        "a hidden bookmark a field uses is kept"
    );
    let paragraph = parsed.document().paragraph_at(1).expect("p1");
    assert_eq!(paragraph.text(), "Go to page 7 and Target para.");
    assert!(paragraph.bookmarks().is_empty(), "_GoBack stays hidden");
    assert_eq!(
        paragraph.cross_refs(),
        [
            CrossRef {
                range: 11..12,
                target: "_Ref42".into(),
                kind: CrossRefKind::Page,
            },
            CrossRef {
                range: 17..28,
                target: "_Ref42".into(),
                kind: CrossRefKind::Text,
            },
        ]
    );
    // And it survives a save.
    let again = Docx::parse(&parsed.to_bytes().expect("save")).expect("re-read");
    assert_same_anchors(parsed.document(), again.document());
}

fn assert_unsupported(body: &str, what: &str) {
    match Docx::parse(&body_package(body)) {
        Ok(_) => panic!("{what} must be refused"),
        Err(DocxError::Unsupported { .. }) => {}
        Err(other) => panic!("{what}: wrong error {other:?}"),
    }
}

#[test]
fn other_fields_and_unsupported_reference_switches_are_still_refused() {
    assert_unsupported(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        "a PAGE field",
    );
    assert_unsupported(
        r#"<w:p><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>9</w:t></w:r></w:fldSimple></w:p>"#,
        "a NUMPAGES field",
    );
    assert_unsupported(
        r#"<w:p><w:fldSimple w:instr=" REF x \n "><w:r><w:t>1.2</w:t></w:r></w:fldSimple></w:p>"#,
        "a REF field with a paragraph-number switch",
    );
    assert_unsupported(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> REF x \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        "a cross-reference with no result text",
    );
    assert_unsupported(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> REF x \h </w:instrText></w:r></w:p>"#,
        "a field that never ends",
    );
    assert_unsupported(r#"<w:p><w:r><w:pict/></w:r></w:p>"#, "a VML picture");
}

#[test]
fn a_floating_picture_is_refused() {
    assert_unsupported(
        r#"<w:p><w:r><w:drawing><wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" behindDoc="0" locked="0" layoutInCell="1" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:extent cx="1270000" cy="635000"/><wp:docPr id="1" name="Picture 1"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r></w:p>"#,
        "a floating picture",
    );
}

#[test]
fn reads_a_word_style_inline_picture() {
    let png = sample_png(16, 8).expect("png");
    let document = format!(
        "{W_DOC_OPEN}<w:p><w:r><w:t>Look </w:t></w:r><w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"1270000\" cy=\"635000\"/><wp:docPr id=\"1\" name=\"Picture 1\" descr=\"Alt text\"/><a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:nvPicPr><pic:cNvPr id=\"0\" name=\"photo.png\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"rId7\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr/></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r><w:r><w:t>here</w:t></w:r></w:p></w:body></w:document>"
    );
    let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId7" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;
    let bytes = zip_parts(&[
        ("word/document.xml", document.into_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
        ("word/media/image1.png", png.bytes().to_vec()),
    ]);
    let parsed = Docx::parse(&bytes).expect("parse");
    let paragraph = parsed.document().paragraph_at(0).expect("p0");
    assert_eq!(paragraph.text(), "Look here");
    let images = paragraph.images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].offset, 5);
    assert!((images[0].width_pt - 100.0).abs() < 0.001);
    assert!((images[0].height_pt - 50.0).abs() < 0.001);
    assert_eq!(images[0].alt, "Alt text");
    assert_eq!(images[0].name, "photo.png");
    assert_eq!(images[0].media.bytes(), png.bytes());

    // Saving writes ours beside the original; the result reads back the same.
    let saved = parsed.to_bytes().expect("save");
    let again = Docx::parse(&saved).expect("re-read");
    assert_same_anchors(parsed.document(), again.document());
    let names = zip_names(&saved);
    assert!(names.iter().any(|name| name == "word/media/image1.png"));
}

#[test]
fn a_second_save_of_images_references_and_styles_is_stable() {
    let doc = phase6_document();
    let first = Docx::new(doc.clone()).to_bytes().expect("first save");
    let parsed = Docx::parse(&first).expect("parse");
    assert_same_anchors(&doc, parsed.document());
    let second = parsed.to_bytes().expect("second save");
    let parsed_again = Docx::parse(&second).expect("parse again");
    assert_same_anchors(&doc, parsed_again.document());
    let third = parsed_again.to_bytes().expect("third save");

    for part in [
        "word/document.xml",
        "word/styles.xml",
        "word/_rels/document.xml.rels",
        "[Content_Types].xml",
    ] {
        assert_eq!(
            zip_text(&second, part),
            zip_text(&third, part),
            "{part} changes on a repeated save"
        );
    }
    for part in ["word/document.xml", "word/styles.xml"] {
        assert_eq!(
            zip_text(&first, part),
            zip_text(&second, part),
            "{part} differs between the first and second save"
        );
    }
    assert_eq!(zip_names(&second), zip_names(&third));
    let rels = zip_text(&third, "word/_rels/document.xml.rels").expect("rels");
    assert_eq!(rels.matches("relationships/image").count(), 1, "{rels}");
}

#[test]
fn table_styles_round_trip_with_their_fills() {
    let style = banded();
    let header = style.header_fill;
    let band = style.band_fill;
    let doc = Document::from_blocks(vec![Block::Table(small_table(
        10,
        Some(style.clone()),
        5,
        1,
        Some((3, [255, 0, 0, 255])),
    ))]);
    let bytes = Docx::new(doc.clone()).to_bytes().expect("write");

    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert!(
        xml.contains("<w:tblStyle w:val=\"TblBandedRows\"/>"),
        "{xml}"
    );
    assert!(xml.contains("<w:tblLook w:val=\"0420\""), "{xml}");
    assert_eq!(
        xml.matches("<w:shd ").count(),
        1,
        "only the cell's own fill is written as cell shading"
    );
    let styles = zip_text(&bytes, "word/styles.xml").expect("styles.xml");
    assert!(
        styles.contains("<w:style w:type=\"table\" w:styleId=\"TblBandedRows\">"),
        "{styles}"
    );
    assert!(
        styles.contains("<w:name w:val=\"Banded Rows\"/>"),
        "{styles}"
    );
    assert!(
        styles.contains("<w:tblStylePr w:type=\"firstRow\">"),
        "{styles}"
    );
    assert!(
        styles.contains("<w:tblStylePr w:type=\"band2Horz\">"),
        "{styles}"
    );
    assert!(styles.contains("w:fill=\"D9D9D9\""), "{styles}");
    assert!(styles.contains("w:fill=\"F2F2F2\""), "{styles}");

    let back = Docx::parse(&bytes).expect("read");
    let table = back
        .document()
        .body()
        .at(0)
        .and_then(|block| block.as_table())
        .expect("table");
    let read = table.style.as_ref().expect("style");
    assert_eq!(read.name, "Banded Rows");
    assert_eq!(read.header_fill, header);
    assert_eq!(read.band_fill, band);
    assert_eq!(table.header_rows, 1);
    assert_eq!(table.style_fill(0), header);
    assert_eq!(table.style_fill(1), None);
    assert_eq!(table.style_fill(2), band);
    assert_eq!(table.style_fill(3), None);
    assert_eq!(table.style_fill(4), band);
    // The cell's own fill stays its own; the style's fills are not copied into the cells.
    assert_eq!(table.cell(3, 0).expect("cell").fill, Some([255, 0, 0, 255]));
    for (row, cell) in [(0, 0), (1, 0), (2, 1), (4, 1)] {
        assert_eq!(table.cell(row, cell).expect("cell").fill, None);
    }
    let original = doc
        .body()
        .at(0)
        .and_then(|block| block.as_table())
        .expect("table");
    assert_eq!(original.widths_pt, table.widths_pt);
    assert_eq!(original.borders, table.borders);
    assert_eq!(original.header_rows, table.header_rows);
    for (a, b) in original.rows.iter().zip(table.rows.iter()) {
        for (ca, cb) in a.cells.iter().zip(b.cells.iter()) {
            assert_eq!(ca.fill, cb.fill);
            assert_eq!(ca.paragraphs[0].text(), cb.paragraphs[0].text());
        }
    }
}

#[test]
fn table_look_flags_follow_the_style_and_header_rows() {
    let header_only = TableStyle::builtin("Header Row").expect("built-in").0;
    let plain = TableStyle::builtin("Plain").expect("built-in").0;
    let doc = Document::from_blocks(vec![
        Block::Table(small_table(10, Some(header_only), 3, 1, None)),
        Block::Table(small_table(20, Some(plain), 3, 0, None)),
        Block::Table(small_table(30, Some(banded()), 4, 0, None)),
        Block::Table(small_table(40, None, 2, 0, None)),
    ]);
    let bytes = Docx::new(doc.clone()).to_bytes().expect("write");
    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    let looks: Vec<&str> = xml
        .split("<w:tblLook w:val=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    // Header fill only; no fills; banded with no header row.
    assert_eq!(looks, ["0620", "0600", "0400"], "{xml}");
    assert_eq!(xml.matches("<w:tblStyle ").count(), 3);

    let back = Docx::parse(&bytes).expect("read");
    for (index, original) in doc.body().iter().enumerate() {
        let expected = original.as_table().expect("table");
        let actual = back
            .document()
            .body()
            .at(index)
            .and_then(|block| block.as_table())
            .expect("table");
        assert_eq!(expected.style, actual.style, "table {index}");
        assert_eq!(expected.header_rows, actual.header_rows, "table {index}");
    }
}

#[test]
fn two_styles_with_one_name_keep_their_own_fills() {
    let first = TableStyle {
        name: "Custom".into(),
        header_fill: Some([1, 2, 3, 255]),
        band_fill: None,
    };
    let second = TableStyle {
        name: "Custom".into(),
        header_fill: Some([9, 8, 7, 255]),
        band_fill: Some([200, 201, 202, 255]),
    };
    let doc = Document::from_blocks(vec![
        Block::Table(small_table(10, Some(first.clone()), 2, 1, None)),
        Block::Table(small_table(20, Some(second.clone()), 3, 1, None)),
        Block::Table(small_table(30, Some(first.clone()), 2, 1, None)),
    ]);
    let bytes = Docx::new(doc).to_bytes().expect("write");
    let styles = zip_text(&bytes, "word/styles.xml").expect("styles.xml");
    assert_eq!(
        styles.matches("w:type=\"table\" w:styleId=\"Tbl").count(),
        2
    );
    let back = Docx::parse(&bytes).expect("read");
    let style_of = |index: usize| {
        back.document()
            .body()
            .at(index)
            .and_then(|block| block.as_table())
            .and_then(|table| table.style.clone())
            .expect("style")
    };
    assert_eq!(style_of(0), first);
    assert_eq!(style_of(2), first);
    let renamed = style_of(1);
    assert_eq!(renamed.name, "Custom 2");
    assert_eq!(renamed.header_fill, second.header_fill);
    assert_eq!(renamed.band_fill, second.band_fill);
}

#[test]
fn reads_word_table_styles_through_based_on_and_ignores_other_conditions() {
    let styles = r#"<?xml version="1.0" encoding="UTF-8"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/></w:style>
<w:style w:type="table" w:styleId="Fancy"><w:name w:val="Fancy"/><w:basedOn w:val="TableNormal"/>
<w:tblStylePr w:type="firstRow"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="4472C4"/></w:tcPr></w:tblStylePr>
<w:tblStylePr w:type="band1Horz"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/></w:tcPr></w:tblStylePr>
<w:tblStylePr w:type="lastCol"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="000000"/></w:tcPr></w:tblStylePr></w:style>
<w:style w:type="table" w:styleId="Child"><w:name w:val="Child"/><w:basedOn w:val="Fancy"/>
<w:tblStylePr w:type="band2Horz"><w:tcPr><w:shd w:val="clear" w:color="auto" w:fill="EEEEEE"/></w:tcPr></w:tblStylePr></w:style>
</w:styles>"#;
    let table = |style: &str| {
        format!(
            "<w:tbl><w:tblPr><w:tblStyle w:val=\"{style}\"/></w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:p><w:r><w:t>h</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
        )
    };
    let document = format!(
        "{W_DOC_OPEN}{}{}{}<w:p/></w:body></w:document>",
        table("Fancy"),
        table("Child"),
        table("TableNormal")
    );
    let bytes = zip_parts(&[
        ("word/document.xml", document.into_bytes()),
        ("word/styles.xml", styles.as_bytes().to_vec()),
    ]);
    let parsed = Docx::parse(&bytes).expect("parse");
    let style_of = |index: usize| {
        parsed
            .document()
            .body()
            .at(index)
            .and_then(|block| block.as_table())
            .expect("table")
            .style
            .clone()
    };
    assert_eq!(
        style_of(0),
        Some(TableStyle {
            name: "Fancy".into(),
            header_fill: Some([0x44, 0x72, 0xC4, 255]),
            band_fill: None,
        }),
        "band1Horz and column conditions are ignored"
    );
    assert_eq!(
        style_of(1),
        Some(TableStyle {
            name: "Child".into(),
            header_fill: Some([0x44, 0x72, 0xC4, 255]),
            band_fill: Some([0xEE, 0xEE, 0xEE, 255]),
        }),
        "the header fill comes from the base style"
    );
    assert_eq!(style_of(2), None, "Word's default table style is no style");
}

#[test]
fn cant_split_maps_to_keep_together_both_ways() {
    let mut table = small_table(10, None, 3, 0, None);
    table.rows[0].keep_together = true;
    table.rows[1].keep_together = false;
    table.rows[2].keep_together = true;
    let doc = Document::from_blocks(vec![Block::Table(table)]);
    let bytes = Docx::new(doc).to_bytes().expect("write");
    let xml = zip_text(&bytes, "word/document.xml").expect("document.xml");
    assert_eq!(xml.matches("<w:cantSplit/>").count(), 2, "{xml}");
    let back = Docx::parse(&bytes).expect("read");
    let table = back
        .document()
        .body()
        .at(0)
        .and_then(|block| block.as_table())
        .expect("table");
    let flags: Vec<bool> = table.rows.iter().map(|row| row.keep_together).collect();
    assert_eq!(flags, [true, false, true]);

    // A row without `w:cantSplit` may break across pages, Word's default.
    let plain = body_package(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/>"#,
    );
    let parsed = Docx::parse(&plain).expect("parse");
    let table = parsed
        .document()
        .body()
        .at(0)
        .and_then(|block| block.as_table())
        .expect("table");
    assert!(!table.rows[0].keep_together);
}

fn watermarked(text: &str, diagonal: bool) -> Document {
    let mut document = Document::new();
    let id = document.alloc_id();
    document.append_blocks(vec![Block::Paragraph(Paragraph::new(id, "Body text"))]);
    for section in &mut document.sections {
        section.watermark = libertum_model::Watermark::new(text, diagonal);
    }
    document
}

#[test]
fn a_watermark_round_trips_through_a_new_docx() {
    for diagonal in [true, false] {
        let bytes = Docx::new(watermarked("DRAFT & <FINAL>", diagonal))
            .to_bytes()
            .expect("save");
        let back = Docx::parse(&bytes).expect("reopen");
        let watermark = back.document().sections[0]
            .watermark
            .clone()
            .expect("the watermark comes back");
        assert_eq!(watermark.text, "DRAFT & <FINAL>");
        assert_eq!(watermark.diagonal, diagonal);
        // The shape sits in a header the section refers to, with `r:id`.
        let document = zip_text(&bytes, "word/document.xml").expect("document.xml");
        assert!(document.contains("<w:headerReference w:type=\"default\" r:id=\""));
        let content_types = zip_text(&bytes, "[Content_Types].xml").expect("content types");
        assert!(content_types.contains("wordprocessingml.header+xml"));
    }
}

#[test]
fn changing_and_removing_a_watermark_leaves_nothing_behind() {
    let first = Docx::new(watermarked("DRAFT", true))
        .to_bytes()
        .expect("save");
    let mut docx = Docx::parse(&first).expect("reopen");
    let mut document = docx.document().clone();
    for section in &mut document.sections {
        section.watermark = libertum_model::Watermark::new("FINAL", false);
    }
    docx.set_document(document.clone());
    let second = docx.to_bytes().expect("save again");
    let header = zip_text(&second, "word/header1.xml").expect("the header");
    assert_eq!(header.matches("PowerPlusWaterMarkObject").count(), 1);
    assert!(header.contains("string=\"FINAL\"") && !header.contains("DRAFT"));

    let mut docx = Docx::parse(&second).expect("reopen");
    let mut document = docx.document().clone();
    for section in &mut document.sections {
        section.watermark = None;
    }
    docx.set_document(document);
    let third = docx.to_bytes().expect("save a third time");
    assert!(
        !zip_text(&third, "word/header1.xml")
            .expect("the header stays")
            .contains("PowerPlusWaterMarkObject")
    );
    assert!(
        Docx::parse(&third).expect("reopen").document().sections[0]
            .watermark
            .is_none()
    );
}

#[test]
fn a_watermark_joins_an_existing_header_and_keeps_its_text() {
    let ns = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";
    let document = format!(
        "<w:document {ns}><w:body><w:p><w:r><w:t>Hello</w:t></w:r></w:p>\
         <w:sectPr><w:headerReference w:type=\"default\" r:id=\"rId7\"/>\
         <w:pgSz w:w=\"11906\" w:h=\"16838\"/></w:sectPr></w:body></w:document>"
    );
    let rels = "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
        <Relationship Id=\"rId7\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/header\" Target=\"header9.xml\"/></Relationships>";
    let header = format!(
        "<w:hdr {ns}><w:p><w:pPr><w:jc w:val=\"right\"/></w:pPr><w:r><w:t>Running head</w:t></w:r></w:p></w:hdr>"
    );
    let bytes = zip_package(&[
        ("word/document.xml", &document),
        ("word/_rels/document.xml.rels", rels),
        ("word/header9.xml", &header),
    ]);
    let mut docx = Docx::parse(&bytes).expect("parse");
    assert!(docx.document().sections[0].watermark.is_none());
    let mut edited = docx.document().clone();
    edited.sections[0].watermark = libertum_model::Watermark::new("SECRET", true);
    docx.set_document(edited);
    let saved = docx.to_bytes().expect("save");
    let header = zip_text(&saved, "word/header9.xml").expect("header9");
    assert!(header.contains("Running head"), "{header}");
    assert!(header.contains("string=\"SECRET\""));
    assert!(
        header.find("<w:jc").unwrap_or(0) < header.find("<w:pict").unwrap_or(0),
        "the shape comes after the paragraph properties"
    );
    // No second header was invented, and the reference is intact.
    assert!(zip_text(&saved, "word/header1.xml").is_none());
    let document = zip_text(&saved, "word/document.xml").expect("document");
    assert!(document.contains("r:id=\"rId7\""), "{document}");
    let back = Docx::parse(&saved).expect("reopen");
    assert_eq!(
        back.document().sections[0]
            .watermark
            .as_ref()
            .map(|w| w.text.as_str()),
        Some("SECRET")
    );
}
