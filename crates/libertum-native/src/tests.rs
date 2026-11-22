//! Round-trip, container-shape and refusal tests for the native format.

use std::sync::Arc;

use libertum_model::{
    Block, BlockId, Bookmark, CharFormat, CrossRef, CrossRefKind, DateTimeField, DateTimeFormat,
    DateTimeSource, Document, HeaderFooter, HeadersFooters, InlineImage, Media, Note, NoteKind,
    ParaProps, Paragraph, Run, Section, StyleId, Table, TableCell, TableRow, TableStyle,
    test_png_header,
};
use proptest::prelude::*;

use super::*;

fn image(width_px: u32, height_px: u32) -> Arc<Media> {
    Arc::new(Media::new(test_png_header(width_px, height_px)).expect("a PNG header"))
}

fn cell(text: &str) -> TableCell {
    TableCell::new(Paragraph::new(BlockId(0), text))
}

/// A document with one of everything the model carries.
fn rich_document() -> Document {
    let mut heading = Paragraph::new(BlockId(0), "Chapter One").with_props(ParaProps {
        style: StyleId::HEADING_1,
        outline_level: Some(1),
        ..ParaProps::default()
    });
    heading.set_bookmarks(vec![Bookmark {
        name: "chapter_1".to_owned(),
        offset: 0,
    }]);

    let text = "A bold start, a footnote, a picture and a reference to Chapter One.";
    let mut body = Paragraph::new(BlockId(0), text);
    body.set_runs(vec![Run {
        range: 0..4,
        format: CharFormat {
            weight: Some(700),
            ..CharFormat::default()
        },
    }]);
    body.set_notes(vec![Note {
        kind: NoteKind::Footnote,
        offset: 8,
        text: "A note at the bottom.".to_owned(),
    }]);
    let mut shown = InlineImage::fitted(12, image(64, 32), 300.0);
    shown.alt = "A checker".to_owned();
    shown.name = "checker.png".to_owned();
    body.set_images(vec![shown]);
    let at = text.find("Chapter One").expect("the reference text");
    body.set_cross_refs(vec![CrossRef {
        range: at..at + "Chapter One".len(),
        target: "chapter_1".to_owned(),
        kind: CrossRefKind::Text,
    }]);

    let (style, borders) = TableStyle::builtin("Banded Rows").expect("a built-in style");
    let mut rows: Vec<TableRow> = (0..3)
        .map(|row| TableRow {
            cells: vec![cell(&format!("Row {row}")), cell(&format!("{row}0"))],
            min_height_pt: 0.0,
            keep_together: false,
        })
        .collect();
    rows[1].cells[0].col_span = 2;
    let table = Table {
        id: BlockId(0),
        widths_pt: vec![140.0, 160.0],
        rows,
        header_rows: 1,
        borders,
        style: Some(style),
    };

    let mut document = Document::from_blocks(vec![
        Block::Paragraph(heading),
        Block::Paragraph(body),
        Block::Table(table),
    ]);
    document.sections = vec![Section {
        headers: HeadersFooters {
            default: Some(HeaderFooter::new("Halfway — {page}")),
            ..HeadersFooters::default()
        },
        ..Section::default()
    }];
    document
}

#[test]
fn a_rich_document_round_trips() {
    let original = rich_document();
    let view = ViewState {
        pageless: true,
        zoom_percent: 125,
    };
    let native = Native::with_view(original.clone(), view.clone());
    let bytes = native.to_bytes().expect("save");
    let read = Native::parse(&bytes).expect("read");

    assert_eq!(read.document(), &original, "the model comes back unchanged");
    assert_eq!(read.view(), &view, "pageless and zoom are kept");
    // Spelled out, so a change to PartialEq cannot hide a loss.
    let paragraph = read.document().paragraph_at(1).expect("the body paragraph");
    assert_eq!(paragraph.images().len(), 1, "the image is there");
    assert_eq!(paragraph.images()[0].media.bytes(), image(64, 32).bytes());
    assert_eq!(paragraph.images()[0].alt, "A checker");
    assert_eq!(paragraph.notes().len(), 1);
    assert_eq!(paragraph.cross_refs().len(), 1);
    assert!(
        read.document()
            .paragraph_at(0)
            .expect("heading")
            .bookmarks()
            .len()
            == 1
    );
    let table = read
        .document()
        .body()
        .iter()
        .find_map(|block| block.as_table())
        .expect("the table");
    assert_eq!(
        table.style.as_ref().map(|style| style.name.as_str()),
        Some("Banded Rows")
    );
    assert_eq!(table.rows[1].cells[0].col_span, 2);
}

#[test]
fn date_time_fields_and_the_save_time_survive_a_round_trip() {
    let text = "Saved on 12 November 2026";
    let start = "Saved on ".len();
    let end = start + "12 November 2026".len();
    let mut paragraph = Paragraph::new(BlockId(1), text);
    paragraph.set_date_fields(vec![
        DateTimeField::new(
            start..end,
            DateTimeSource::Updated,
            DateTimeFormat::DateLong,
        ),
        DateTimeField::new(0..5, DateTimeSource::Inserted, DateTimeFormat::DateIso),
    ]);
    let mut document = Document::from_paragraphs(vec![paragraph]);
    document.set_saved_at(1_760_000_000);
    let bytes = Native::new(document).to_bytes().expect("save");
    let read = Native::parse(&bytes).expect("read");
    let paragraph = read.document().paragraph_at(0).expect("the paragraph");
    assert_eq!(paragraph.date_fields().len(), 2, "both fields are kept");
    // Sorted by where they start.
    assert_eq!(
        paragraph.date_fields()[0],
        DateTimeField::new(0..5, DateTimeSource::Inserted, DateTimeFormat::DateIso)
    );
    assert_eq!(
        paragraph.date_fields()[1],
        DateTimeField::new(
            start..end,
            DateTimeSource::Updated,
            DateTimeFormat::DateLong
        )
    );
    assert_eq!(
        read.document().saved_at(),
        Some(1_760_000_000),
        "the last save time is written"
    );
}

#[test]
fn an_image_is_stored_once_however_often_it_is_used() {
    let media = image(32, 16);
    let first = {
        let mut paragraph = Paragraph::new(BlockId(0), "one");
        paragraph.set_images(vec![InlineImage::fitted(0, media.clone(), 300.0)]);
        paragraph
    };
    let second = {
        let mut paragraph = Paragraph::new(BlockId(0), "two");
        paragraph.set_images(vec![InlineImage::fitted(0, media.clone(), 300.0)]);
        paragraph
    };
    let native = Native::new(Document::from_paragraphs(vec![first, second]));
    let bytes = native.to_bytes().expect("save");

    let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).expect("zip");
    let mut names: Vec<String> = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).expect("entry");
        names.push(file.name().expect("a name").to_string());
    }
    assert_eq!(
        names
            .iter()
            .filter(|name| name.starts_with("media/"))
            .count(),
        1,
        "one media entry for two uses: {names:?}"
    );
    assert_eq!(
        names.first().map(String::as_str),
        Some("mimetype"),
        "mimetype first"
    );
    assert_eq!(
        names.get(1).map(String::as_str),
        Some("manifest.json"),
        "manifest second"
    );
    drop(archive);

    let read = Native::parse(&bytes).expect("read");
    let widths: Vec<f32> = read
        .document()
        .paragraphs()
        .flat_map(|paragraph| paragraph.images())
        .map(|image| image.width_pt)
        .collect();
    assert_eq!(widths.len(), 2, "both uses come back");
}

#[test]
fn unknown_extensions_survive_a_re_save() {
    let mut native = Native::new(rich_document());
    native.insert_extension("com.example.plugin", b"opaque plugin data".to_vec());
    let first = native.to_bytes().expect("save");

    let mut read = Native::parse(&first).expect("read");
    assert_eq!(
        read.extensions().collect::<Vec<_>>(),
        vec![("com.example.plugin", b"opaque plugin data".as_slice())]
    );
    read.document_mut();
    let second = read.to_bytes().expect("save again");
    let read_again = Native::parse(&second).expect("read again");
    assert_eq!(
        read_again.extensions().count(),
        1,
        "a save keeps what it does not understand"
    );
}

#[test]
fn saving_twice_gives_the_same_bytes() {
    let native = Native::new(rich_document());
    let first = native.to_bytes().expect("save");
    let second = native.to_bytes().expect("save again");
    assert_eq!(first, second, "saves are deterministic");
}

#[test]
fn foreign_and_damaged_files_are_refused() {
    let not_a_zip = Native::parse(b"just some text");
    assert!(matches!(not_a_zip, Err(NativeError::Zip(_))));

    // A ZIP that is not ours, and one whose manifest is newer than this build reads.
    let foreign = zip_with(&[
        ("mimetype", b"application/epub+zip"),
        ("other.txt", b"hello"),
    ]);
    assert!(matches!(
        Native::parse(&foreign),
        Err(NativeError::NotNative)
    ));
    let newer = zip_with(&[
        ("mimetype", MIMETYPE.as_bytes()),
        (
            "manifest.json",
            br#"{"format":"libertum-document","version":99,"generator":"later"}"#,
        ),
        ("document.json", b"{}"),
    ]);
    assert!(matches!(
        Native::parse(&newer),
        Err(NativeError::TooNew { found: 99 })
    ));
}

/// Builds a ZIP from raw entries, for the refusals above.
fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::default());
        for (name, bytes) in entries {
            zip.start_file(*name, options).expect("entry");
            zip.write_all(bytes).expect("write");
        }
        zip.finish().expect("finish");
    }
    buffer.into_inner()
}

proptest! {
    /// Whatever a document holds, it comes back exactly.
    #[test]
    fn any_document_round_trips(
        texts in proptest::collection::vec("[a-z ]{0,40}", 1..8),
        bold in proptest::collection::vec(proptest::bool::ANY, 1..8),
        pageless in proptest::bool::ANY,
        zoom in 25u16..400,
    ) {
        let paragraphs: Vec<Paragraph> = texts
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let mut paragraph = Paragraph::new(BlockId(index as u64 + 1), text.clone());
                if bold[index % bold.len()] && !text.is_empty() {
                    paragraph.set_runs(vec![Run {
                        range: 0..text.len(),
                        format: CharFormat {
                            weight: Some(700),
                            ..CharFormat::default()
                        },
                    }]);
                }
                paragraph
            })
            .collect();
        let document = Document::from_paragraphs(paragraphs);
        let native = Native::with_view(
            document.clone(),
            ViewState {
                pageless,
                zoom_percent: zoom,
            },
        );
        let bytes = native.to_bytes().expect("save");
        let read = Native::parse(&bytes).expect("read");
        prop_assert_eq!(read.document(), &document);
        prop_assert_eq!(read.view().pageless, pageless);
        prop_assert_eq!(read.view().zoom_percent, zoom);
    }
}

#[test]
fn a_watermark_round_trips() {
    let mut document = Document::from_paragraphs(vec![Paragraph::new(BlockId(0), "Text")]);
    document.sections[0].watermark = libertum_model::Watermark::new("DRAFT", false);
    let bytes = Native::new(document.clone()).to_bytes().expect("write");
    let back = Native::parse(&bytes).expect("read");
    assert_eq!(
        back.document().sections[0].watermark,
        document.sections[0].watermark
    );
}

#[test]
fn the_page_colour_survives_a_round_trip() {
    let mut document = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Body")]);
    document.sections[0].page_colour = Some([0xD9, 0xD9, 0xD9, 0xFF]);
    let bytes = Native::new(document).to_bytes().expect("save");
    let read = Native::parse(&bytes).expect("read");
    assert_eq!(
        read.document().sections[0].page_colour,
        Some([0xD9, 0xD9, 0xD9, 0xFF])
    );
}
