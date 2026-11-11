//! The "what will be lost" report.

use libertum_markdown::{Loss, LossKind, Markdown, loss_report};
use libertum_model::{
    Align, Block, BlockId, Bookmark, CharFormat, Document, HeadersFooters, Note, NoteKind,
    ParaFormat, ParaProps, Paragraph, Run, StyleId,
};

fn fixture_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "{}/../../tests/fixtures/md",
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn kinds(document: &Document) -> Vec<LossKind> {
    loss_report(document)
        .into_iter()
        .map(|loss| loss.kind)
        .collect()
}

fn paragraph(text: &str, runs: Vec<Run>, props: ParaProps) -> Block {
    Block::Paragraph(Paragraph::with_runs(BlockId(0), text, runs).with_props(props))
}

#[test]
fn every_markdown_fixture_reports_nothing() {
    let mut checked = 0;
    for entry in std::fs::read_dir(fixture_dir()).expect("fixture folder") {
        let path = entry.expect("entry").path();
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        // Opened from disk (images load) and parsed without a folder.
        let opened = Markdown::open(&path).expect("open");
        assert_eq!(
            loss_report(opened.document()),
            Vec::<Loss>::new(),
            "{}",
            path.display()
        );
        let parsed = Markdown::parse(&std::fs::read(&path).expect("read")).expect("parse");
        assert!(
            loss_report(parsed.document()).is_empty(),
            "{}",
            path.display()
        );
        checked += 1;
    }
    assert!(checked >= 9, "expected the fixtures, found {checked}");
}

#[test]
fn a_fresh_document_reports_nothing() {
    assert!(loss_report(&Document::new()).is_empty());
}

#[test]
fn colour_a_section_break_and_a_table_span_are_reported() {
    let mut scratch = Document::new();
    let mut table = scratch.new_table(2, 2, 400.0);
    table.rows[0].cells[0].col_span = 2;
    table.rows[0].cells.truncate(1);

    let red = CharFormat {
        colour: Some([200, 0, 0, 255]),
        ..CharFormat::default()
    };
    let document = Document::from_blocks(vec![
        paragraph(
            "Red words",
            vec![Run {
                range: 0..3,
                format: red,
            }],
            ParaProps::default(),
        ),
        paragraph(
            "New section",
            Vec::new(),
            ParaProps {
                section_start: true,
                ..ParaProps::default()
            },
        ),
        Block::Table(table),
    ]);

    assert_eq!(
        loss_report(&document),
        vec![
            Loss {
                kind: LossKind::TextColour,
                count: 1
            },
            Loss {
                kind: LossKind::SectionBreak,
                count: 1
            },
            Loss {
                kind: LossKind::TableSpans,
                count: 1
            },
        ]
    );
}

#[test]
fn character_and_paragraph_formatting_is_reported_by_kind() {
    let run = |range: std::ops::Range<usize>, format: CharFormat| Run { range, format };
    let document = Document::from_blocks(vec![paragraph(
        "abcdefghij",
        vec![
            run(
                0..1,
                CharFormat {
                    family: Some("Georgia".into()),
                    ..CharFormat::default()
                },
            ),
            run(
                1..2,
                CharFormat {
                    size_pt: Some(18.0),
                    ..CharFormat::default()
                },
            ),
            run(
                2..3,
                CharFormat {
                    underline: Some(true),
                    ..CharFormat::default()
                },
            ),
            run(
                3..4,
                CharFormat {
                    highlight: Some(Some([255, 255, 0, 255])),
                    ..CharFormat::default()
                },
            ),
            run(
                4..5,
                CharFormat {
                    vertical: Some(libertum_model::VerticalAlign::Superscript),
                    ..CharFormat::default()
                },
            ),
            run(
                5..6,
                CharFormat {
                    letter_spacing_pt: Some(1.5),
                    ..CharFormat::default()
                },
            ),
            // Expressed in Markdown, so not reported: bold, italic, strikethrough, a
            // monospace family, and a removed highlight.
            run(
                6..7,
                CharFormat {
                    weight: Some(700),
                    italic: Some(true),
                    strikethrough: Some(true),
                    highlight: Some(None),
                    ..CharFormat::default()
                },
            ),
            run(
                7..8,
                CharFormat {
                    family: Some("Courier New".into()),
                    ..CharFormat::default()
                },
            ),
        ],
        ParaProps {
            format: ParaFormat {
                align: Some(Align::Justify),
                indent_first_pt: Some(18.0),
                line_spacing: Some(libertum_model::LineSpacing::Multiple(1.5)),
                ..ParaFormat::default()
            },
            page_break_before: true,
            ..ParaProps::default()
        },
    )]);
    assert_eq!(
        kinds(&document),
        vec![
            LossKind::FontFamily,
            LossKind::FontSize,
            LossKind::Highlight,
            LossKind::Underline,
            LossKind::SuperSubscript,
            LossKind::LetterSpacing,
            LossKind::Alignment,
            LossKind::Indent,
            LossKind::LineSpacing,
            LossKind::PageBreak,
        ]
    );
}

#[test]
fn what_markdown_can_express_is_not_reported() {
    // Start alignment, a quote's indent, a heading, a bullet list and a centred table column.
    let markdown = Markdown::parse(
        b"> quoted\n\n> > deeper\n\n## Heading\n\n- item\n\n| a | b |\n|:-:|--:|\n| 1 | 2 |\n",
    )
    .expect("parse");
    assert!(loss_report(markdown.document()).is_empty());

    let mut document = markdown.document().clone();
    // A paragraph explicitly aligned to the start is the default, not a loss.
    let first = document.first_id().expect("block");
    document
        .apply(libertum_model::Operation::SetParaProps {
            block: first,
            props: ParaProps {
                style: StyleId::QUOTE,
                format: ParaFormat {
                    align: Some(Align::Start),
                    indent_left_pt: Some(36.0),
                    ..ParaFormat::default()
                },
                ..ParaProps::default()
            },
        })
        .expect("props");
    assert!(loss_report(&document).is_empty());
}

#[test]
fn notes_bookmarks_headers_and_columns_are_reported() {
    let mut p = Paragraph::new(BlockId(0), "Text with a note");
    p.set_notes(vec![Note {
        kind: NoteKind::Footnote,
        offset: 4,
        text: "A note".to_owned(),
    }]);
    p.set_bookmarks(vec![Bookmark {
        name: "mark".to_owned(),
        offset: 0,
    }]);
    let mut document = Document::from_blocks(vec![Block::Paragraph(p)]);
    document.sections[0].headers = HeadersFooters::with_default("Running head");
    document.sections[0].set_columns(2);
    document.sections[0].first_page_number = Some(0);
    assert_eq!(
        kinds(&document),
        vec![
            LossKind::Columns,
            LossKind::HeadersFooters,
            LossKind::PageNumbers,
            LossKind::Footnotes,
            LossKind::Bookmarks,
        ]
    );
}

#[test]
fn table_features_markdown_cannot_hold_are_reported() {
    let mut scratch = Document::new();
    let mut table = scratch.new_table(3, 2, 400.0);
    table.rows[1].cells[0].fill = Some([220, 220, 220, 255]);
    table.rows[1].min_height_pt = 30.0;
    table.rows[2].cells[1].merged_into_above = true;
    table.widths_pt = vec![100.0, 300.0];
    table.header_rows = 2;
    table.style = Some(libertum_model::TableStyle {
        name: "Banded Rows".to_owned(),
        header_fill: None,
        band_fill: None,
    });
    let extra = Paragraph::new(BlockId(0), "second paragraph");
    table.rows[0].cells[0].paragraphs.push(extra);
    let document = Document::from_blocks(vec![Block::Table(table)]);
    let found = kinds(&document);
    for expected in [
        LossKind::TableFills,
        LossKind::TableRowHeights,
        LossKind::TableSpans,
        LossKind::TableColumnWidths,
        LossKind::TableHeaderRows,
        LossKind::TableStyle,
        LossKind::TableMultiParagraph,
    ] {
        assert!(
            found.contains(&expected),
            "{expected:?} missing from {found:?}"
        );
    }
}

#[test]
fn the_text_is_british_english_and_counts_places() {
    let one = Loss {
        kind: LossKind::TextColour,
        count: 1,
    };
    let many = Loss {
        kind: LossKind::TextColour,
        count: 3,
    };
    assert_eq!(one.to_string(), "Text colour: 1 place");
    assert_eq!(many.to_string(), "Text colour: 3 places");
    assert_eq!(LossKind::TableFills.to_string(), "Table cell shading");
}
