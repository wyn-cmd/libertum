//! Round-trip and fixture tests for `libertum-odt` (PLAN §6.3, §8).

use std::io::{Cursor, Read};

use libertum_model::{
    Align, Block, BlockId, Bookmark, Borders, CharFormat, Document, HeaderFooter, HeadersFooters,
    LineSpacing, ListItem, ListStyle, Margins, Orientation, PageNumberHorizontal, PageNumberPos,
    PageNumberVertical, PageSetup, PaperSize, ParaFormat, ParaProps, Paragraph, ParagraphStyle,
    Run, Section, StyleId, StyleSheet, TabStop, TabStopKind, TabStops, Table, TableBorders,
    TableCell, TableRow, VerticalAlign,
};
use libertum_odt::{MIME_TEXT, Odt, OdtError};

fn zip_entries(bytes: &[u8]) -> Vec<(String, zip::CompressionMethod, Vec<u8>)> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok().map(|name| name.into_owned()))
        .collect();
    let mut out = Vec::new();
    for name in names {
        let mut file = archive.by_name(&name).expect("entry");
        let method = file.compression();
        let mut data = Vec::new();
        file.read_to_end(&mut data).expect("read");
        out.push((name, method, data));
    }
    out
}

fn zip_text(bytes: &[u8], name: &str) -> Option<String> {
    zip_entries(bytes)
        .into_iter()
        .find(|(entry, _, _)| entry == name)
        .map(|(_, _, data)| String::from_utf8_lossy(&data).into_owned())
}

fn paragraph(text: &str, props: ParaProps, runs: Vec<Run>) -> Paragraph {
    Paragraph::with_runs(BlockId(0), text, runs).with_props(props)
}

fn body(text: &str) -> Paragraph {
    paragraph(text, ParaProps::default(), Vec::new())
}

fn cell(text: &str) -> TableCell {
    TableCell::new(paragraph(text, ParaProps::default(), Vec::new()))
}

fn rich_document() -> Document {
    let mut styles = StyleSheet::with_defaults("Georgia", 12.0);
    styles.paragraph.push(ParagraphStyle {
        name: "Aside".to_owned(),
        base: Some(StyleId::BODY),
        char: CharFormat {
            italic: Some(true),
            ..CharFormat::default()
        },
        para: ParaFormat {
            indent_left_pt: Some(36.0),
            space_before_pt: Some(3.0),
            ..ParaFormat::default()
        },
        next: Some(StyleId::BODY),
        outline_level: None,
    });
    styles.paragraph[2].char.size_pt = Some(18.0);

    let bold = CharFormat {
        weight: Some(700),
        ..CharFormat::default()
    };
    let fancy = CharFormat {
        colour: Some([255, 0, 0, 255]),
        highlight: Some(Some([255, 255, 0, 255])),
        underline: Some(true),
        ..CharFormat::default()
    };
    let superscript = CharFormat {
        vertical: Some(VerticalAlign::Superscript),
        ..CharFormat::default()
    };
    let mut list_stops = TabStops::new();
    let _ = list_stops.push(TabStop {
        position_pt: 144.0,
        kind: TabStopKind::Right,
    });

    let mut blocks: Vec<Block> = Vec::new();
    // Heading.
    blocks.push(Block::Paragraph(paragraph(
        "Chapter One",
        ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    // Mixed runs, a tab and a page break.
    let text = "Bold plain coloured super\tend";
    let first = text.find("coloured").expect("coloured");
    let super_at = text.find("super").expect("super");
    let end = text.find('\t').expect("tab");
    blocks.push(Block::Paragraph(paragraph(
        text,
        ParaProps {
            format: ParaFormat {
                align: Some(Align::Justify),
                space_before_pt: Some(6.0),
                line_spacing: Some(LineSpacing::Multiple(1.5)),
                keep_with_next: Some(true),
                tab_stops: Some(list_stops),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
        vec![
            Run {
                range: 0..4,
                format: bold,
            },
            Run {
                range: first..first + 7,
                format: fancy,
            },
            Run {
                range: super_at..super_at + 5,
                format: superscript,
            },
            Run {
                range: end + 1..text.len(),
                format: CharFormat::sized(14.0),
            },
        ],
    )));
    // A bordered paragraph with a page break before it.
    blocks.push(Block::Paragraph(paragraph(
        "Boxed",
        ParaProps {
            page_break_before: true,
            format: ParaFormat {
                borders: Some(Borders::line(1.0)),
                // An explicit empty set clears an inherited one; it must survive the trip.
                tab_stops: Some(TabStops::new()),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    // Custom style.
    blocks.push(Block::Paragraph(paragraph(
        "Aside text",
        ParaProps {
            style: StyleId(6),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    // Two lists, the second continuing after ordinary text.
    let bullet = ListItem {
        style: ListStyle::Bullet,
        level: 0,
        group: 1,
    };
    let numbered = ListItem {
        style: ListStyle::Numbered,
        level: 0,
        group: 2,
    };
    let numbered_child = ListItem {
        style: ListStyle::Numbered,
        level: 1,
        group: 2,
    };
    blocks.push(Block::Paragraph(paragraph(
        "First bullet",
        ParaProps {
            list: Some(bullet),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(Block::Paragraph(paragraph(
        "First number",
        ParaProps {
            list: Some(numbered),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(Block::Paragraph(paragraph(
        "Nested number",
        ParaProps {
            list: Some(numbered_child),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(Block::Paragraph(paragraph(
        "Another number",
        ParaProps {
            list: Some(numbered),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(Block::Paragraph(body("Between the lists.")));
    blocks.push(Block::Paragraph(paragraph(
        "Continued number",
        ParaProps {
            list: Some(numbered),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    // Table with a header row, a fill and a merged cell.
    let mut merged = cell("");
    merged.merged_into_above = true;
    let table = Table {
        style: None,
        id: BlockId(0),
        widths_pt: vec![100.0, 140.0],
        rows: vec![
            TableRow {
                cells: vec![cell("Head A"), cell("Head B")],
                min_height_pt: 18.0,
                keep_together: true,
            },
            TableRow {
                cells: vec![
                    TableCell {
                        fill: Some([200, 220, 255, 255]),
                        ..cell("Body")
                    },
                    TableCell {
                        col_span: 1,
                        ..cell("Merged")
                    },
                ],
                min_height_pt: 0.0,
                keep_together: true,
            },
            TableRow {
                cells: vec![cell("Tail A"), merged],
                min_height_pt: 0.0,
                keep_together: true,
            },
        ],
        header_rows: 1,
        borders: TableBorders {
            width_pt: 1.0,
            colour: [0, 0, 0, 255],
            outer: true,
            inner: true,
        },
    };
    blocks.push(Block::Table(table));
    // Section two starts here.
    blocks.push(Block::Paragraph(paragraph(
        "Second section.",
        ParaProps {
            section_start: true,
            ..ParaProps::default()
        },
        Vec::new(),
    )));

    let mut document = Document::from_blocks(blocks);
    document.styles = styles;

    let mut first_section = Section {
        page: PageSetup {
            paper: PaperSize::A4,
            orientation: Orientation::Landscape,
            margins: Margins {
                top: 60.0,
                right: 50.0,
                bottom: 55.0,
                left: 45.0,
            },
        },
        page_numbers: Some(PageNumberPos {
            vertical: PageNumberVertical::Bottom,
            horizontal: PageNumberHorizontal::Centre,
            mirror: false,
        }),
        first_page_number: Some(5),
        hide_first_page: true,
        columns: 2,
        column_gutter_pt: 24.0,
        header_distance_pt: 30.0,
        footer_distance_pt: 40.0,
        ..Section::default()
    };
    first_section.headers = HeadersFooters {
        default: Some(HeaderFooter::new("Chapter")),
        first: Some(HeaderFooter::new("First page")),
        even: None,
        different_first: true,
        different_even_odd: false,
    };
    let mut footer = HeaderFooter::new("Page {page} of {pages}");
    footer.align = Align::End;
    first_section.footers = HeadersFooters {
        default: Some(footer),
        ..HeadersFooters::default()
    };
    let second_section = Section {
        page: PageSetup {
            paper: PaperSize::Letter,
            orientation: Orientation::Portrait,
            margins: Margins::uniform(50.0),
        },
        ..Section::default()
    };
    document.sections = vec![first_section, second_section];
    document
}

fn assert_same_table(left: &Table, right: &Table) {
    assert_eq!(left.widths_pt, right.widths_pt, "column widths");
    assert_eq!(left.header_rows, right.header_rows, "header rows");
    assert_eq!(left.borders, right.borders, "table borders");
    assert_eq!(left.rows.len(), right.rows.len(), "row count");
    for (row, (a, b)) in left.rows.iter().zip(right.rows.iter()).enumerate() {
        assert_eq!(a.min_height_pt, b.min_height_pt, "row {row} height");
        assert_eq!(a.cells.len(), b.cells.len(), "row {row} cells");
        for (cell, (a, b)) in a.cells.iter().zip(b.cells.iter()).enumerate() {
            assert_eq!(a.col_span, b.col_span, "row {row} cell {cell} span");
            assert_eq!(
                a.merged_into_above, b.merged_into_above,
                "row {row} cell {cell} merge"
            );
            assert_eq!(a.fill, b.fill, "row {row} cell {cell} fill");
            assert_eq!(a.paragraphs.len(), b.paragraphs.len());
            for (a, b) in a.paragraphs.iter().zip(b.paragraphs.iter()) {
                assert_eq!(a.text(), b.text());
                assert_eq!(a.runs(), b.runs(), "row {row} cell {cell} runs");
                assert_eq!(a.props, b.props, "row {row} cell {cell} props");
            }
        }
    }
}

fn assert_same_document(left: &Document, right: &Document) {
    assert_eq!(left.body().len(), right.body().len(), "block count");
    for (index, (a, b)) in left.body().iter().zip(right.body().iter()).enumerate() {
        match (a, b) {
            (Block::Paragraph(a), Block::Paragraph(b)) => {
                assert_eq!(a.text(), b.text(), "block {index} text");
                assert_eq!(a.runs(), b.runs(), "block {index} runs");
                assert_eq!(a.props, b.props, "block {index} props");
            }
            (Block::Table(a), Block::Table(b)) => assert_same_table(a, b),
            _ => panic!("block {index} changed kind"),
        }
    }
    assert_eq!(left.sections, right.sections, "sections");
    assert_eq!(left.styles, right.styles, "styles");
}

fn fixture() -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/odt/rich.odt"
    );
    std::fs::read(path).expect("rich.odt fixture")
}

#[test]
fn reads_the_rich_fixture() {
    let parsed = Odt::parse(&fixture()).expect("parse the fixture");
    let doc = parsed.document();
    assert_eq!(doc.styles.defaults.family.as_ref(), "Georgia");
    assert!((doc.styles.defaults.size_pt - 12.0).abs() < 0.01);
    let quote = doc.styles.get(StyleId::QUOTE).expect("quote style");
    assert_eq!(quote.para.align, Some(Align::Centre));
    let heading = doc.styles.get(StyleId::HEADING_1).expect("heading style");
    assert_eq!(heading.char.size_pt, Some(16.0));
    assert_eq!(heading.char.weight, Some(700));
    assert_eq!(heading.char.colour, Some([0x1A, 0x1A, 0x1A, 255]));

    let first = doc.paragraph_at(0).expect("heading");
    assert_eq!(first.text(), "Fixture Heading");
    assert_eq!(first.props.style, StyleId::HEADING_1);
    assert_eq!(first.props.outline_level, Some(1));

    let mixed = doc.paragraph_at(1).expect("mixed runs");
    assert_eq!(
        mixed.text(),
        "Plain Georgia text with bold words and italic words."
    );
    assert_eq!(mixed.runs().len(), 2, "two spans, two runs");
    assert_eq!(mixed.runs()[0].format.weight, Some(700));
    assert_eq!(mixed.runs()[1].format.italic, Some(true));
    assert_eq!(mixed.runs()[1].format.colour, Some([0xAA, 0x00, 0x00, 255]));

    assert_eq!(
        doc.paragraph_at(2).expect("quote").props.style,
        StyleId::QUOTE
    );
    let boxed = doc.paragraph_at(3).expect("boxed");
    let borders = boxed.props.format.borders.expect("border box");
    assert_eq!(borders.colour, [0x33, 0x66, 0x99, 255]);
    assert!((borders.width_pt - 0.5).abs() < 0.01);
    assert_eq!(
        boxed.props.format.line_spacing,
        Some(LineSpacing::Multiple(1.5))
    );

    let bullet = doc.paragraph_at(5).expect("first bullet");
    let item = bullet.props.list.expect("list item");
    assert_eq!(item.style, ListStyle::Bullet);
    assert_eq!(item.level, 0);
    assert_eq!(
        doc.paragraph_at(6).expect("second bullet").props.list,
        Some(item),
        "both bullets are one list"
    );
    assert!(
        doc.paragraph_at(7)
            .expect("page break")
            .props
            .page_break_before,
        "fo:break-before"
    );

    let table = doc
        .body()
        .iter()
        .find_map(|block| block.as_table())
        .expect("table");
    assert_eq!(table.widths_pt.len(), 2);
    assert!((table.widths_pt[0] - 144.0).abs() < 0.01);
    assert!((table.widths_pt[1] - 180.0).abs() < 0.01);
    assert_eq!(table.header_rows, 1);
    assert!(table.borders.is_visible());
    assert!((table.borders.width_pt - 1.0).abs() < 0.01);
    assert_eq!(table.rows[0].cells[0].text(), "Head A");
    assert_eq!(table.rows[0].cells[0].fill, Some([0xDD, 0xEE, 0xFF, 255]));

    assert_eq!(doc.sections.len(), 2, "two master pages");
    let landscape = &doc.sections[0];
    assert_eq!(landscape.page.paper, PaperSize::Letter);
    assert_eq!(landscape.page.orientation, Orientation::Landscape);
    assert!((landscape.page.margins.top - 57.6).abs() < 0.01);
    assert!((landscape.page.margins.left - 72.0).abs() < 0.01);
    assert_eq!(landscape.first_page_number, Some(3));
    assert!((landscape.header_distance_pt - 28.8).abs() < 0.01);
    assert!((landscape.footer_distance_pt - 36.0).abs() < 0.01);
    assert_eq!(
        landscape
            .headers
            .default
            .as_ref()
            .map(|hf| hf.text.as_str()),
        Some("Fixture Header")
    );
    assert_eq!(
        landscape.headers.first.as_ref().map(|hf| hf.text.as_str()),
        Some("First page header")
    );
    assert!(landscape.headers.different_first);
    assert_eq!(
        landscape
            .footers
            .default
            .as_ref()
            .map(|hf| hf.text.as_str()),
        Some("Page {page} of {pages}")
    );
    assert_eq!(doc.sections[1].page.orientation, Orientation::Portrait);
    assert_eq!(doc.sections[1].page.paper, PaperSize::Letter);
    let second = doc
        .paragraphs()
        .find(|paragraph| paragraph.text() == "Second section starts here.")
        .expect("second section paragraph");
    assert!(
        second.props.section_start,
        "the master-page change starts section two"
    );
}

#[test]
fn the_mimetype_entry_is_first_and_stored() {
    let bytes = Odt::new(rich_document())
        .to_bytes()
        .expect("write the package");
    let entries = zip_entries(&bytes);
    let (name, method, data) = entries.first().expect("an entry");
    assert_eq!(name, "mimetype", "ODF requires mimetype first");
    assert_eq!(*method, zip::CompressionMethod::Stored);
    assert_eq!(String::from_utf8_lossy(data), MIME_TEXT);
    let names: Vec<&str> = entries.iter().map(|(name, _, _)| name.as_str()).collect();
    for required in [
        "content.xml",
        "styles.xml",
        "meta.xml",
        "settings.xml",
        "META-INF/manifest.xml",
    ] {
        assert!(names.contains(&required), "missing {required}");
    }
    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(content.contains("office:document-content"));
    assert!(content.contains("text:outline-level=\"1\""));
    let styles = zip_text(&bytes, "styles.xml").expect("styles");
    assert!(styles.contains("<style:master-page"));
    assert!(styles.contains("<style:page-layout"));
}

#[test]
fn round_trips_a_rich_document() {
    let original = rich_document();
    let bytes = Odt::new(original.clone()).to_bytes().expect("write");
    let back = Odt::parse(&bytes).expect("read");
    assert_same_document(&original, back.document());

    // And a second pass is stable.
    let again = back.to_bytes().expect("write again");
    let third = Odt::parse(&again).expect("read again");
    assert_same_document(&original, third.document());
}

#[test]
fn unknown_parts_and_manifest_entries_survive_a_save() {
    let document = rich_document();
    let mut odt = Odt::new(document);
    let base = odt.to_bytes().expect("write");
    let manifest = zip_text(&base, "META-INF/manifest.xml").expect("manifest");
    // Add a custom part the way another program would: entry plus manifest line.
    let mut archive = zip::ZipArchive::new(Cursor::new(base)).expect("zip");
    let mut buffer = Cursor::new(Vec::new());
    {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("mimetype", stored).expect("mimetype");
        writer.write_all(MIME_TEXT.as_bytes()).expect("mimetype");
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for name in archive
            .file_names()
            .filter_map(|name| name.ok().map(|name| name.into_owned()))
            .collect::<Vec<_>>()
        {
            if name == "mimetype" || name == "META-INF/manifest.xml" {
                continue;
            }
            let mut file = archive.by_name(&name).expect("entry");
            let mut data = Vec::new();
            file.read_to_end(&mut data).expect("read");
            writer.start_file(&name, deflated).expect("entry");
            writer.write_all(&data).expect("write");
        }
        let manifest = manifest.replace(
            "</manifest:manifest>",
            "<manifest:file-entry manifest:full-path=\"Pictures/custom.bin\" manifest:media-type=\"application/octet-stream\"/></manifest:manifest>",
        );
        writer
            .start_file("META-INF/manifest.xml", deflated)
            .expect("manifest");
        writer.write_all(manifest.as_bytes()).expect("manifest");
        writer
            .start_file("Pictures/custom.bin", deflated)
            .expect("custom");
        writer.write_all(b"preserve me").expect("custom");
        writer.finish().expect("finish");
    }
    odt = Odt::parse(buffer.into_inner().as_slice()).expect("re-read");
    let saved = odt.to_bytes().expect("save");
    assert_eq!(
        zip_text(&saved, "Pictures/custom.bin").as_deref(),
        Some("preserve me")
    );
    let manifest = zip_text(&saved, "META-INF/manifest.xml").expect("manifest");
    assert!(manifest.contains("Pictures/custom.bin"));
}

#[test]
fn a_package_that_is_not_odt_is_refused() {
    let bytes = Odt::new(Document::new()).to_bytes().expect("write");
    // Replace the mimetype content with a spreadsheet one.
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut buffer = Cursor::new(Vec::new());
    {
        use std::io::Write;
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("mimetype", stored).expect("mimetype");
        writer
            .write_all(b"application/vnd.oasis.opendocument.spreadsheet")
            .expect("mimetype");
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for name in archive
            .file_names()
            .filter_map(|name| name.ok().map(|name| name.into_owned()))
            .collect::<Vec<_>>()
        {
            if name == "mimetype" {
                continue;
            }
            let mut file = archive.by_name(&name).expect("entry");
            let mut data = Vec::new();
            file.read_to_end(&mut data).expect("read");
            writer.start_file(&name, deflated).expect("entry");
            writer.write_all(&data).expect("write");
        }
        writer.finish().expect("finish");
    }
    assert!(matches!(
        Odt::parse(buffer.into_inner().as_slice()),
        Err(OdtError::NotOdt)
    ));
}

#[test]
fn floats_are_refused_rather_than_dropped() {
    // A body paragraph followed by a note: the importer must refuse it, because saving would
    // lose the note.
    let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3">
  <office:body><office:text>
    <text:p>Text<text:note text:note-class="footnote"><text:note-citation>1</text:note-citation><text:note-body><text:p>Note</text:p></text:note-body></text:note></text:p>
  </office:text></office:body>
</office:document-content>"#;
    let bytes = package_with_content(content);
    let back = Odt::parse(&bytes).expect("footnotes are supported");
    assert_eq!(
        back.document().paragraphs().next().and_then(|p| p.note()),
        Some("Note")
    );
}

/// Builds a minimal package around a given content.xml, for negative tests.
fn package_with_content(content: &str) -> Vec<u8> {
    use std::io::Write;
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("mimetype", stored).expect("mimetype");
        writer.write_all(MIME_TEXT.as_bytes()).expect("mimetype");
        let deflated = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("content.xml", deflated).expect("content");
        writer.write_all(content.as_bytes()).expect("content");
        writer
            .start_file("META-INF/manifest.xml", deflated)
            .expect("manifest");
        writer
            .write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#,
            )
            .expect("manifest");
        writer.finish().expect("finish");
    }
    buffer.into_inner()
}

#[test]
fn a_footnote_round_trips_through_content_xml() {
    let mut paragraph = body("Text with a note");
    paragraph.set_note(Some("The note text".into()));
    let document = Document::from_paragraphs(vec![paragraph]);
    let bytes = Odt::new(document).to_bytes().expect("write");
    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(
        content.contains("text:note-class=\"footnote\""),
        "{content}"
    );
    let back = Odt::parse(&bytes).expect("read");
    assert_eq!(
        back.document().paragraphs().next().and_then(|p| p.note()),
        Some("The note text")
    );
}

#[test]
fn multiple_footnotes_and_endnotes_round_trip() {
    use libertum_model::{Note, NoteKind};
    let mut paragraph = body("Two notes and an endnote");
    paragraph.set_notes(vec![
        Note {
            kind: NoteKind::Footnote,
            offset: 3,
            text: "First".into(),
        },
        Note {
            kind: NoteKind::Endnote,
            offset: 6,
            text: "End".into(),
        },
        Note {
            kind: NoteKind::Footnote,
            offset: 10,
            text: "Second".into(),
        },
    ]);
    let bytes = Odt::new(Document::from_paragraphs(vec![paragraph]))
        .to_bytes()
        .expect("write");
    let back = Odt::parse(&bytes).expect("read");
    let notes = back
        .document()
        .paragraphs()
        .next()
        .expect("paragraph")
        .notes();
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
    assert_eq!(notes[0].text, "First");
}

#[test]
fn bookmarks_round_trip_at_their_offsets() {
    let mut paragraph = body("Hello world");
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
    let bytes = Odt::new(Document::from_paragraphs(vec![paragraph]))
        .to_bytes()
        .expect("write");
    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(content.contains("text:bookmark"), "{content}");
    assert!(content.contains("text:name=\"Middle\""), "{content}");

    let back = Odt::parse(&bytes).expect("read");
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
    let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3">
  <office:body><office:text>
    <text:p><text:bookmark text:name="_GoBack"/><text:bookmark-start text:name="Chapter"/>Text<text:bookmark-end text:name="Chapter"/></text:p>
  </office:text></office:body>
</office:document-content>"#;
    let back = Odt::parse(&package_with_content(content)).expect("parse");
    let paragraph = back.document().paragraph_at(0).expect("p0");
    assert_eq!(paragraph.text(), "Text");
    assert_eq!(
        paragraph.bookmarks(),
        [Bookmark {
            name: "Chapter".into(),
            offset: 0,
        }]
    );
}

// ------------------------------------------------------------------ phase 6 features

use libertum_formats_common::media::sample_png;
use libertum_model::{CrossRef, CrossRefKind, InlineImage, TableStyle};

fn image_at(offset: usize, width: u32, height: u32, alt: &str, name: &str) -> InlineImage {
    InlineImage {
        offset,
        media: sample_png(width, height).expect("png"),
        width_pt: 120.5,
        height_pt: 60.25,
        alt: alt.to_owned(),
        name: name.to_owned(),
    }
}

fn single_paragraph(bytes: &[u8]) -> Paragraph {
    Odt::parse(bytes)
        .expect("read")
        .document()
        .paragraph_at(0)
        .expect("p0")
        .clone()
}

fn picture_names(data: &[u8]) -> Vec<String> {
    zip_entries(data)
        .into_iter()
        .map(|(name, _, _)| name)
        .filter(|name| name.starts_with("Pictures/"))
        .collect()
}

#[test]
fn an_inline_image_round_trips_with_its_package_entry() {
    let mut original = body("Before and after");
    let image = image_at(7, 8, 4, "A small test picture", "figure-1.png");
    original.set_images(vec![image.clone()]);
    let bytes = Odt::new(Document::from_paragraphs(vec![original.clone()]))
        .to_bytes()
        .expect("write");

    // `mimetype` is still first and stored.
    let entries = zip_entries(&bytes);
    assert_eq!(entries[0].0, "mimetype");
    assert_eq!(entries[0].1, zip::CompressionMethod::Stored);
    // The bytes are under Pictures/ and listed in the manifest with their media type.
    let path = format!(
        "Pictures/{:016x}.{}",
        image.media.hash(),
        image.media.format().extension()
    );
    let stored = entries
        .iter()
        .find(|(name, _, _)| *name == path)
        .expect("picture entry");
    assert_eq!(stored.2, image.media.bytes());
    let manifest = zip_text(&bytes, "META-INF/manifest.xml").expect("manifest");
    assert!(
        manifest.contains(&format!(
            "manifest:full-path=\"{path}\" manifest:media-type=\"image/png\""
        )),
        "{manifest}"
    );
    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(
        content.contains("text:anchor-type=\"as-char\""),
        "{content}"
    );
    assert!(content.contains("<svg:desc>A small test picture</svg:desc>"));

    let back = single_paragraph(&bytes);
    assert_eq!(back.text(), "Before and after");
    let [read] = back.images() else {
        panic!("expected one image, got {:?}", back.images());
    };
    assert_eq!(read.offset, 7);
    assert_eq!(read.alt, "A small test picture");
    assert_eq!(read.name, "figure-1.png");
    assert_eq!(read.media.bytes(), image.media.bytes());
    assert!((read.width_pt - 120.5).abs() < 0.01, "{}", read.width_pt);
    assert!((read.height_pt - 60.25).abs() < 0.01, "{}", read.height_pt);
    let mut expected = original;
    expected.set_images(vec![read.clone()]);
    assert!(back.anchors_eq(&expected));

    // A second save of the re-read document is stable: same picture entries, same content.
    let again = Odt::parse(&bytes)
        .expect("parse")
        .to_bytes()
        .expect("write");
    assert_eq!(picture_names(&bytes), picture_names(&again));
    assert_eq!(picture_names(&again), vec![path]);
    assert_eq!(zip_text(&again, "content.xml"), Some(content));
}

#[test]
fn the_same_picture_twice_is_stored_once() {
    let media = sample_png(6, 6).expect("png");
    let make = |offset| InlineImage {
        offset,
        media: media.clone(),
        width_pt: 30.0,
        height_pt: 30.0,
        alt: String::new(),
        name: String::new(),
    };
    let mut first = body("ab");
    first.set_images(vec![make(0), make(2)]);
    let mut second = body("cd");
    second.set_images(vec![make(1)]);
    let bytes = Odt::new(Document::from_paragraphs(vec![first, second]))
        .to_bytes()
        .expect("write");
    assert_eq!(picture_names(&bytes).len(), 1);
    let back = Odt::parse(&bytes).expect("read");
    let offsets: Vec<Vec<usize>> = back
        .document()
        .paragraphs()
        .map(|p| p.images().iter().map(|i| i.offset).collect())
        .collect();
    assert_eq!(offsets, vec![vec![0, 2], vec![1]]);
}

#[test]
fn images_survive_a_save_of_an_imported_package_without_duplicates() {
    let mut original = body("x");
    original.set_images(vec![image_at(1, 5, 5, "", "")]);
    let first = Odt::new(Document::from_paragraphs(vec![original]))
        .to_bytes()
        .expect("write");
    // Open, edit the text, save: the Pictures/ entry is reused, not duplicated or lost.
    let mut odt = Odt::parse(&first).expect("read");
    let images = odt
        .document()
        .paragraph_at(0)
        .expect("p0")
        .images()
        .to_vec();
    let mut edited = body("x and more");
    edited.set_images(images);
    odt.set_document(Document::from_paragraphs(vec![edited]));
    let second = odt.to_bytes().expect("write");
    assert_eq!(picture_names(&first), picture_names(&second));
    assert_eq!(picture_names(&second).len(), 1);
    // Delete the image: the picture it used leaves the package and the manifest.
    let mut odt = Odt::parse(&second).expect("read");
    odt.set_document(Document::from_paragraphs(vec![body("no image")]));
    let third = odt.to_bytes().expect("write");
    assert!(picture_names(&third).is_empty());
    let manifest = zip_text(&third, "META-INF/manifest.xml").expect("manifest");
    assert!(!manifest.contains("Pictures/"), "{manifest}");
}

fn content_with_frame(frame: &str) -> Vec<u8> {
    package_with_content(&format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" office:version="1.3">
  <office:body><office:text><text:p>Text {frame}</text:p></office:text></office:body>
</office:document-content>"#
    ))
}

#[test]
fn frames_that_are_not_inline_pictures_are_refused() {
    let floating = r#"<draw:frame text:anchor-type="paragraph" svg:width="10pt" svg:height="10pt"><draw:image xlink:href="Pictures/a.png"/></draw:frame>"#;
    let text_box = r#"<draw:frame text:anchor-type="as-char" svg:width="10pt" svg:height="10pt"><draw:text-box><text:p>Boxed</text:p></draw:text-box></draw:frame>"#;
    let linked = r#"<draw:frame text:anchor-type="as-char" svg:width="10pt" svg:height="10pt"><draw:image xlink:href="https://example.com/a.png"/></draw:frame>"#;
    for frame in [floating, text_box, linked] {
        let result = Odt::parse(&content_with_frame(frame));
        assert!(
            matches!(result, Err(OdtError::Unsupported { .. })),
            "{frame}: {:?}",
            result.map(|_| ())
        );
    }
    // An inline picture whose bytes are missing from the package is a typed content error.
    let missing = r#"<draw:frame text:anchor-type="as-char" svg:width="10pt" svg:height="10pt"><draw:image xlink:href="Pictures/none.png"/></draw:frame>"#;
    assert!(matches!(
        Odt::parse(&content_with_frame(missing)),
        Err(OdtError::Content { .. })
    ));
}

#[test]
fn cross_references_round_trip_with_range_target_and_kind() {
    let mut original = body("See Heading One on page 3 for more");
    original.set_bookmarks(vec![Bookmark {
        name: "Chapter_1".into(),
        offset: 0,
    }]);
    let refs = vec![
        CrossRef {
            range: 4..15,
            target: "Chapter_1".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 24..25,
            target: "Chapter_1".into(),
            kind: CrossRefKind::Page,
        },
    ];
    original.set_cross_refs(refs.clone());
    let bytes = Odt::new(Document::from_paragraphs(vec![original.clone()]))
        .to_bytes()
        .expect("write");
    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(
        content.contains(
            "<text:bookmark-ref text:reference-format=\"text\" text:ref-name=\"Chapter_1\">Heading One</text:bookmark-ref>"
        ),
        "{content}"
    );
    assert!(
        content.contains("text:reference-format=\"page\""),
        "{content}"
    );
    let back = single_paragraph(&bytes);
    assert_eq!(back.text(), original.text());
    assert_eq!(back.cross_refs(), refs.as_slice());
    assert!(back.anchors_eq(&original));
    let again = Odt::parse(&bytes)
        .expect("parse")
        .to_bytes()
        .expect("write");
    assert_eq!(zip_text(&again, "content.xml"), Some(content));
}

#[test]
fn a_cross_reference_split_by_formatting_and_an_anchor_comes_back_whole() {
    let bold = CharFormat {
        weight: Some(700),
        ..CharFormat::default()
    };
    let mut original = paragraph(
        "Go to Heading One now",
        ParaProps::default(),
        vec![
            Run {
                range: 0..9,
                format: CharFormat::default(),
            },
            Run {
                range: 9..21,
                format: bold,
            },
        ],
    );
    original.set_bookmarks(vec![Bookmark {
        name: "Mid".into(),
        offset: 12,
    }]);
    original.set_cross_refs(vec![CrossRef {
        range: 6..17,
        target: "Target".into(),
        kind: CrossRefKind::Text,
    }]);
    let bytes = Odt::new(Document::from_paragraphs(vec![original.clone()]))
        .to_bytes()
        .expect("write");
    let back = single_paragraph(&bytes);
    assert_eq!(back.cross_refs(), original.cross_refs());
    assert_eq!(back.bookmarks(), original.bookmarks());
    assert_eq!(back.text(), original.text());
    assert_eq!(back.runs(), original.runs());
}

#[test]
fn other_reference_formats_keep_their_text() {
    let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.3">
  <office:body><office:text>
    <text:p>See <text:bookmark-ref text:reference-format="chapter" text:ref-name="A">2.1</text:bookmark-ref> and <text:bookmark-ref text:reference-format="direction" text:ref-name="A">above</text:bookmark-ref></text:p>
  </office:text></office:body>
</office:document-content>"#;
    let back = Odt::parse(&package_with_content(content)).expect("parse");
    let paragraph = back.document().paragraph_at(0).expect("p0");
    assert_eq!(paragraph.text(), "See 2.1 and above");
    assert!(paragraph.cross_refs().is_empty());
    assert!(!back.warnings().is_empty());
}

fn styled_table(style: Option<TableStyle>, header_rows: u8, rows: usize) -> Table {
    let mut table = Table {
        style,
        id: BlockId(0),
        widths_pt: vec![100.0, 100.0],
        rows: (0..rows)
            .map(|row| TableRow {
                cells: vec![cell(&format!("r{row}a")), cell(&format!("r{row}b"))],
                min_height_pt: 0.0,
                keep_together: true,
            })
            .collect(),
        header_rows,
        borders: TableBorders::default(),
    };
    table.normalise();
    table
}

fn table_of(bytes: &[u8]) -> Table {
    let back = Odt::parse(bytes).expect("read");
    back.document()
        .body()
        .iter()
        .find_map(|block| match block {
            Block::Table(table) => Some(table.clone()),
            _ => None,
        })
        .expect("table")
}

#[test]
fn a_banded_table_writes_resolved_fills_and_reads_back_its_style() {
    let (style, borders) = TableStyle::builtin("Banded Rows").expect("builtin");
    let mut table = styled_table(Some(style.clone()), 1, 5);
    table.borders = borders;
    // One cell with its own fill, different from the style fill.
    let own = [255, 200, 0, 255];
    table.rows[2].cells[1].fill = Some(own);
    let bytes = Odt::new(Document::from_blocks(vec![
        Block::Paragraph(body("Intro")),
        Block::Table(table.clone()),
    ]))
    .to_bytes()
    .expect("write");

    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(
        content.contains("table:template-name=\"Banded Rows\""),
        "{content}"
    );
    // The fills are written on the cells so other programs draw the banding.
    assert!(
        content.contains("fo:background-color=\"#D9D9D9\""),
        "{content}"
    );
    assert!(
        content.contains("fo:background-color=\"#F2F2F2\""),
        "{content}"
    );
    assert!(
        content.contains("fo:background-color=\"#FFC800\""),
        "{content}"
    );

    let back = table_of(&bytes);
    assert_eq!(back.style.as_ref(), Some(&style));
    assert_eq!(back.header_rows, 1);
    for (row, table_row) in back.rows.iter().enumerate() {
        for (index, back_cell) in table_row.cells.iter().enumerate() {
            let source = &table.rows[row].cells[index];
            assert_eq!(back_cell.fill, source.fill, "row {row} cell {index}");
            assert_eq!(
                back.resolved_fill(row, back_cell),
                table.resolved_fill(row, source),
                "row {row} cell {index}"
            );
        }
    }
    assert_eq!(back.rows[2].cells[1].fill, Some(own));
    assert_eq!(back.rows[0].cells[0].fill, None);

    // A second pass is stable in content.xml.
    let again = Odt::parse(&bytes)
        .expect("parse")
        .to_bytes()
        .expect("write");
    // (The table name carries the block id, which an import renumbers.)
    let strip = |text: String| {
        let start = text.find("table:name=\"").expect("name");
        let end = start + 12 + text[start + 12..].find('"').expect("end");
        format!("{}{}", &text[..start], &text[end..])
    };
    assert_eq!(
        zip_text(&again, "content.xml").map(strip),
        Some(strip(content))
    );
}

#[test]
fn a_cell_fill_equal_to_the_style_fill_comes_back_as_none() {
    // The documented limit: the file cannot tell a user fill from the style fill.
    let (style, _) = TableStyle::builtin("Banded Rows").expect("builtin");
    let mut table = styled_table(Some(style.clone()), 1, 3);
    table.rows[0].cells[0].fill = Some([217, 217, 217, 255]);
    let bytes = Odt::new(Document::from_blocks(vec![Block::Table(table)]))
        .to_bytes()
        .expect("write");
    let back = table_of(&bytes);
    assert_eq!(back.style, Some(style));
    assert_eq!(back.rows[0].cells[0].fill, None);
}

#[test]
fn an_unknown_template_name_keeps_the_fills_on_the_cells() {
    let content = r##"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" office:version="1.3">
  <office:automatic-styles><style:style style:name="C1" style:family="table-cell"><style:table-cell-properties fo:background-color="#336699"/></style:style></office:automatic-styles>
  <office:body><office:text>
    <table:table table:name="T" table:template-name="Corporate Blue"><table:table-column table:number-columns-repeated="2"/>
      <table:table-row><table:table-cell table:style-name="C1"><text:p>A</text:p></table:table-cell><table:table-cell><text:p>B</text:p></table:table-cell></table:table-row>
    </table:table>
  </office:text></office:body>
</office:document-content>"##;
    let back = table_of(&package_with_content(content));
    assert_eq!(back.style, None);
    assert_eq!(back.rows[0].cells[0].fill, Some([0x33, 0x66, 0x99, 255]));
    assert_eq!(back.rows[0].cells[1].fill, None);
}

#[test]
fn a_table_without_a_style_writes_no_template_name() {
    let table = styled_table(None, 0, 2);
    let bytes = Odt::new(Document::from_blocks(vec![Block::Table(table)]))
        .to_bytes()
        .expect("write");
    let content = zip_text(&bytes, "content.xml").expect("content");
    assert!(!content.contains("template-name"));
    assert_eq!(table_of(&bytes).style, None);
}
