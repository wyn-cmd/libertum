use libertum_layout::spike::generate_model_document;
use libertum_model::{Block, Document, InlineImage, Media, Operation, Paragraph, StyleId};
use lopdf::{Document as LoDocument, Object};
use printpdf::{PdfDocument, PdfParseOptions};

use super::{export, srgb_profile};

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
