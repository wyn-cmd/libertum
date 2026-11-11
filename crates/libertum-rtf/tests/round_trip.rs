//! Round-trip and fixture tests for `libertum-rtf` (PLAN §6.5, §8).

use std::sync::Arc;

use libertum_formats_common::media::sample_png;
use libertum_model::{
    Align, Block, BlockId, Bookmark, CharFormat, CrossRef, CrossRefKind, Document, HeaderFooter,
    HeadersFooters, InlineImage, LineSpacing, ListItem, ListStyle, Margins, Media, Note, NoteKind,
    Orientation, PageSetup, PaperSize, ParaFormat, ParaProps, Paragraph, ParagraphStyle, Run,
    Section, StyleId, StyleSheet, TabStop, TabStops, Table, TableBorders, TableCell, TableRow,
    TableStyle, VerticalAlign,
};
use libertum_rtf::{Rtf, RtfError};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../tests/fixtures/rtf/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn paragraph(text: &str, props: ParaProps, runs: Vec<Run>) -> Paragraph {
    Paragraph::with_runs(BlockId(0), text, runs).with_props(props)
}

fn body(text: &str) -> Paragraph {
    paragraph(text, ParaProps::default(), Vec::new())
}

fn note(kind: NoteKind, offset: usize, text: &str) -> Note {
    Note {
        kind,
        offset,
        text: text.to_owned(),
    }
}

fn cell(text: &str) -> TableCell {
    TableCell::new(paragraph(text, ParaProps::default(), Vec::new()))
}

fn table_borders() -> TableBorders {
    TableBorders {
        width_pt: 1.0,
        colour: [0, 0, 0, 255],
        outer: true,
        inner: true,
    }
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
            indent_left_pt: Some(24.0),
            space_before_pt: Some(6.0),
            ..ParaFormat::default()
        },
        next: Some(StyleId::BODY),
        outline_level: None,
    });

    let bold = CharFormat::bold();
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
    let subscript = CharFormat {
        vertical: Some(VerticalAlign::Subscript),
        ..CharFormat::default()
    };
    let strike = CharFormat {
        strikethrough: Some(true),
        ..CharFormat::default()
    };
    let letter_spaced = CharFormat {
        letter_spacing_pt: Some(1.5),
        ..CharFormat::default()
    };
    let mut stops = TabStops::new();
    let _ = stops.push(TabStop::right(144.0));
    let _ = stops.push(TabStop::left(36.0));

    let mut blocks: Vec<Block> = Vec::new();
    blocks.push(Block::Paragraph(paragraph(
        "Chapter One",
        ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(Block::Paragraph(body(
        "Café • 😀 — “quoted” and a soft hyphen: \u{00AD}",
    )));
    let text = "Bold plain coloured super sub strike\tend";
    let first = text.find("Bold").expect("bold");
    let plain = text.find("plain").expect("plain");
    let coloured = text.find("coloured").expect("coloured");
    let super_at = text.find("super").expect("super");
    let sub_at = text.find("sub").expect("sub");
    let strike_at = text.find("strike").expect("strike");
    let end = text.find('\t').expect("tab");
    blocks.push(Block::Paragraph(paragraph(
        text,
        ParaProps {
            format: ParaFormat {
                align: Some(Align::Justify),
                indent_left_pt: Some(18.0),
                indent_right_pt: Some(9.0),
                indent_first_pt: Some(-9.0),
                space_before_pt: Some(6.0),
                space_after_pt: Some(3.0),
                line_spacing: Some(LineSpacing::Multiple(1.5)),
                keep_with_next: Some(true),
                keep_together: Some(true),
                tab_stops: Some(stops),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
        vec![
            Run {
                range: first..plain,
                format: bold,
            },
            Run {
                range: coloured..coloured + 8,
                format: fancy,
            },
            Run {
                range: super_at..super_at + 5,
                format: superscript,
            },
            Run {
                range: sub_at..sub_at + 3,
                format: subscript,
            },
            Run {
                range: strike_at..strike_at + 6,
                format: strike,
            },
            Run {
                range: end + 1..text.len(),
                format: letter_spaced,
            },
        ],
    )));
    blocks.push(Block::Paragraph(paragraph(
        "Boxed",
        ParaProps {
            page_break_before: true,
            format: ParaFormat {
                space_before_pt: Some(3.0),
                space_after_pt: Some(3.0),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
        Vec::new(),
    )));
    blocks.push(Block::Paragraph(paragraph(
        "Aside text",
        ParaProps {
            style: StyleId(6),
            ..ParaProps::default()
        },
        Vec::new(),
    )));

    let bullet = ListItem {
        style: ListStyle::Bullet,
        level: 0,
        group: 11,
    };
    let numbered = ListItem {
        style: ListStyle::Numbered,
        level: 0,
        group: 12,
    };
    let nested = ListItem {
        style: ListStyle::Numbered,
        level: 1,
        group: 12,
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
        "Second bullet",
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
            list: Some(nested),
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

    let table = Table {
        style: None,
        id: BlockId(0),
        widths_pt: vec![120.0, 180.0],
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
                        ..cell("Body A")
                    },
                    cell("Body B"),
                ],
                min_height_pt: 0.0,
                keep_together: true,
            },
        ],
        header_rows: 1,
        borders: table_borders(),
    };
    blocks.push(Block::Table(table));
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
                top: 54.0,
                right: 45.0,
                bottom: 45.0,
                left: 54.0,
            },
        },
        first_page_number: Some(3),
        columns: 2,
        column_gutter_pt: 24.0,
        header_distance_pt: 30.0,
        footer_distance_pt: 36.0,
        ..Section::default()
    };
    first_section.headers = HeadersFooters {
        default: Some(HeaderFooter::new("Chapter")),
        first: Some(HeaderFooter::new("First page")),
        even: Some(HeaderFooter::new("Even page")),
        different_first: true,
        different_even_odd: true,
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
            margins: Margins::uniform(54.0),
        },
        ..Section::default()
    };
    document.sections = vec![first_section, second_section];
    document
}

fn same_props(mut left: ParaProps, mut right: ParaProps) -> bool {
    // List groups are internal continuation ids; the RTF `\ls` index becomes a new group.
    if let Some(list) = &mut left.list {
        list.group = 0;
    }
    if let Some(list) = &mut right.list {
        list.group = 0;
    }
    left == right
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
                assert_eq!(a.notes(), b.notes(), "row {row} cell {cell} notes");
                assert!(
                    same_props(a.props, b.props),
                    "row {row} cell {cell} props: {:?} vs {:?}",
                    a.props,
                    b.props
                );
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
                assert_eq!(a.notes(), b.notes(), "block {index} notes");
                assert!(
                    same_props(a.props, b.props),
                    "block {index} props: {:?} vs {:?}",
                    a.props,
                    b.props
                );
            }
            (Block::Table(a), Block::Table(b)) => assert_same_table(a, b),
            _ => panic!("block {index} changed kind"),
        }
    }
    assert_eq!(left.sections, right.sections, "sections");
    assert_eq!(
        left.styles.defaults, right.styles.defaults,
        "document character defaults"
    );
    assert_eq!(
        left.styles.para_defaults, right.styles.para_defaults,
        "document paragraph defaults"
    );
    assert_eq!(
        left.styles.paragraph.len(),
        right.styles.paragraph.len(),
        "style count"
    );
    for (index, (a, b)) in left
        .styles
        .paragraph
        .iter()
        .zip(right.styles.paragraph.iter())
        .enumerate()
    {
        // RTF stores font sizes in half-points, so a generated built-in size such as
        // 28.8 pt comes back as 29 pt. Everything else in a style is kept exactly.
        let mut a = a.clone();
        let mut b = b.clone();
        if let Some(size) = a.char.size_pt {
            a.char.size_pt = Some((size * 2.0).round() / 2.0);
        }
        if let Some(size) = b.char.size_pt {
            b.char.size_pt = Some((size * 2.0).round() / 2.0);
        }
        assert_eq!(a, b, "style {index}");
    }
}

#[test]
fn round_trips_a_rich_document() {
    let original = rich_document();
    let bytes = Rtf::new(original.clone())
        .to_bytes()
        .expect("write the rich document");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("{\\rtf1"), "{text}");
    assert!(text.contains("\\fonttbl"), "a font table is written");
    assert!(text.contains("\\colortbl"), "a colour table is written");
    assert!(text.contains("\\stylesheet"), "a style sheet is written");
    assert!(text.contains("\\listtable"), "a list table is written");
    assert!(text.contains("\\header"), "headers are written");
    assert!(text.contains("\\field"), "page fields are written");

    let parsed = match Rtf::parse(&bytes) {
        Ok(parsed) => parsed,
        Err(error) => {
            let start = 860.min(bytes.len());
            let end = 1000.min(bytes.len());
            panic!(
                "read the written document: {error:?}\n{}",
                String::from_utf8_lossy(&bytes[start..end])
            );
        }
    };
    assert_same_document(&original, parsed.document());

    let again = parsed.to_bytes().expect("write again");
    let third = Rtf::parse(&again).expect("read again");
    assert_same_document(&original, third.document());
}

#[test]
fn a_picture_is_refused_rather_than_dropped() {
    let error = Rtf::parse(br"{\rtf1\ansi {\pict\wmetafile8 0100090000032e00} text\par}")
        .expect_err("a picture must be refused");
    assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
}

#[test]
fn an_unknown_destination_survives_a_save() {
    let source = fixture("unknown_destination.rtf");
    let parsed = Rtf::parse(&source).expect("parse the fixture");
    assert_eq!(
        parsed.document().paragraph_at(0).expect("paragraph").text(),
        "Preserve this text."
    );
    let saved = parsed.to_bytes().expect("save");
    let saved_text = String::from_utf8_lossy(&saved);
    assert!(
        saved_text.contains("{\\*\\unknownlib keep this raw group}"),
        "{saved_text}"
    );
    let again = Rtf::parse(&saved).expect("re-read");
    assert_eq!(
        again.document().paragraph_at(0).expect("paragraph").text(),
        "Preserve this text."
    );
}

#[test]
fn an_inline_bookmark_is_captured_and_rewritten() {
    let source = br"{\rtf1\ansi\deff0{\fonttbl{\f0\fcharset0 Georgia;}}\pard\plain Prefix {\*\bkmkstart b1}Post\par}";
    let parsed = Rtf::parse(source).expect("parse");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Prefix Post");
    assert_eq!(
        paragraph.bookmarks(),
        [Bookmark {
            name: "b1".into(),
            offset: 7,
        }]
    );
    let saved = String::from_utf8_lossy(&parsed.to_bytes().expect("save")).into_owned();
    assert!(saved.contains(r"{\*\bkmkstart b1}"), "{saved}");
    assert!(saved.contains(r"{\*\bkmkend b1}"), "{saved}");
    let again = Rtf::parse(saved.as_bytes()).expect("re-read");
    let paragraph = again.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Prefix Post");
    assert_eq!(
        paragraph.bookmarks(),
        [Bookmark {
            name: "b1".into(),
            offset: 7,
        }]
    );
}

#[test]
fn a_single_footnote_round_trips_at_its_offset() {
    let text = "A sentence with a note.";
    let at = text.find("with").expect("with");
    let mut paragraph = body(text);
    paragraph.set_notes(vec![note(
        NoteKind::Footnote,
        at,
        "Braces { }, a backslash \\ and caf\u{e9}.",
    )]);
    let original = Document::from_blocks(vec![Block::Paragraph(paragraph)]);

    let bytes = Rtf::new(original.clone())
        .to_bytes()
        .expect("write the footnote");
    let raw = String::from_utf8_lossy(&bytes);
    assert!(
        raw.contains("{\\footnote \\pard \\plain \\chftn Braces \\{ \\}, a backslash \\\\"),
        "{raw}"
    );

    let parsed = Rtf::parse(&bytes).expect("read the footnote");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), text);
    let expected = vec![note(
        NoteKind::Footnote,
        at,
        "Braces { }, a backslash \\ and caf\u{e9}.",
    )];
    assert_eq!(paragraph.notes(), expected.as_slice());
    assert_same_document(&original, parsed.document());
}

#[test]
fn a_paragraph_with_two_footnotes_and_an_endnote_round_trips() {
    let text = "Alpha beta gamma delta.";
    let first = text.find("beta").expect("beta");
    let second = text.find("gamma").expect("gamma") + "gamma".len();
    let third = text.find("delta").expect("delta");
    let mut paragraph = body(text);
    paragraph.set_notes(vec![
        note(NoteKind::Footnote, first, "The first footnote."),
        note(NoteKind::Endnote, second, "The only endnote."),
        note(NoteKind::Footnote, third, "The second footnote."),
    ]);
    let original = Document::from_blocks(vec![Block::Paragraph(paragraph)]);

    let bytes = Rtf::new(original.clone())
        .to_bytes()
        .expect("write the notes");
    let raw = String::from_utf8_lossy(&bytes);
    assert_eq!(
        raw.matches("{\\footnote \\pard \\plain \\chftn ").count(),
        2,
        "{raw}"
    );
    assert_eq!(
        raw.matches("{\\footnote \\ftnalt \\pard \\plain \\chftn ")
            .count(),
        1,
        "{raw}"
    );

    let parsed = Rtf::parse(&bytes).expect("read the notes");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), text);
    let expected = vec![
        note(NoteKind::Footnote, first, "The first footnote."),
        note(NoteKind::Endnote, second, "The only endnote."),
        note(NoteKind::Footnote, third, "The second footnote."),
    ];
    assert_eq!(paragraph.notes(), expected.as_slice());
    assert_same_document(&original, parsed.document());

    let third_pass = Rtf::parse(&parsed.to_bytes().expect("write again")).expect("read again");
    assert_same_document(&original, third_pass.document());
}

#[test]
fn a_note_offset_past_the_text_is_placed_at_the_paragraph_end() {
    let mut paragraph = body("Short");
    paragraph.set_notes(vec![note(NoteKind::Footnote, 99, "At the end.")]);
    let bytes = Rtf::new(Document::from_blocks(vec![Block::Paragraph(paragraph)]))
        .to_bytes()
        .expect("write");
    let parsed = Rtf::parse(&bytes).expect("read");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(
        paragraph.notes(),
        [note(NoteKind::Footnote, "Short".len(), "At the end.")].as_slice()
    );
}

#[test]
fn reads_note_groups_written_by_another_writer() {
    let source = br"{\rtf1\ansi\pard Text with a note{\footnote \pard\plain\chftn The note.} and an endnote{\endnote \pard\plain\chftn The endnote.}.\par}";
    let parsed = Rtf::parse(source).expect("parse");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Text with a note and an endnote.");
    let expected = vec![
        note(NoteKind::Footnote, "Text with a note".len(), "The note."),
        note(
            NoteKind::Endnote,
            "Text with a note and an endnote".len(),
            "The endnote.",
        ),
    ];
    assert_eq!(paragraph.notes(), expected.as_slice());
}

#[test]
fn a_word_style_endnote_is_read_as_an_endnote() {
    // Word writes endnotes as a `\footnote` destination carrying `\ftnalt`, not as an
    // `\endnote` destination.
    let source =
        br"{\rtf1\ansi\pard Text\chftn{\footnote\ftnalt\pard\plain\chftn The endnote body}\par}";
    let parsed = Rtf::parse(source).expect("parse");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Text");
    assert_eq!(
        paragraph.notes(),
        [note(NoteKind::Endnote, "Text".len(), "The endnote body")].as_slice()
    );
}

#[test]
fn a_note_paragraph_break_round_trips_as_a_newline() {
    let mut paragraph = body("Text");
    paragraph.set_notes(vec![note(
        NoteKind::Footnote,
        "Text".len(),
        "First line\nSecond line",
    )]);
    let bytes = Rtf::new(Document::from_blocks(vec![Block::Paragraph(paragraph)]))
        .to_bytes()
        .expect("write");
    let raw = String::from_utf8_lossy(&bytes);
    assert!(raw.contains("First line\\line Second line"), "{raw}");
    let parsed = Rtf::parse(&bytes).expect("read");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(
        paragraph.notes(),
        [note(
            NoteKind::Footnote,
            "Text".len(),
            "First line\nSecond line"
        )]
        .as_slice()
    );
}

#[test]
fn a_picture_inside_a_footnote_is_still_refused() {
    let error = Rtf::parse(br"{\rtf1\ansi Text{\footnote A note {\pict\pngblip 89504e47}}\par}")
        .expect_err("a picture must be refused");
    assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
}

#[test]
fn reads_the_hand_written_fixture() {
    let parsed = Rtf::parse(&fixture("handwritten.rtf")).expect("parse the fixture");
    let document = parsed.document();
    assert_eq!(document.styles.defaults.family.as_ref(), "Georgia");
    assert!((document.styles.defaults.size_pt - 12.0).abs() < 0.01);
    let heading = document
        .styles
        .get(StyleId::HEADING_1)
        .expect("heading style");
    assert_eq!(heading.char.size_pt, Some(16.0));
    assert_eq!(heading.char.weight, Some(700));
    assert_eq!(heading.outline_level, Some(1));

    let first = document.paragraph_at(0).expect("heading");
    assert_eq!(first.text(), "Fixture Heading");
    assert_eq!(first.props.style, StyleId::HEADING_1);
    let mixed = document.paragraph_at(1).expect("mixed runs");
    assert_eq!(mixed.text(), "Plain text with bold words and italic words.");
    assert_eq!(mixed.runs().len(), 2);
    assert_eq!(mixed.runs()[0].format.weight, Some(700));
    assert_eq!(mixed.runs()[1].format.italic, Some(true));

    let bullet = document.paragraph_at(2).expect("bullet");
    let item = bullet.props.list.expect("list item");
    assert_eq!(item.style, ListStyle::Bullet);
    assert_eq!(
        document.paragraph_at(3).expect("second bullet").props.list,
        Some(item)
    );
    let table = document
        .body()
        .iter()
        .find_map(|block| block.as_table())
        .expect("table");
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.header_rows, 1);
    assert_eq!(table.cell(0, 0).expect("cell").text(), "Head A");
    assert_eq!(table.cell(1, 1).expect("cell").text(), "Body B");

    assert_eq!(document.sections.len(), 2, "two sections");
    let first_section = &document.sections[0];
    assert_eq!(first_section.page.paper, PaperSize::A4);
    assert_eq!(first_section.page.orientation, Orientation::Landscape);
    assert!((first_section.page.margins.left - 54.0).abs() < 0.01);
    assert_eq!(first_section.columns, 2);
    assert_eq!(
        first_section
            .headers
            .default
            .as_ref()
            .map(|header| header.text.as_str()),
        Some("Fixture Header")
    );
    assert_eq!(
        first_section
            .headers
            .first
            .as_ref()
            .map(|header| header.text.as_str()),
        Some("First page header")
    );
    assert_eq!(
        first_section
            .footers
            .default
            .as_ref()
            .map(|footer| footer.text.as_str()),
        Some("Page {page} of {pages}")
    );
    assert_eq!(document.sections[1].page.orientation, Orientation::Portrait);
    assert!(
        document
            .paragraphs()
            .any(|paragraph| paragraph.text() == "Second section."
                && paragraph.props.section_start)
    );
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
    let bytes = Rtf::new(Document::from_paragraphs(vec![paragraph]))
        .to_bytes()
        .expect("write");
    let saved = String::from_utf8_lossy(&bytes).into_owned();
    assert!(saved.contains(r"{\*\bkmkstart Middle}"), "{saved}");
    let back = Rtf::parse(&bytes).expect("read");
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

// ---------------------------------------------------------------------- pictures

/// A JPEG that is only a header (SOF0, 20 x 10 pixels): enough for `Media::new`, which does
/// not decode.
fn jpeg_header_bytes() -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
    bytes.extend_from_slice(&[
        0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x0A, 0x00, 0x14, 0x01, 0x01, 0x11, 0x00,
    ]);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

fn png(width: u32, height: u32) -> Arc<Media> {
    sample_png(width, height).expect("sample png")
}

fn image(offset: usize, media: Arc<Media>, width_pt: f32, height_pt: f32) -> InlineImage {
    InlineImage {
        offset,
        media,
        width_pt,
        height_pt,
        alt: String::new(),
        name: String::new(),
    }
}

fn assert_images_match(expected: &[InlineImage], got: &[InlineImage]) {
    assert_eq!(expected.len(), got.len(), "image count");
    for (a, b) in expected.iter().zip(got) {
        assert_eq!(a.offset, b.offset, "image offset");
        assert_eq!(a.media.bytes(), b.media.bytes(), "image bytes");
        assert_eq!(a.media.format(), b.media.format(), "image format");
        // Sizes are written in twips, so they match to 1/40 pt.
        assert!((a.width_pt - b.width_pt).abs() <= 0.026, "width");
        assert!((a.height_pt - b.height_pt).abs() <= 0.026, "height");
        assert_eq!(a.alt, b.alt, "alt text");
        assert_eq!(a.name, b.name, "image name");
    }
}

#[test]
fn pictures_round_trip_with_offsets_sizes_and_bytes() {
    let jpeg = Arc::new(Media::new(jpeg_header_bytes()).expect("jpeg header"));
    let mut first = body("Before after é end");
    let mut photo = image(7, png(16, 8), 120.35, 60.1);
    photo.alt = "A checker {pattern}".to_owned();
    photo.name = "checker.png".to_owned();
    // Offset 0 and the paragraph end, and a JPEG.
    let images = vec![
        image(0, jpeg.clone(), 40.0, 20.0),
        photo,
        image(first.text().len(), png(4, 4), 30.0, 30.0),
    ];
    first.set_images(images.clone());
    // An image alone in an otherwise empty paragraph, and one inside a table cell.
    let mut lonely = body("");
    lonely.set_images(vec![image(0, png(8, 8), 72.0, 72.0)]);
    let mut cell_paragraph = body("cell");
    cell_paragraph.set_images(vec![image(2, png(8, 4), 50.0, 25.0)]);
    let table = Table {
        style: None,
        id: BlockId(0),
        widths_pt: vec![120.0],
        rows: vec![TableRow {
            cells: vec![TableCell::new(cell_paragraph.clone())],
            min_height_pt: 0.0,
            keep_together: true,
        }],
        header_rows: 0,
        borders: table_borders(),
    };
    let document = Document::from_blocks(vec![
        Block::Paragraph(first.clone()),
        Block::Paragraph(lonely.clone()),
        Block::Table(table),
        Block::Paragraph(body("after the table")),
    ]);

    let bytes = Rtf::new(document).to_bytes().expect("write");
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(r"\pngblip"), "PNG blip written");
    assert!(text.contains(r"\jpegblip"), "JPEG blip written");
    assert!(text.contains(r"\picwgoal2407"), "twip goal width: {text}");
    assert!(!text.contains("shppict"), "the bare pict form is used");

    let back = Rtf::parse(&bytes).expect("read");
    let blocks: Vec<&Block> = back.document().body().iter().collect();
    let p0 = blocks[0].as_paragraph().expect("p0");
    assert_eq!(p0.text(), "Before after é end");
    assert_images_match(&images, p0.images());
    let p1 = blocks[1].as_paragraph().expect("p1");
    assert_images_match(lonely.images(), p1.images());
    let table = blocks[2].as_table().expect("table");
    let cell = table.cell(0, 0).expect("cell");
    assert_images_match(cell_paragraph.images(), cell.paragraphs[0].images());
    assert!(blocks[3].as_paragraph().expect("p3").images().is_empty());

    // A second save is byte for byte the same and reads back to the same images.
    let again = back.to_bytes().expect("write again");
    assert_eq!(bytes, again, "a second save is stable");
    let third = Rtf::parse(&again).expect("read again");
    let p0 = third
        .document()
        .body()
        .iter()
        .next()
        .and_then(Block::as_paragraph)
        .expect("p0");
    assert_images_match(&images, p0.images());
}

#[test]
fn a_metafile_picture_is_refused() {
    for source in [
        &br"{\rtf1\ansi Text {\pict\wmetafile8\picw10\pich10 0100090000032e00} end\par}"[..],
        &br"{\rtf1\ansi Text {\pict\emfblip\picw10\pich10 0100090000032e00} end\par}"[..],
        &br"{\rtf1\ansi Text {\*\shppict{\pict\emfblip\picw10\pich10 0100}} end\par}"[..],
    ] {
        let error = Rtf::parse(source).expect_err("a metafile must be refused");
        assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
    }
    // A shape and an object keep being refused too.
    let error = Rtf::parse(br"{\rtf1\ansi {\shp{\*\shpinst}} x\par}").expect_err("shape");
    assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
    let error =
        Rtf::parse(br"{\rtf1\ansi {\object\objemb{\*\objdata 00}} x\par}").expect_err("object");
    assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
}

#[test]
fn damaged_or_cropped_pictures_are_errors_not_drops() {
    // Not hex.
    let error = Rtf::parse(br"{\rtf1\ansi {\pict\pngblip zz} x\par}").expect_err("not hex");
    assert!(matches!(error, RtfError::Parse { .. }), "{error:?}");
    // Odd number of digits.
    let error = Rtf::parse(br"{\rtf1\ansi {\pict\pngblip 8950f} x\par}").expect_err("odd");
    assert!(matches!(error, RtfError::Parse { .. }), "{error:?}");
    // Valid hex that is not an image.
    let error = Rtf::parse(br"{\rtf1\ansi {\pict\pngblip 89504e47} x\par}").expect_err("short");
    assert!(matches!(error, RtfError::Parse { .. }), "{error:?}");
    // A crop has no place in the model.
    let png_hex = hex(png(4, 4).bytes());
    let source = format!(r"{{\rtf1\ansi {{\pict\pngblip\piccropl100 {png_hex}}} x\par}}");
    let error = Rtf::parse(source.as_bytes()).expect_err("crop");
    assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn a_binary_picture_payload_is_read() {
    let media = png(8, 8);
    let mut source =
        br"{\rtf1\ansi Look {\pict\pngblip\picw8\pich8\picwgoal400\pichgoal400".to_vec();
    source.extend_from_slice(format!(r"\bin{} ", media.bytes().len()).as_bytes());
    source.extend_from_slice(media.bytes());
    source.extend_from_slice(br"} here\par}");
    let parsed = Rtf::parse(&source).expect("read binary picture");
    let paragraph = parsed.document().paragraph_at(0).expect("paragraph");
    assert_eq!(paragraph.text(), "Look  here");
    let images = paragraph.images();
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].offset, 5);
    assert_eq!(images[0].media.bytes(), media.bytes());
    assert!((images[0].width_pt - 20.0).abs() < 0.01);
}

#[test]
fn picture_size_uses_the_scale_and_falls_back_to_pixels() {
    let media = png(40, 20);
    let hex = hex(media.bytes());
    let scaled = format!(
        r"{{\rtf1\ansi {{\pict\pngblip\picscalex50\picscaley200\picwgoal2000\pichgoal400 {hex}}}\par}}"
    );
    let parsed = Rtf::parse(scaled.as_bytes()).expect("read");
    let image = &parsed.document().paragraph_at(0).expect("p").images()[0];
    assert!((image.width_pt - 50.0).abs() < 0.01, "{}", image.width_pt);
    assert!((image.height_pt - 40.0).abs() < 0.01, "{}", image.height_pt);
    let unsized_ = format!(r"{{\rtf1\ansi {{\pict\pngblip {hex}}}\par}}");
    let parsed = Rtf::parse(unsized_.as_bytes()).expect("read");
    let image = &parsed.document().paragraph_at(0).expect("p").images()[0];
    assert!(
        (image.width_pt - 30.0).abs() < 0.01,
        "40 px at 96 dpi is 30 pt"
    );
    assert!((image.height_pt - 15.0).abs() < 0.01);
}

// ---------------------------------------------------------------------- cross-references

#[test]
fn cross_references_round_trip() {
    let mut target = body("Figure 1: a checker");
    target.set_bookmarks(vec![Bookmark {
        name: "fig_cap".into(),
        offset: 0,
    }]);
    let text = "As shown in Figure 1 on page 12, see also é.";
    let figure = text.find("Figure 1").expect("figure");
    let page = text.find("12").expect("page");
    let mut referring = paragraph(
        text,
        ParaProps::default(),
        vec![Run {
            range: figure..figure + 6,
            format: CharFormat::bold(),
        }],
    );
    let refs = vec![
        CrossRef {
            range: figure..figure + 8,
            target: "fig_cap".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: page..page + 2,
            target: "fig_cap".into(),
            kind: CrossRefKind::Page,
        },
    ];
    referring.set_cross_refs(refs.clone());
    // A reference at the very start of a paragraph and one at its very end; the target
    // name with a space is written in quotes.
    let mut edges = body("Intro tail");
    edges.set_cross_refs(vec![
        CrossRef {
            range: 0..5,
            target: "My Mark".into(),
            kind: CrossRefKind::Text,
        },
        CrossRef {
            range: 6..10,
            target: "fig_cap".into(),
            kind: CrossRefKind::Page,
        },
    ]);
    let document =
        Document::from_paragraphs(vec![target.clone(), referring.clone(), edges.clone()]);

    let bytes = Rtf::new(document).to_bytes().expect("write");
    let text_out = String::from_utf8_lossy(&bytes).into_owned();
    assert!(text_out.contains(r"REF fig_cap \\h"), "{text_out}");
    assert!(text_out.contains(r"PAGEREF fig_cap \\h"), "{text_out}");
    assert!(text_out.contains(r#"REF "My Mark" \\h"#), "{text_out}");

    let back = Rtf::parse(&bytes).expect("read");
    let doc = back.document();
    let p1 = doc.paragraph_at(1).expect("p1");
    assert_eq!(p1.text(), text);
    assert_eq!(p1.cross_refs(), refs.as_slice());
    assert_eq!(p1.runs(), referring.runs(), "formatting inside the result");
    let p2 = doc.paragraph_at(2).expect("p2");
    assert_eq!(p2.text(), "Intro tail");
    assert_eq!(p2.cross_refs(), edges.cross_refs());
    assert!(doc.paragraph_at(1).expect("p1").anchors_eq(&referring));
    assert!(doc.paragraph_at(2).expect("p2").anchors_eq(&edges));
    assert_eq!(
        doc.paragraph_at(0).expect("p0").bookmarks(),
        target.bookmarks()
    );

    let again = back.to_bytes().expect("write again");
    assert_eq!(bytes, again, "a second save is stable");
    let third = Rtf::parse(&again).expect("read again");
    assert!(
        third
            .document()
            .paragraph_at(1)
            .expect("p1")
            .anchors_eq(&referring)
    );
}

#[test]
fn reference_instructions_are_read_with_or_without_switches() {
    for instruction in [
        r"REF mark",
        r"REF mark \\h",
        r"REF mark \\h \\* MERGEFORMAT",
        r#"REF "mark" \\h"#,
        r"ref mark",
    ] {
        let source = format!(
            r"{{\rtf1\ansi See {{\field{{\*\fldinst {instruction} }}{{\fldrslt cached text}}}} now\par}}"
        );
        let parsed = Rtf::parse(source.as_bytes()).unwrap_or_else(|e| panic!("{instruction}: {e}"));
        let paragraph = parsed.document().paragraph_at(0).expect("p");
        assert_eq!(paragraph.text(), "See cached text now", "{instruction}");
        assert_eq!(
            paragraph.cross_refs(),
            [CrossRef {
                range: 4..15,
                target: "mark".into(),
                kind: CrossRefKind::Text,
            }],
            "{instruction}"
        );
    }
    let source = br"{\rtf1\ansi p {\field{\*\fldinst PAGEREF mark}{\fldrslt 3}}\par}";
    let parsed = Rtf::parse(source).expect("pageref");
    let paragraph = parsed.document().paragraph_at(0).expect("p");
    assert_eq!(paragraph.cross_refs()[0].kind, CrossRefKind::Page);
    assert_eq!(paragraph.cross_refs()[0].range, 2..3);
    // Other fields keep being refused.
    let error =
        Rtf::parse(br"{\rtf1\ansi {\field{\*\fldinst TOC \\o}{\fldrslt x}}\par}").expect_err("TOC");
    assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
}

// ---------------------------------------------------------------------- table styles

fn styled_table(style_name: &str) -> Table {
    let (style, borders) = TableStyle::builtin(style_name).expect("built-in style");
    let mut rows = Vec::new();
    for row in 0..5 {
        rows.push(TableRow {
            cells: vec![cell(&format!("r{row}a")), cell(&format!("r{row}b"))],
            min_height_pt: 0.0,
            keep_together: true,
        });
    }
    // Cells with a fill of their own: one on a banded row, one on a plain row, one on the
    // header row.
    rows[2].cells[0].fill = Some([255, 0, 0, 255]);
    rows[3].cells[1].fill = Some([0, 128, 0, 255]);
    rows[0].cells[1].fill = Some([0, 0, 255, 255]);
    Table {
        style: Some(style),
        id: BlockId(0),
        widths_pt: vec![100.0, 100.0],
        rows,
        header_rows: 1,
        borders,
    }
}

#[test]
fn trkeep_maps_to_keep_together_both_ways() {
    let mut table = styled_table("Grid");
    for (index, row) in table.rows.iter_mut().enumerate() {
        row.keep_together = index % 2 == 0;
    }
    let document = Document::from_blocks(vec![Block::Table(table)]);
    let bytes = Rtf::new(document).to_bytes().expect("write");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    assert_eq!(text.matches("\\trkeep").count(), 3, "{text}");
    let back = Rtf::parse(&bytes).expect("parse");
    let table = back
        .document()
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("table");
    let flags: Vec<bool> = table.rows.iter().map(|row| row.keep_together).collect();
    assert_eq!(flags, [true, false, true, false, true]);
    // A second save is stable.
    let again = Rtf::new(back.document().clone())
        .to_bytes()
        .expect("rewrite");
    assert_eq!(again, bytes);
    // Absent means the row may break across pages.
    let source = br"{\rtf1\ansi\trowd\cellx1000 a\cell\row}";
    let parsed = Rtf::parse(source).expect("plain row");
    let table = parsed
        .document()
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("table");
    assert!(!table.rows[0].keep_together);
}

fn fills(table: &Table) -> Vec<Vec<Option<[u8; 4]>>> {
    table
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.fill).collect())
        .collect()
}

#[test]
fn a_table_style_round_trips_through_resolved_fills() {
    for name in [
        "Banded Rows",
        "Header Row",
        "Banded, No Grid",
        "Grid",
        "Plain",
    ] {
        let original = styled_table(name);
        let document = Document::from_blocks(vec![
            Block::Paragraph(body("before")),
            Block::Table(original.clone()),
            Block::Paragraph(body("after")),
        ]);
        let bytes = Rtf::new(document).to_bytes().expect("write");
        let text = String::from_utf8_lossy(&bytes).into_owned();
        assert_eq!(
            text.matches("libertumtblstyle").count(),
            1,
            "{name}: {text}"
        );
        if name == "Banded Rows" {
            // The resolved fill is in every banded and header cell for other readers.
            assert!(text.contains(r"\red217\green217\blue217"), "header colour");
            assert!(text.contains(r"\red242\green242\blue242"), "band colour");
            assert_eq!(text.matches(r"\clcbpat").count(), 7, "{text}");
        }

        let back = Rtf::parse(&bytes).expect("read");
        let table = back
            .document()
            .body()
            .iter()
            .find_map(Block::as_table)
            .expect("table");
        assert_eq!(table.style, original.style, "{name}: style");
        assert_eq!(table.header_rows, original.header_rows);
        assert_eq!(fills(table), fills(&original), "{name}: cell fills");
        assert_eq!(table.borders, original.borders, "{name}: borders");

        // The destination is not preserved a second time as a raw snippet.
        let again = back.to_bytes().expect("write again");
        let again_text = String::from_utf8_lossy(&again).into_owned();
        assert_eq!(
            again_text.matches("libertumtblstyle").count(),
            1,
            "{name}: no duplicate after a second save"
        );
        assert_eq!(bytes, again, "{name}: a second save is stable");
        let third = Rtf::parse(&again).expect("read again");
        let table = third
            .document()
            .body()
            .iter()
            .find_map(Block::as_table)
            .expect("table");
        assert_eq!(table.style, original.style);
        assert_eq!(fills(table), fills(&original));
    }
}

#[test]
fn a_table_without_a_style_marker_keeps_its_fills_and_gets_no_style() {
    let source = br"{\rtf1\ansi{\colortbl;\red217\green217\blue217;}\trowd\trhdr\clcbpat1\cellx2000 \pard\intbl H\cell\row\trowd\cellx2000 \pard\intbl B\cell\row\pard\par}";
    let parsed = Rtf::parse(source).expect("read");
    let table = parsed
        .document()
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("table");
    assert_eq!(table.style, None);
    assert_eq!(table.rows[0].cells[0].fill, Some([217, 217, 217, 255]));
}

#[test]
fn an_unknown_table_style_name_leaves_fills_on_the_cells() {
    let source = br"{\rtf1\ansi{\colortbl;\red1\green2\blue3;}{\*\libertumtblstyle Custom Thing}\trowd\clcbpat1\cellx2000 \pard\intbl H\cell\row\pard\par}";
    let parsed = Rtf::parse(source).expect("read");
    let table = parsed
        .document()
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("table");
    assert_eq!(table.style, None);
    assert_eq!(table.rows[0].cells[0].fill, Some([1, 2, 3, 255]));
    assert!(
        parsed.warnings().iter().any(|w| w.contains("Custom Thing")),
        "{:?}",
        parsed.warnings()
    );
    let saved = String::from_utf8_lossy(&parsed.to_bytes().expect("save")).into_owned();
    assert!(!saved.contains("libertumtblstyle"), "{saved}");
}

// ---------------------------------------------------------------------- fixture

#[test]
fn reads_the_picture_and_reference_fixture() {
    let parsed = Rtf::parse(&fixture("picture_and_ref.rtf")).expect("parse the fixture");
    let document = parsed.document();
    let first = document.paragraph_at(0).expect("first");
    assert_eq!(first.text(), "Look at the figure: ");
    // The shppict picture is read and the nonshppict metafile fallback is skipped (a
    // metafile would otherwise be refused).
    assert_eq!(first.images().len(), 1);
    let image = &first.images()[0];
    assert_eq!(image.offset, 20);
    assert_eq!(image.media.width_px(), 4);
    assert_eq!(image.media.height_px(), 2);
    assert!(
        (image.width_pt - 50.0).abs() < 0.01,
        "goal 2000 twips at 50 %"
    );
    assert!((image.height_pt - 25.0).abs() < 0.01);
    assert_eq!(image.alt, "A blue and grey checker");

    let caption = document.paragraph_at(1).expect("caption");
    assert_eq!(caption.text(), "Figure 1: a checker");
    assert_eq!(caption.bookmarks().len(), 1);
    let sentence = document.paragraph_at(2).expect("sentence");
    assert_eq!(sentence.text(), "As shown in Figure 1 on page 1.");
    assert_eq!(
        sentence.cross_refs(),
        [
            CrossRef {
                range: 12..20,
                target: "fig_cap".into(),
                kind: CrossRefKind::Text,
            },
            CrossRef {
                range: 29..30,
                target: "fig_cap".into(),
                kind: CrossRefKind::Page,
            },
        ]
    );
    assert_eq!(sentence.runs()[0].range, 12..20);
    assert_eq!(sentence.runs()[0].format.weight, Some(700));

    let saved = parsed.to_bytes().expect("save");
    let again = Rtf::parse(&saved).expect("re-read");
    for index in 0..3 {
        assert!(
            again
                .document()
                .paragraph_at(index)
                .expect("paragraph")
                .anchors_eq(document.paragraph_at(index).expect("paragraph")),
            "paragraph {index} anchors"
        );
    }
}
