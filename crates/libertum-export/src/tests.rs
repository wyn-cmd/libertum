use std::io::Read;

use libertum_model::{
    BOLD, Block, CharFormat, Document, ListItem, ListStyle, ParaProps, Paragraph, Run, StyleId,
};

use crate::{ChapterLevels, WHOLE_DOCUMENT, ao3_fragment, build_epub, chapters, document_text};

fn make_paragraph(document: &mut Document, text: &str, style: StyleId, level: Option<u8>) -> Block {
    let mut paragraph = Paragraph::new(document.alloc_id(), text);
    paragraph.props = ParaProps {
        style,
        outline_level: level,
        ..ParaProps::default()
    };
    Block::Paragraph(paragraph)
}

fn push_paragraph(document: &mut Document, text: &str, style: StyleId, level: Option<u8>) {
    let block = make_paragraph(document, text, style, level);
    document.append_blocks(vec![block]);
}

fn push_styled(document: &mut Document, text: &str, bold: bool, italic: bool) {
    let format = CharFormat {
        weight: bold.then_some(BOLD),
        italic: italic.then_some(true),
        ..CharFormat::default()
    };
    let id = document.alloc_id();
    let mut paragraph = Paragraph::with_runs(
        id,
        text,
        vec![Run {
            range: 0..text.len(),
            format,
        }],
    );
    paragraph.props = ParaProps::default();
    document.append_blocks(vec![Block::Paragraph(paragraph)]);
}

fn push_list(document: &mut Document, text: &str, level: u8, group: u64) {
    let mut paragraph = Paragraph::new(document.alloc_id(), text);
    paragraph.props = ParaProps {
        list: Some(ListItem {
            style: ListStyle::Bullet,
            level,
            group,
        }),
        ..ParaProps::default()
    };
    document.append_blocks(vec![Block::Paragraph(paragraph)]);
}

fn book() -> Document {
    let mut document = Document::new();
    push_paragraph(&mut document, "Chapter One", StyleId::HEADING_1, Some(1));
    push_styled(&mut document, "Bold & <tag>", true, false);
    push_paragraph(&mut document, "Quoted line", StyleId::QUOTE, None);
    push_list(&mut document, "First", 0, 1);
    push_list(&mut document, "Nested", 1, 1);
    push_list(&mut document, "Second", 0, 1);
    push_paragraph(&mut document, "Scene", StyleId::HEADING_2, Some(2));
    push_paragraph(&mut document, "After the scene.", StyleId::BODY, None);
    push_paragraph(&mut document, "Chapter Two", StyleId::HEADING_1, Some(1));
    push_paragraph(&mut document, "Second chapter.", StyleId::BODY, None);
    document
}

#[test]
fn chapters_follow_the_selected_heading_levels() {
    let document = book();
    let h1 = ChapterLevels {
        h1: true,
        h2: false,
        h3: false,
        h4: false,
    };
    let both = ChapterLevels::default();
    let titles = |levels| {
        chapters(&document, levels)
            .into_iter()
            .map(|chapter| chapter.title)
            .collect::<Vec<_>>()
    };
    assert_eq!(titles(h1), ["Chapter One", "Chapter Two"]);
    assert_eq!(titles(both), ["Chapter One", "Scene", "Chapter Two"]);
}

#[test]
fn a_document_without_matching_headings_is_one_chapter() {
    let mut document = Document::new();
    push_paragraph(&mut document, "Only prose.", StyleId::BODY, None);
    let chapters = chapters(&document, ChapterLevels::default());
    assert_eq!(chapters.len(), 1);
    assert_eq!(chapters[0].title, WHOLE_DOCUMENT);
    assert_eq!(chapters[0].start, 0);
    assert_eq!(chapters[0].end, document.body().iter().count());
}

#[test]
fn ao3_fragment_keeps_emphasis_lists_and_quotes() {
    let document = book();
    let levels = ChapterLevels {
        h1: true,
        h2: false,
        h3: false,
        h4: false,
    };
    let chapter = &chapters(&document, levels)[0];
    let html = ao3_fragment(&document, chapter);
    assert!(
        !html.contains("Chapter One"),
        "the title is separate from the body"
    );
    assert!(
        html.contains("<strong>Bold &amp; &lt;tag&gt;</strong>"),
        "{html}"
    );
    assert!(html.contains("<blockquote>"));
    assert!(html.contains("<ul>"));
    assert!(html.contains("<li>First"), "{html}");
    assert!(html.contains("<ul>\n<li>Nested</li>"), "{html}");
    assert!(html.contains("<li>Second</li>"));
    assert!(html.contains("<h2>") && html.contains("Scene"), "{html}");
}

#[test]
fn ao3_fragment_leaves_generated_contents_out() {
    let mut document = Document::new();
    push_paragraph(&mut document, "Chapter", StyleId::HEADING_1, Some(1));
    let target = document.body().iter().next().map(Block::id);
    let mut entry = Paragraph::new(document.alloc_id(), "Generated entry");
    entry.props.toc_target = target;
    document.append_blocks(vec![Block::Paragraph(entry)]);
    push_paragraph(&mut document, "Body", StyleId::BODY, None);
    let chapter = &chapters(&document, ChapterLevels::default())[0];
    let html = ao3_fragment(&document, chapter);
    assert!(!html.contains("Generated entry"), "{html}");
    assert!(html.contains("<p>Body</p>"), "{html}");
}

#[test]
fn plain_text_separates_paragraphs_and_collects_tables() {
    let mut document = Document::new();
    push_paragraph(&mut document, "One", StyleId::BODY, None);
    push_paragraph(&mut document, "Two", StyleId::BODY, None);
    let mut table = document.new_table(1, 2, 400.0);
    let first = document.alloc_id();
    let second = document.alloc_id();
    table.rows[0].cells[0].paragraphs[0] = Paragraph::new(first, "A");
    table.rows[0].cells[1].paragraphs[0] = Paragraph::new(second, "B");
    document.append_blocks(vec![Block::Table(table)]);
    let text = document_text(&document);
    assert!(text.contains("One\n\nTwo\n\n"), "{text}");
    assert!(text.contains("A\tB"), "{text}");
}

#[test]
fn epub_has_the_required_package_layout() {
    let document = book();
    let bytes = build_epub(&document, ChapterLevels::default(), "A Book", "en-GB").expect("epub");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    {
        let first = archive.by_index(0).expect("first entry");
        assert_eq!(first.name().expect("name"), "mimetype");
        assert_eq!(first.compression(), zip::CompressionMethod::Stored);
    }
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .and_then(|file| file.name().ok().map(|name| name.to_string()))
        })
        .collect();
    names.sort();
    assert!(
        names.contains(&"META-INF/container.xml".to_owned()),
        "{names:?}"
    );
    assert!(names.contains(&"OEBPS/content.opf".to_owned()), "{names:?}");
    assert!(names.contains(&"OEBPS/nav.xhtml".to_owned()), "{names:?}");
    assert!(
        names.contains(&"OEBPS/text/chapter-0001.xhtml".to_owned()),
        "{names:?}"
    );
    assert!(
        names.contains(&"OEBPS/text/chapter-0002.xhtml".to_owned()),
        "{names:?}"
    );

    let mut package = String::new();
    archive
        .by_name("OEBPS/content.opf")
        .expect("content.opf")
        .read_to_string(&mut package)
        .expect("read");
    assert!(package.contains("<dc:title>A Book</dc:title>"), "{package}");
    assert!(
        package.contains("<dc:language>en-GB</dc:language>"),
        "{package}"
    );
    assert!(package.contains("dcterms:modified"), "{package}");

    let mut chapter = String::new();
    archive
        .by_name("OEBPS/text/chapter-0001.xhtml")
        .expect("chapter")
        .read_to_string(&mut chapter)
        .expect("read");
    assert!(chapter.contains("<h1>Chapter One</h1>"), "{chapter}");
    assert!(chapter.contains("<strong>Bold"), "{chapter}");
}

#[test]
fn the_generated_contents_title_is_not_a_chapter() {
    let mut document = Document::new();
    push_paragraph(&mut document, "Contents", StyleId::HEADING_1, Some(1));
    let target = document.body().iter().next().map(Block::id);
    let mut entry = Paragraph::new(document.alloc_id(), "Chapter One");
    entry.props.toc_target = target;
    document.append_blocks(vec![Block::Paragraph(entry)]);
    push_paragraph(&mut document, "Chapter One", StyleId::HEADING_1, Some(1));
    push_paragraph(&mut document, "The body.", StyleId::BODY, None);
    let chapters = chapters(&document, ChapterLevels::default());
    assert_eq!(chapters.len(), 1, "{chapters:?}");
    assert_eq!(chapters[0].title, "Chapter One");
}

fn epub_file(bytes: Vec<u8>, name: &str) -> String {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("zip");
    let mut text = String::new();
    archive
        .by_name(name)
        .expect("the file is in the package")
        .read_to_string(&mut text)
        .expect("read");
    text
}

#[test]
fn the_epub_contents_list_nests_sections_under_their_chapters() {
    let mut document = Document::new();
    push_paragraph(&mut document, "Chapter One", StyleId::HEADING_1, Some(1));
    push_paragraph(&mut document, "Body.", StyleId::BODY, None);
    push_paragraph(&mut document, "A Section", StyleId::HEADING_2, Some(2));
    push_paragraph(&mut document, "More.", StyleId::BODY, None);
    push_paragraph(&mut document, "Chapter Two", StyleId::HEADING_1, Some(1));
    push_paragraph(&mut document, "End.", StyleId::BODY, None);
    let levels = ChapterLevels {
        h2: true,
        ..ChapterLevels::default()
    };
    let bytes = build_epub(&document, levels, "Nested", "en-GB").expect("epub");
    let nav = epub_file(bytes, "OEBPS/nav.xhtml");
    let one = nav.find("Chapter One").expect("chapter one");
    let section = nav.find("A Section").expect("the section");
    let two = nav.find("Chapter Two").expect("chapter two");
    assert!(one < section && section < two, "{nav}");
    // The section's list opens after chapter one and closes before chapter two.
    assert_eq!(nav.matches("<ol>").count(), 2, "{nav}");
    assert_eq!(nav.matches("</ol>").count(), 2, "{nav}");
    assert!(nav[one..section].contains("<ol>"), "{nav}");
    assert!(nav[section..two].contains("</ol>"), "{nav}");
}

#[test]
fn the_epub_keeps_what_comes_before_the_first_chapter() {
    let mut document = Document::new();
    push_paragraph(&mut document, "A Title Page", StyleId::BODY, None);
    push_paragraph(&mut document, "Chapter One", StyleId::HEADING_1, Some(1));
    push_paragraph(&mut document, "Body.", StyleId::BODY, None);
    let bytes = build_epub(&document, ChapterLevels::default(), "Front", "en-GB").expect("epub");
    let front = epub_file(bytes.clone(), "OEBPS/text/chapter-0001.xhtml");
    assert!(front.contains("A Title Page"), "{front}");
    let nav = epub_file(bytes, "OEBPS/nav.xhtml");
    assert!(
        nav.contains("Front matter") && nav.contains("Chapter One"),
        "{nav}"
    );
}

#[test]
fn ao3_fragment_points_linked_pictures_and_videos_at_their_addresses() {
    use libertum_model::{InlineImage, MediaKind, MediaLink, test_png_header};
    let mut document = Document::default();
    let png = libertum_model::Media::new(test_png_header(4, 4)).expect("png");
    let mut paragraph = Paragraph::new(document.alloc_id(), "See: ");
    let image = |offset: usize, link: MediaLink, alt: &str| {
        let mut image = InlineImage::fitted(offset, std::sync::Arc::new(png.clone()), 100.0);
        image.name = link.name();
        image.alt = alt.to_owned();
        image
    };
    let link = |url: &str, kind| MediaLink {
        url: url.to_owned(),
        kind,
    };
    paragraph.set_images(vec![
        image(
            5,
            link("https://a.example/p.png?x=1&y=2", MediaKind::Image),
            "A \"pic\"",
        ),
        image(5, link("https://a.example/clip.MP4", MediaKind::Video), ""),
        image(
            5,
            link("https://www.youtube.com/watch?v=abc", MediaKind::Video),
            "Trailer",
        ),
        // An ordinary picture is left out of an AO3 chapter, as before.
        InlineImage::fitted(5, std::sync::Arc::new(png), 100.0),
    ]);
    document.append_blocks(vec![Block::Paragraph(paragraph)]);
    let levels = ChapterLevels {
        h1: false,
        h2: false,
        h3: false,
        h4: false,
    };
    let chapter = &chapters(&document, levels)[0];
    let html = ao3_fragment(&document, chapter);
    assert!(
        html.contains(
            "<img src=\"https://a.example/p.png?x=1&amp;y=2\" alt=\"A &quot;pic&quot;\" />"
        ),
        "{html}"
    );
    assert!(
        html.contains("<video controls src=\"https://a.example/clip.MP4\"></video>"),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"https://www.youtube.com/watch?v=abc\">Trailer</a>"),
        "{html}"
    );
    assert_eq!(html.matches("<img").count(), 1, "{html}");
}
