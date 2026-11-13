//! EPUB 3 packaging: one XHTML file per chapter, a navigation document, the images the
//! chapters use and the package metadata, in the container's required order.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io::Write;

use chrono::Utc;
use libertum_model::Document;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::html::{ImageAsset, chapter_html};
use crate::{Chapter, ChapterLevels, chapters};

#[derive(Debug, thiserror::Error)]
pub enum EpubError {
    #[error("could not write the EPUB package: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("could not write the EPUB package: {0}")]
    Io(#[from] std::io::Error),
}

/// Builds an EPUB 3 book. `title` is the book's title (the app passes the file's name when
/// the document has no title of its own); a document without matching headings becomes one
/// chapter.
pub fn build_epub(
    document: &Document,
    levels: ChapterLevels,
    title: &str,
    language: &str,
) -> Result<Vec<u8>, EpubError> {
    let chapters = chapters(document, levels);
    let title = if title.trim().is_empty() {
        chapters
            .first()
            .map_or_else(|| "Untitled".to_owned(), |chapter| chapter.title.clone())
    } else {
        title.trim().to_owned()
    };

    let mut images: BTreeMap<String, ImageAsset> = BTreeMap::new();
    let mut documents: Vec<(String, String, String)> = Vec::new();
    // Each document's heading level in the contents list; front matter sits at the top level.
    let mut levels_of: Vec<u8> = Vec::new();
    // What comes before the first chapter (a title page, a dedication) is still part of the
    // book: it becomes its own first document rather than being dropped.
    let mut parts: Vec<Chapter> = Vec::new();
    if let Some(first) = chapters.first().filter(|first| first.start > 0) {
        parts.push(Chapter {
            title: FRONT_MATTER.to_owned(),
            start: 0,
            end: first.start,
            level: None,
        });
    }
    parts.extend(chapters.iter().cloned());
    for chapter in &parts {
        let index = documents.len();
        let html = chapter_html(document, chapter, Some("../images/"));
        if chapter.title == FRONT_MATTER
            && chapter.level.is_none()
            && !has_content(&html.body)
            && html.images.is_empty()
        {
            continue;
        }
        levels_of.push(chapter.level.unwrap_or(1));
        for image in html.images {
            images.insert(image.name.clone(), image);
        }
        let heading = if chapter.level.is_some() {
            format!("<h1>{}</h1>\n", escape(&chapter.title))
        } else {
            String::new()
        };
        let name = format!("text/chapter-{:04}.xhtml", index + 1);
        let body = format!("{heading}{}", html.body);
        documents.push((name, chapter.title.clone(), body));
    }

    // A stable identifier: the same book bytes produce the same EPUB.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    title.hash(&mut hasher);
    for (_, _, body) in &documents {
        body.hash(&mut hasher);
    }
    let identifier = format!("urn:libertum:{:016x}", hasher.finish());
    let modified = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let language = if language.trim().is_empty() {
        "en-GB"
    } else {
        language.trim()
    };

    let mut manifest = String::new();
    manifest.push_str(
        "    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n",
    );
    manifest.push_str("    <item id=\"css\" href=\"style.css\" media-type=\"text/css\"/>\n");
    for (index, (name, _, _)) in documents.iter().enumerate() {
        manifest.push_str(&format!(
            "    <item id=\"text-{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            index + 1,
            escape_attr(name)
        ));
    }
    for (index, (name, image)) in images.iter().enumerate() {
        manifest.push_str(&format!(
            "    <item id=\"image-{}\" href=\"images/{}\" media-type=\"{}\"/>\n",
            index + 1,
            escape_attr(name),
            image.mime
        ));
    }
    let mut spine = String::new();
    for index in 0..documents.len() {
        spine.push_str(&format!("    <itemref idref=\"text-{}\"/>\n", index + 1));
    }
    let package = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"{language}\">\n\
  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n\
    <dc:identifier id=\"bookid\">{identifier}</dc:identifier>\n\
    <dc:title>{title}</dc:title>\n\
    <dc:language>{language}</dc:language>\n\
    <meta property=\"dcterms:modified\">{modified}</meta>\n\
  </metadata>\n\
  <manifest>\n{manifest}  </manifest>\n\
  <spine>\n{spine}  </spine>\n\
</package>\n",
        title = escape(&title),
        identifier = escape(&identifier),
        language = escape(language),
        modified = modified,
    );

    let nav_items = nav_list(&documents, &levels_of);
    let nav = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" lang=\"{language}\" xml:lang=\"{language}\">\n\
<head><meta charset=\"utf-8\"/><title>{title}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"style.css\"/></head>\n\
<body>\n\
  <nav epub:type=\"toc\" id=\"toc\">\n\
    <h1>Contents</h1>\n\
    {nav_items}\
  </nav>\n\
</body>\n\
</html>\n",
        title = escape(&title),
    );

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut cursor);
        let stored = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        let deflated = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());

        // EPUB requires the mimetype entry first, stored, with no extra fields.
        zip.start_file("mimetype", stored)?;
        zip.write_all(b"application/epub+zip")?;
        zip.start_file("META-INF/container.xml", deflated)?;
        zip.write_all(CONTAINER.as_bytes())?;
        zip.start_file("OEBPS/content.opf", deflated)?;
        zip.write_all(package.as_bytes())?;
        zip.start_file("OEBPS/nav.xhtml", deflated)?;
        zip.write_all(nav.as_bytes())?;
        zip.start_file("OEBPS/style.css", deflated)?;
        zip.write_all(STYLE.as_bytes())?;
        for (name, chapter_title, body) in &documents {
            let xhtml = format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<!DOCTYPE html>\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\" lang=\"{language}\" xml:lang=\"{language}\">\n\
<head><meta charset=\"utf-8\"/><title>{title}</title><link rel=\"stylesheet\" type=\"text/css\" href=\"../style.css\"/></head>\n\
<body>\n{body}</body>\n\
</html>\n",
                title = escape(chapter_title),
            );
            zip.start_file(format!("OEBPS/{name}"), deflated)?;
            zip.write_all(xhtml.as_bytes())?;
        }
        for (name, image) in &images {
            zip.start_file(format!("OEBPS/images/{name}"), deflated)?;
            zip.write_all(&image.bytes)?;
        }
        zip.finish()?;
    }
    Ok(cursor.into_inner())
}

const CONTAINER: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n\
  <rootfiles>\n\
    <rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n\
  </rootfiles>\n\
</container>\n";

const STYLE: &str = "body { font-family: serif; line-height: 1.5; margin: 0 5%; }\n\
h1 { font-size: 1.5em; margin: 1.2em 0 0.8em; }\n\
h2, h3, h4, h5, h6 { font-size: 1.15em; margin: 1em 0 0.6em; }\n\
p { margin: 0 0 0.7em; text-indent: 0; }\n\
blockquote { margin: 0.8em 1.5em; font-style: italic; }\n\
ul, ol { margin: 0.6em 0 0.8em 1.4em; }\n\
table { border-collapse: collapse; margin: 0.8em 0; }\n\
td { border: 1px solid #999; padding: 0.2em 0.4em; vertical-align: top; }\n\
img { max-width: 100%; height: auto; }\n\
hr { border: 0; border-top: 1px solid #999; margin: 1.2em 0; }\n";

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_attr(text: &str) -> String {
    escape(text)
}

/// Whether an XHTML body holds any text, table or picture (empty paragraphs do not count).
fn has_content(body: &str) -> bool {
    if body.contains("<img") || body.contains("<table") {
        return true;
    }
    let mut in_tag = false;
    for character in body.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => return true,
            _ => {}
        }
    }
    false
}

/// Title of the part of the book before its first chapter heading.
const FRONT_MATTER: &str = "Front matter";

/// The navigation list: chapters nested under the heading above them that has a lower level,
/// so a book with parts and chapters shows them as a tree.
fn nav_list(documents: &[(String, String, String)], levels: &[u8]) -> String {
    let mut out = String::new();
    // The levels of the lists currently open, innermost last.
    let mut open: Vec<u8> = Vec::new();
    for (index, (name, title, _)) in documents.iter().enumerate() {
        let level = levels.get(index).copied().unwrap_or(1);
        while let Some(&top) = open.last() {
            if level < top {
                out.push_str(
                    "</li>
</ol>
",
                );
                open.pop();
            } else {
                break;
            }
        }
        match open.last() {
            Some(&top) if level > top => {
                out.push_str(
                    "
<ol>
",
                );
                open.push(level);
            }
            Some(_) => out.push_str(
                "</li>
",
            ),
            None => {
                out.push_str(
                    "<ol>
",
                );
                open.push(level);
            }
        }
        out.push_str(&format!(
            "<li><a href=\"{}\">{}</a>",
            escape_attr(name),
            escape(title)
        ));
    }
    while open.pop().is_some() {
        out.push_str(
            "</li>
</ol>
",
        );
    }
    out
}
