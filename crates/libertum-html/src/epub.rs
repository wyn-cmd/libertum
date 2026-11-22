//! EPUB import: the container's XHTML files, in spine order, read with the HTML reader.
//!
//! The spine's files are joined into one body (each wrapped in a `div` that remembers its
//! folder, so relative image paths resolve) and read once. Reading them as one tree lets an
//! AO3 EPUB, whose preface, chapters and afterword may sit in different files, get the same
//! rules as an AO3 HTML download. Images come from the EPUB itself; nothing is fetched.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

use libertum_model::Document;
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;

use crate::HtmlError;
use crate::dom::Dom;
use crate::read::{self, MAX_IMAGE_BYTES};

/// Largest EPUB entry read, bytes.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

/// Reads an EPUB file.
pub fn import_epub(path: impl AsRef<Path>) -> Result<(Document, Vec<String>), HtmlError> {
    let bytes = std::fs::read(path)?;
    import_epub_bytes(&bytes)
}

/// Reads an EPUB from memory.
pub fn import_epub_bytes(bytes: &[u8]) -> Result<(Document, Vec<String>), HtmlError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| HtmlError::Epub(format!("it is not a zip container ({e})")))?;
    let mut entries: HashMap<String, Vec<u8>> = HashMap::new();
    for index in 0..archive.len() {
        let Ok(mut file) = archive.by_index(index) else {
            continue;
        };
        if file.is_dir() || file.size() > MAX_ENTRY_BYTES {
            continue;
        }
        let Ok(name) = file.name().map(|n| n.into_owned()) else {
            continue;
        };
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_ok() {
            entries.insert(name, buf);
        }
    }
    let text = |name: &str| -> Option<String> {
        entries
            .get(name)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    };
    let container = text("META-INF/container.xml")
        .ok_or_else(|| HtmlError::Epub("META-INF/container.xml is missing".into()))?;
    let opf_path = first_attr(&container, "rootfile", "full-path")
        .ok_or_else(|| HtmlError::Epub("the container names no package file".into()))?;
    let opf = text(&opf_path)
        .ok_or_else(|| HtmlError::Epub(format!("the package file {opf_path} is missing")))?;
    let opf_dir = parent_dir(&opf_path);
    let (manifest, spine) = package(&opf);

    let mut warnings = Vec::new();
    let mut joined = String::from("<!DOCTYPE html><html><head></head><body>");
    let mut files = 0;
    for idref in spine {
        let Some((href, media_type)) = manifest.get(&idref) else {
            warnings.push(format!(
                "the spine names \"{idref}\", which the package lacks"
            ));
            continue;
        };
        if !(media_type.contains("html") || media_type.is_empty()) {
            continue;
        }
        let path = join(&opf_dir, &crate::util::percent_decode(href));
        let Some(source) = text(&path) else {
            warnings.push(format!("the chapter file {path} is missing"));
            continue;
        };
        let body = body_inner(&source);
        joined.push_str("<div data-libertum-base=\"");
        crate::util::escape_attr(&parent_dir(&path), &mut joined);
        joined.push_str("\">");
        joined.push_str(body);
        joined.push_str("</div>");
        files += 1;
    }
    joined.push_str("</body></html>");
    if files == 0 {
        return Err(HtmlError::Epub("the book has no readable chapters".into()));
    }
    let dom = Dom::parse(&joined);
    let resolve = |src: &str| -> Result<Vec<u8>, String> {
        let bytes = entries
            .get(src)
            .ok_or_else(|| "it is not in the book".to_owned())?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("the file is larger than 32 MiB".to_owned());
        }
        Ok(bytes.clone())
    };
    let result = read::read(&dom, Some(&resolve), true);
    warnings.extend(result.warnings);
    Ok((result.document, warnings))
}

/// The manifest (id to href and media type) and the spine's item ids, in order.
fn package(opf: &str) -> (HashMap<String, (String, String)>, Vec<String>) {
    let mut manifest = HashMap::new();
    let mut spine = Vec::new();
    let mut reader = XmlReader::from_str(opf);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let name = e.local_name();
                let attr = |key: &str| {
                    e.attributes()
                        .flatten()
                        .find_map(|a| (a.key.local_name().as_ref() == key).then(|| attr_value(&a)))
                };
                match name.as_ref() {
                    "item" => {
                        if let (Some(id), Some(href)) = (attr("id"), attr("href")) {
                            manifest.insert(id, (href, attr("media-type").unwrap_or_default()));
                        }
                    }
                    "itemref" => {
                        if let Some(id) = attr("idref") {
                            spine.push(id);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    (manifest, spine)
}

fn first_attr(xml: &str, element: &str, key: &str) -> Option<String> {
    let mut reader = XmlReader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == element => {
                return e
                    .attributes()
                    .flatten()
                    .find_map(|a| (a.key.local_name().as_ref() == key).then(|| attr_value(&a)));
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

fn attr_value(a: &quick_xml::events::attributes::Attribute<'_>) -> String {
    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .map(|v| v.into_owned())
        .unwrap_or_else(|_| a.value.to_string())
}

/// What is inside `<body>`, or the whole text when there is no body tag.
fn body_inner(source: &str) -> &str {
    let lower = source.to_ascii_lowercase();
    let Some(open) = lower.find("<body") else {
        return source;
    };
    let Some(open_end) = lower[open..].find('>') else {
        return source;
    };
    let start = open + open_end + 1;
    let end = lower
        .rfind("</body")
        .filter(|&e| e >= start)
        .unwrap_or(source.len());
    &source[start..end]
}

/// The folder part of a zip path, with a trailing slash (empty at the top).
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..=i].to_owned(),
        None => String::new(),
    }
}

/// Joins `dir` (ending in `/` or empty) and a relative `href`, resolving `.` and `..`.
pub fn join(dir: &str, href: &str) -> String {
    let href = href.split(['#', '?']).next().unwrap_or("");
    let mut parts: Vec<&str> = if href.starts_with('/') {
        Vec::new()
    } else {
        dir.split('/').filter(|p| !p.is_empty()).collect()
    };
    for piece in href.split('/') {
        match piece {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_join_with_parent_steps() {
        assert_eq!(join("OEBPS/text/", "../images/a.png"), "OEBPS/images/a.png");
        assert_eq!(join("", "a.xhtml#x"), "a.xhtml");
        assert_eq!(body_inner("<html><BODY class=x>hi</body></html>"), "hi");
    }
}
