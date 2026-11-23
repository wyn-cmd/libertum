use libertum_layout::spike::generate_model_document;
use libertum_model::{Block, Document, InlineImage, Media, Operation, Paragraph, StyleId};
use lopdf::{Document as LoDocument, Object};
use printpdf::{PdfDocument, PdfParseOptions};

use super::{export, sfnt_has_table, srgb_profile};

fn pdf(document: &libertum_model::Document, archival: bool) -> (Vec<u8>, LoDocument) {
    let bytes = export(document, archival, "A Test Book").expect("PDF export");
    assert!(bytes.starts_with(b"%PDF-1."), "a PDF header");
    let parsed = LoDocument::load_mem(&bytes).expect("the PDF parses");
    (bytes, parsed)
}

#[test]
fn a_document_becomes_one_pdf_page_per_layout_page() {
    let document = generate_model_document(1, 7);
    let (bytes, parsed) = pdf(&document, false);
    let pages = parsed.get_pages();
    assert!(!pages.is_empty(), "at least one page");
    // Real text is written with embedded fonts, not as an image.
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("FontFile2") || text.contains("FontFile3"),
        "a font program is embedded"
    );
}

#[test]
fn archival_export_has_the_srgb_output_intent_and_pdfa_metadata() {
    let document = generate_model_document(1, 9);
    let (bytes, parsed) = pdf(&document, true);
    let catalog = parsed.catalog().expect("catalog");
    let intents = catalog.get(b"OutputIntents").expect("an output intent");
    let array = intents.as_array().expect("an array");
    assert_eq!(array.len(), 1);
    let intent = array[0].as_reference().expect("a reference");
    let intent = parsed
        .get_object(intent)
        .and_then(Object::as_dict)
        .expect("the output intent");
    let subtype = intent.get(b"S").and_then(Object::as_name).expect("S");
    assert_eq!(subtype, b"GTS_PDFA1");
    assert!(intent.get(b"DestOutputProfile").is_ok());
    assert!(catalog.get(b"Metadata").is_ok(), "XMP metadata");

    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("pdfaid:part"), "the PDF/A identification");
    assert!(text.contains("pdfaid:conformance"), "the PDF/A conformance");
}

#[test]
fn the_profile_is_a_valid_rgb_icc_header_and_tag_table() {
    let profile = srgb_profile();
    assert_eq!(profile.len() % 4, 0, "tag data is four-byte aligned");
    let size = u32::from_be_bytes(profile[0..4].try_into().expect("size")) as usize;
    assert_eq!(size, profile.len());
    assert_eq!(&profile[16..20], b"RGB ");
    assert_eq!(&profile[20..24], b"XYZ ");
    assert_eq!(&profile[36..40], b"acsp");
    let count = u32::from_be_bytes(profile[128..132].try_into().expect("count"));
    assert_eq!(count, 9);
    for index in 0..count as usize {
        let at = 132 + index * 12;
        let offset =
            u32::from_be_bytes(profile[at + 4..at + 8].try_into().expect("offset")) as usize;
        let length =
            u32::from_be_bytes(profile[at + 8..at + 12].try_into().expect("length")) as usize;
        assert!(offset + length <= profile.len());
        assert_eq!(offset % 4, 0);
    }
}

#[test]
fn the_pdf_text_can_be_searched_and_copied() {
    let mut document = Document::new();
    for text in ["Hello searchable world", "A second line of text"] {
        let id = document.alloc_id();
        let mut paragraph = Paragraph::new(id, text);
        paragraph.props.style = StyleId::BODY;
        document.append_blocks(vec![Block::Paragraph(paragraph)]);
    }
    let bytes = export(&document, false, "Search").expect("PDF export");
    let mut warnings = Vec::new();
    let parsed = PdfDocument::parse(&bytes, &PdfParseOptions::default(), &mut warnings)
        .expect("the exported PDF parses");
    let text = parsed
        .extract_text()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("Hello"), "extracted: {text:?}");
    assert!(text.contains("searchable"), "extracted: {text:?}");
    assert!(text.contains("world"), "extracted: {text:?}");
}

#[test]
fn an_inline_image_is_embedded_as_an_xobject() {
    let mut document = Document::new();
    let id = document.alloc_id();
    let paragraph = Paragraph::new(id, "Picture:");
    document.append_blocks(vec![Block::Paragraph(paragraph)]);
    let media = Media::new(include_bytes!("../../../../tests/fixtures/md/image.png").to_vec())
        .expect("the fixture is a PNG");
    document
        .apply(Operation::SetImages {
            block: id,
            images: vec![InlineImage {
                offset: 0,
                media: std::sync::Arc::new(media),
                width_pt: 30.0,
                height_pt: 30.0,
                alt: "a picture".to_owned(),
                name: "image.png".to_owned(),
            }],
        })
        .expect("set the image");
    let bytes = export(&document, false, "Picture").expect("PDF export");
    let parsed = LoDocument::load_mem(&bytes).expect("parse");
    let has_image = parsed.objects.values().any(|object| {
        object
            .as_stream()
            .ok()
            .and_then(|stream| stream.dict.get(b"Subtype").ok())
            .and_then(|subtype| subtype.as_name().ok())
            == Some(b"Image".as_slice())
    });
    assert!(has_image, "an image XObject is present");
}

#[test]
fn a_watermark_is_written_on_every_page_in_both_variants() {
    let mut document = generate_model_document(3, 7);
    for section in &mut document.sections {
        section.watermark = libertum_model::Watermark::new("CONFIDENTIAL", true);
    }
    let pages = generate_model_document(3, 7);
    for archival in [false, true] {
        let bytes = export(&document, archival, "Marked").expect("PDF export");
        let plain = export(&pages, archival, "Plain").expect("PDF export");
        let mut warnings = Vec::new();
        let parsed = PdfDocument::parse(&bytes, &PdfParseOptions::default(), &mut warnings)
            .expect("the exported PDF parses");
        let count = parsed.pages.len();
        assert!(count >= 1, "at least one page");
        let text = parsed
            .extract_text()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            text.matches("CONFIDENTIAL").count() >= count,
            "the watermark text is on each of {count} pages: {text:?}"
        );
        assert!(bytes.len() > plain.len(), "the watermark adds content");
    }
}

#[test]
fn headings_become_bookmarks_and_the_contents_list_does_not() {
    let mut document = Document::new();
    // The contents title is heading-styled and is followed by one generated entry per chapter.
    let title = document.alloc_id();
    let mut contents = Paragraph::new(title, "Contents");
    contents.props.outline_level = Some(1);
    document.append_blocks(vec![Block::Paragraph(contents)]);
    let mut chapters = Vec::new();
    for text in ["Chapter One", "Chapter Two"] {
        let id = document.alloc_id();
        chapters.push(id);
        let mut paragraph = Paragraph::new(id, text);
        paragraph.props.outline_level = Some(1);
        document.append_blocks(vec![Block::Paragraph(paragraph)]);
    }
    for (index, target) in chapters.iter().enumerate() {
        let id = document.alloc_id();
        let mut entry = Paragraph::new(id, format!("Chapter {index}"));
        entry.props.toc_target = Some(*target);
        document.append_blocks(vec![Block::Paragraph(entry)]);
    }
    let bytes = export(&document, false, "Outline").expect("PDF export");
    let parsed = LoDocument::load_mem(&bytes).expect("parse");
    let catalog = parsed.catalog().expect("catalog");
    assert!(catalog.get(b"Outlines").is_ok(), "an outline is written");
    // Two chapters, and no bookmark for the "Contents" title.
    let titles = parsed
        .objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter(|dict| dict.has(b"Title") && dict.has(b"Dest"))
        .count();
    assert_eq!(titles, 2, "one bookmark per chapter heading");
    // Each generated entry is a link that jumps to a page.
    // The annotations sit in each page's /Annots array.
    let links = parsed
        .get_pages()
        .values()
        .filter_map(|id| parsed.get_object(*id).and_then(Object::as_dict).ok())
        .filter_map(|page| page.get(b"Annots").ok())
        .filter_map(|annots| annots.as_array().ok())
        .flatten()
        .filter_map(|annot| annot.as_dict().ok())
        .filter(|dict| {
            dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) == Some(b"Link".as_slice())
        })
        .count();
    assert_eq!(links, 2, "one link per contents entry");
}

// ---------------------------------------------------------------- East Asian text

/// A minimal sfnt table directory with these tags.
fn sfnt(tags: &[&[u8; 4]]) -> Vec<u8> {
    let mut out = vec![0, 1, 0, 0];
    out.extend((tags.len() as u16).to_be_bytes());
    out.extend([0; 6]);
    for tag in tags {
        out.extend(*tag);
        out.extend([0; 12]);
    }
    out
}

#[test]
fn a_font_tables_are_found_in_a_single_font_and_in_a_collection() {
    let plain = sfnt(&[b"cmap", b"CFF2", b"fvar"]);
    assert!(sfnt_has_table(&plain, 0, b"CFF2"));
    assert!(!sfnt_has_table(&plain, 0, b"glyf"));
    // A collection of two fonts: the second has the table, the first does not.
    let first = sfnt(&[b"glyf"]);
    let second = sfnt(&[b"CFF2"]);
    let header = 12 + 4 * 2;
    let mut collection = b"ttcf".to_vec();
    collection.extend([0, 1, 0, 0]);
    collection.extend(2u32.to_be_bytes());
    collection.extend((header as u32).to_be_bytes());
    collection.extend(((header + first.len()) as u32).to_be_bytes());
    collection.extend(&first);
    collection.extend(&second);
    assert!(!sfnt_has_table(&collection, 0, b"CFF2"));
    assert!(sfnt_has_table(&collection, 1, b"CFF2"));
    assert!(sfnt_has_table(&collection, 0, b"glyf"));
    // Out of range, truncated and empty data are "no", never a panic.
    assert!(!sfnt_has_table(&collection, 2, b"CFF2"));
    assert!(!sfnt_has_table(&collection[..20], 1, b"CFF2"));
    assert!(!sfnt_has_table(&[], 0, b"CFF2"));
}

#[test]
fn chinese_japanese_and_korean_text_exports_in_both_modes_with_a_turned_watermark() {
    let lines = [
        "你好，世界 / こんにちは / 안녕하세요",
        "春眠不觉晓，处处闻啼鸟。夜来风雨声，花落知多少。",
        "吾輩は猫である。名前はまだ無い。",
        "동해물과 백두산이 마르고 닳도록 하느님이 보우하사 우리나라 만세.",
    ];
    let mut document = Document::from_paragraphs(
        lines
            .iter()
            .enumerate()
            .map(|(i, text)| Paragraph::new(libertum_model::BlockId(i as u64 + 1), *text))
            .collect(),
    );
    for section in &mut document.sections {
        section.language = Some("ja-JP".into());
        section.watermark = libertum_model::Watermark::new("草稿 초안", true);
    }
    for archival in [false, true] {
        let (_, parsed) = pdf(&document, archival);
        assert!(!parsed.get_pages().is_empty(), "archival: {archival}");
    }
}

fn vertical_document(on: bool) -> Document {
    let mut document = Document::from_paragraphs(vec![Paragraph::new(
        libertum_model::BlockId(1),
        "縦書きの日本語です。ABC 123 привіт。",
    )]);
    for section in &mut document.sections {
        section.vertical = on;
        section.language = Some("ja-JP".into());
    }
    document
}

/// The page's content stream, decoded.
fn page_content(parsed: &LoDocument) -> String {
    let page = *parsed.get_pages().values().next().expect("a page");
    String::from_utf8_lossy(&parsed.get_page_content(page)).into_owned()
}

#[test]
fn vertical_text_is_written_on_the_real_sheet_and_turned_back_by_one_matrix() {
    let (_, parsed) = pdf(&vertical_document(true), false);
    // The page is the real sheet: A4 portrait, not the turned layout page.
    let page_id = *parsed.get_pages().values().next().expect("a page");
    let media_box = parsed
        .get_object(page_id)
        .and_then(Object::as_dict)
        .and_then(|page| page.get(b"MediaBox"))
        .and_then(Object::as_array)
        .expect("MediaBox")
        .iter()
        .map(|n| n.as_float().unwrap_or(0.0))
        .collect::<Vec<_>>();
    assert!(
        (media_box[2] - 595.28).abs() < 0.1 && (media_box[3] - 841.89).abs() < 0.1,
        "{media_box:?}"
    );
    // One matrix turns the content a quarter turn clockwise: 0 -1 1 0 0 <sheet height> cm.
    let content = page_content(&parsed);
    assert!(
        content.contains("0 -1 1 0 0 841.89 cm") || content.contains("0 -1 1 0 0 841."),
        "{}",
        &content[..content.len().min(400)]
    );
    assert!(content.contains("\nq\n") || content.starts_with("q\n") || content.contains("q\n"));
    // And the graphics state is restored at the end.
    assert!(
        content.trim_end().ends_with('Q'),
        "{}",
        &content[content.len().saturating_sub(80)..]
    );
}

#[test]
fn horizontal_pages_have_no_turning_matrix() {
    let (_, parsed) = pdf(&vertical_document(false), false);
    let content = page_content(&parsed);
    assert!(
        !content.contains("0 -1 1 0 0"),
        "{}",
        &content[..content.len().min(300)]
    );
}

#[test]
fn vertical_text_exports_in_archival_mode_too() {
    let (_, parsed) = pdf(&vertical_document(true), true);
    assert!(!parsed.get_pages().is_empty());
}

#[test]
fn a_picture_on_a_vertical_page_is_turned_back_upright_in_the_pdf() {
    let red =
        libertum_formats_common::media::png_from_rgba(60, 20, [200u8, 30, 30, 255].repeat(60 * 20))
            .expect("png");
    for vertical in [true, false] {
        let mut paragraph = Paragraph::new(libertum_model::BlockId(1), "");
        paragraph.set_images(vec![libertum_model::InlineImage {
            offset: 0,
            media: red.clone(),
            width_pt: 120.0,
            height_pt: 40.0,
            alt: String::new(),
            name: String::new(),
        }]);
        let mut document = Document::from_paragraphs(vec![paragraph]);
        for section in &mut document.sections {
            section.vertical = vertical;
        }
        let (_, parsed) = pdf(&document, false);
        let content = page_content(&parsed);
        // Vertical: a counter-turn matrix just for the picture, inside the page's own turn.
        assert_eq!(
            content.contains("0 1 -1 0"),
            vertical,
            "{vertical}: {content}"
        );
        assert!(content.contains("Do"), "the picture is drawn: {content}");
    }
}

#[test]
fn a_vertical_pages_header_is_written_after_the_turned_content_so_it_stays_upright() {
    let mut document = vertical_document(true);
    for section in &mut document.sections {
        section.headers = libertum_model::HeadersFooters::with_default("RUNNINGHEAD");
    }
    let (_, parsed) = pdf(&document, false);
    let content = page_content(&parsed);
    // The page matrix opens a `q` block that is closed before the header's text is written.
    let closed = content.rfind("\nQ").expect("the turned block is closed");
    let header_at = content
        .rfind("TJ")
        .or(content.rfind("Tj"))
        .expect("some text");
    assert!(
        header_at > closed,
        "header written after the turned block ends"
    );
    // Text drawn there has no turn of its own: nothing like the page matrix after the close.
    assert!(
        !content[closed..].contains("0 -1 1 0"),
        "{}",
        &content[closed..]
    );
}
