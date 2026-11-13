//! Images in Markdown (PLAN §7, Phase 7): loading linked files, warnings for the ones that
//! cannot be loaded, writing an edited block's images back, and saving unnamed images beside
//! the document.

use std::path::{Path, PathBuf};

use libertum_formats_common::media::sample_png;
use libertum_markdown::Markdown;
use libertum_model::{Block, Document, InlineImage, Operation};

fn fixture_dir() -> PathBuf {
    PathBuf::from(format!(
        "{}/../../tests/fixtures/md",
        env!("CARGO_MANIFEST_DIR")
    ))
}

/// A fresh, empty scratch folder for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("libertum-md-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create scratch folder");
    dir
}

fn write_png(path: &Path, width: u32, height: u32) -> Vec<u8> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create folder");
    }
    let media = sample_png(width, height).expect("sample png");
    std::fs::write(path, media.bytes()).expect("write png");
    media.bytes().to_vec()
}

fn paragraphs_with_images(document: &Document) -> Vec<&libertum_model::Paragraph> {
    document
        .paragraphs()
        .filter(|paragraph| !paragraph.images().is_empty())
        .collect()
}

fn append_text(markdown: &mut Markdown, containing: &str, text: &str) {
    let mut document = markdown.document().clone();
    let paragraph = document
        .paragraphs()
        .find(|paragraph| paragraph.text().contains(containing))
        .expect("paragraph to edit");
    let (id, offset) = (paragraph.id, paragraph.text().len());
    document
        .apply(Operation::InsertText {
            block: id,
            offset,
            text: text.to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
}

#[test]
fn the_image_fixture_round_trips_byte_for_byte_and_loads_its_image() {
    let path = fixture_dir().join("image.md");
    let bytes = std::fs::read(&path).expect("fixture");
    let markdown = Markdown::open(&path).expect("open");

    assert_eq!(markdown.to_bytes().expect("save"), bytes);

    let document = markdown.document();
    let with_images = paragraphs_with_images(document);
    assert_eq!(with_images.len(), 1, "only the local image loads");
    let paragraph = with_images[0];
    let image = &paragraph.images()[0];
    assert_eq!(image.alt, "A blue and white checker");
    assert_eq!(image.name, "image.png");
    assert_eq!(image.media.width_px(), 16);
    assert_eq!(
        paragraph.text(),
        "A checker  in a sentence.",
        "the image syntax is not also left as text"
    );
    assert_eq!(image.offset, "A checker ".len());

    let warnings = markdown.warnings().join("\n");
    assert!(warnings.contains("logo.png"), "{warnings}");
    assert!(warnings.contains("nothing.png"), "{warnings}");
    // The two images that did not load show their alt text.
    assert!(
        document
            .paragraphs()
            .any(|paragraph| paragraph.text() == "Remote logo")
    );
    assert!(
        document
            .paragraphs()
            .any(|paragraph| paragraph.text() == "Missing")
    );
}

#[test]
fn an_image_in_a_subfolder_loads_and_is_fitted_to_the_reading_width() {
    let dir = scratch("subfolder");
    write_png(&dir.join("pics").join("cat.png"), 800, 400);
    let md = dir.join("doc.md");
    std::fs::write(&md, "Intro ![A cat](pics/cat.png) outro.\n").expect("write");

    let markdown = Markdown::open(&md).expect("open");
    let paragraph = markdown.document().paragraphs().next().expect("paragraph");
    let image = &paragraph.images()[0];
    assert_eq!(image.name, "pics/cat.png");
    assert_eq!(image.alt, "A cat");
    assert!(
        (image.width_pt - 420.0).abs() < 0.01,
        "wide image is capped to 420 pt, got {}",
        image.width_pt
    );
    assert!(
        (image.height_pt - 210.0).abs() < 0.01,
        "and keeps its proportions"
    );
    assert!(markdown.warnings().is_empty(), "{:?}", markdown.warnings());
    assert_eq!(
        markdown.to_bytes().expect("save"),
        std::fs::read(&md).expect("read"),
        "an unedited save is byte-identical"
    );
}

#[test]
fn an_absolute_target_loads() {
    let dir = scratch("absolute");
    let png = dir.join("abs.png");
    write_png(&png, 8, 8);
    let text = format!("![Abs]({})\n", png.display().to_string().replace('\\', "/"));
    let md = dir.join("doc.md");
    std::fs::write(&md, &text).expect("write");
    let markdown = Markdown::open(&md).expect("open");
    assert_eq!(paragraphs_with_images(markdown.document()).len(), 1);
}

#[test]
fn missing_and_remote_targets_warn_and_keep_the_alt_text() {
    let dir = scratch("bad-targets");
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "![Gone](nope.png)\n\n![Web](https://example.com/a.png)\n\n![Data](data:image/png;base64,AAAA)\n",
    )
    .expect("write");
    let markdown = Markdown::open(&md).expect("open succeeds");
    assert!(paragraphs_with_images(markdown.document()).is_empty());
    let texts: Vec<&str> = markdown
        .document()
        .paragraphs()
        .map(|paragraph| paragraph.text())
        .collect();
    assert_eq!(texts, ["Gone", "Web", "Data"]);
    assert_eq!(markdown.warnings().len(), 3, "{:?}", markdown.warnings());
    assert!(markdown.warnings()[0].contains("not found"));
    assert!(markdown.warnings()[1].contains("not downloaded"));
    assert_eq!(
        markdown.to_bytes().expect("save"),
        std::fs::read(&md).expect("read")
    );
}

#[test]
fn an_undecodable_file_warns_and_does_not_fail_the_open() {
    let dir = scratch("undecodable");
    std::fs::write(dir.join("fake.png"), b"this is not a picture").expect("write");
    let md = dir.join("doc.md");
    std::fs::write(&md, "Before ![Fake](fake.png) after\n").expect("write");
    let markdown = Markdown::open(&md).expect("open");
    assert!(paragraphs_with_images(markdown.document()).is_empty());
    assert_eq!(markdown.warnings().len(), 1);
    assert!(
        markdown.warnings()[0].contains("fake.png"),
        "{:?}",
        markdown.warnings()
    );
}

#[test]
fn parse_without_a_folder_loads_no_image() {
    let dir = scratch("no-folder");
    write_png(&dir.join("a.png"), 8, 8);
    let markdown = Markdown::parse(b"![A](a.png)\n").expect("parse");
    assert!(paragraphs_with_images(markdown.document()).is_empty());
    assert!(
        markdown
            .warnings()
            .iter()
            .any(|warning| warning.contains("image")),
        "{:?}",
        markdown.warnings()
    );
}

#[test]
fn an_edited_block_writes_its_image_link_back() {
    let dir = scratch("edited");
    write_png(&dir.join("pics").join("cat.png"), 8, 8);
    let md = dir.join("doc.md");
    std::fs::write(
        &md,
        "# Title\n\nBefore ![A\\_cat](pics/cat.png \"Title\") after.\n\nUntouched **bold**.\n",
    )
    .expect("write");

    let mut markdown = Markdown::open(&md).expect("open");
    append_text(&mut markdown, "Before", " More.");
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert_eq!(
        saved,
        "# Title\n\nBefore ![A\\_cat](pics/cat.png \"Title\") after. More.\n\nUntouched **bold**.\n",
        "the image sits at its offset and keeps its title; the unedited blocks are verbatim"
    );

    // And it reads back to the same image.
    let again = Markdown::parse_with_base_dir(saved.as_bytes(), Some(&dir)).expect("parse");
    let paragraph = paragraphs_with_images(again.document())[0];
    assert_eq!(paragraph.images()[0].alt, "A_cat");
    assert_eq!(paragraph.images()[0].name, "pics/cat.png");
    assert_eq!(paragraph.images()[0].offset, "Before ".len());
}

#[test]
fn a_name_with_spaces_and_an_awkward_alt_survive_a_round_trip() {
    let dir = scratch("awkward");
    write_png(&dir.join("my pics").join("cat (1).png"), 8, 8);
    let md = dir.join("doc.md");
    std::fs::write(&md, "Look ![a [b] c](<my pics/cat (1).png>) here\n").expect("write");
    let mut markdown = Markdown::open(&md).expect("open");
    let image = markdown.document().paragraphs().next().expect("p").images()[0].clone();
    assert_eq!(image.name, "my pics/cat (1).png");
    assert_eq!(image.alt, "a [b] c");

    append_text(&mut markdown, "Look", "!");
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert_eq!(saved, "Look ![a \\[b\\] c](<my pics/cat (1).png>) here!\n");
    let again = Markdown::parse_with_base_dir(saved.as_bytes(), Some(&dir)).expect("parse");
    assert_eq!(
        again.document().paragraphs().next().expect("p").images()[0].alt,
        "a [b] c"
    );
}

#[test]
fn an_image_in_a_table_cell_is_written_back() {
    let dir = scratch("table");
    write_png(&dir.join("t.png"), 8, 8);
    let md = dir.join("doc.md");
    std::fs::write(&md, "| a | b |\n|---|---|\n| ![T](t.png) | x |\n").expect("write");
    let mut markdown = Markdown::open(&md).expect("open");
    let table = markdown
        .document()
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("table");
    let cell_paragraph = table.rows[1].cells[1].paragraphs[0].id;
    let mut document = markdown.document().clone();
    document
        .apply(Operation::InsertText {
            block: cell_paragraph,
            offset: 1,
            text: "y".to_owned(),
            format: None,
        })
        .expect("edit cell");
    markdown.set_document(document);
    let saved = String::from_utf8(markdown.to_bytes().expect("save")).expect("utf-8");
    assert!(saved.contains("| ![T](t.png) | xy |"), "{saved}");
}

fn document_with_unnamed_image() -> (Markdown, Vec<u8>) {
    let mut markdown = Markdown::parse(b"Picture here.\n").expect("parse");
    let mut document = markdown.document().clone();
    let id = document.first_id().expect("first block");
    let media = sample_png(10, 10).expect("png");
    let bytes = media.bytes().to_vec();
    let mut image = InlineImage::fitted(8, media, 420.0);
    image.alt = "Inserted".to_owned();
    document
        .apply(Operation::SetImages {
            block: id,
            images: vec![image],
        })
        .expect("insert image");
    markdown.set_document(document);
    (markdown, bytes)
}

#[test]
fn an_unnamed_image_is_saved_beside_the_document() {
    let dir = scratch("unnamed");
    let path = dir.join("notes.md");
    let (markdown, bytes) = document_with_unnamed_image();

    let saved = markdown.to_bytes_for(Some(&path)).expect("render");
    assert_eq!(
        String::from_utf8(saved.bytes.clone()).expect("utf-8"),
        "Picture ![Inserted](notes-image-1.png)here.\n"
    );
    assert_eq!(saved.assets.len(), 1);
    assert_eq!(saved.assets[0].file_name, "notes-image-1.png");
    assert_eq!(saved.assets[0].bytes, bytes);
    assert!(saved.warnings.is_empty());

    let warnings = markdown.save_with_report(&path).expect("save");
    assert!(warnings.is_empty());
    assert_eq!(
        std::fs::read(dir.join("notes-image-1.png")).expect("image file"),
        bytes
    );
    assert!(
        std::fs::read_to_string(&path)
            .expect("md")
            .contains("![Inserted](notes-image-1.png)")
    );

    // A second save reuses the file instead of writing notes-image-2.
    let again = markdown.to_bytes_for(Some(&path)).expect("render");
    assert!(again.assets.is_empty(), "{:?}", again.assets);
    assert!(
        String::from_utf8(again.bytes)
            .expect("utf-8")
            .contains("notes-image-1.png")
    );
}

#[test]
fn an_unnamed_image_never_overwrites_a_different_file() {
    let dir = scratch("no-overwrite");
    let path = dir.join("notes.md");
    std::fs::write(dir.join("notes-image-1.png"), b"someone else's file").expect("write");
    let (markdown, _) = document_with_unnamed_image();
    let saved = markdown.to_bytes_for(Some(&path)).expect("render");
    assert_eq!(saved.assets[0].file_name, "notes-image-2.png");
    assert_eq!(
        std::fs::read(dir.join("notes-image-1.png")).expect("read"),
        b"someone else's file"
    );
}

#[test]
fn an_unnamed_image_without_a_path_writes_alt_text_and_warns() {
    let (markdown, _) = document_with_unnamed_image();
    let saved = markdown.to_bytes_for(None).expect("render");
    assert_eq!(
        String::from_utf8(saved.bytes).expect("utf-8"),
        "Picture Insertedhere.\n"
    );
    assert!(saved.assets.is_empty());
    assert_eq!(saved.warnings.len(), 1, "{:?}", saved.warnings);
    assert!(saved.warnings[0].contains("no path"));
    assert_eq!(
        markdown.to_bytes().expect("plain to_bytes agrees"),
        b"Picture Insertedhere.\n"
    );
}

/// Puts `text` at the start of the paragraph that holds the image.
fn edit_image_paragraph(markdown: &mut Markdown, text: &str) {
    let mut document = markdown.document().clone();
    let paragraph = paragraphs_with_images(&document)
        .into_iter()
        .next()
        .expect("a paragraph with an image")
        .clone();
    let id = paragraph.id;
    document
        .apply(Operation::InsertText {
            block: id,
            offset: 0,
            text: text.to_owned(),
            format: None,
        })
        .expect("edit");
    markdown.set_document(document);
}

#[test]
fn an_edited_block_keeps_an_images_title_and_the_link_around_it() {
    let dir = scratch("image-title-link");
    write_png(&dir.join("pic.png"), 20, 20);
    let path = dir.join("doc.md");
    std::fs::write(
        &path,
        "Words.

[![a picture](pic.png \"The castle\")](https://example.com/castle)

More.
",
    )
    .expect("write");
    let mut markdown = Markdown::open(&path).expect("open");
    edit_image_paragraph(&mut markdown, "Look. ");
    let saved =
        String::from_utf8(markdown.to_bytes_for(Some(&path)).expect("save").bytes).expect("utf-8");
    assert!(
        saved.contains("Look. [![a picture](pic.png \"The castle\")](https://example.com/castle)"),
        "{saved}"
    );
}

#[test]
fn an_untitled_image_inside_a_link_keeps_the_link() {
    let dir = scratch("image-link");
    write_png(&dir.join("pic.png"), 20, 20);
    let path = dir.join("doc.md");
    std::fs::write(
        &path,
        "[![alt](pic.png)](https://example.com)
",
    )
    .expect("write");
    let mut markdown = Markdown::open(&path).expect("open");
    edit_image_paragraph(&mut markdown, "X ");
    let saved =
        String::from_utf8(markdown.to_bytes_for(Some(&path)).expect("save").bytes).expect("utf-8");
    assert!(
        saved.contains("X [![alt](pic.png)](https://example.com)"),
        "{saved}"
    );
}
