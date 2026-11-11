//! Writing the model as an ODT package (PLAN §6.3).
//!
//! Only `content.xml` and `styles.xml` are rewritten. Every other package part the importer
//! kept (pictures, meta data, settings, custom parts) stays in the package unchanged, and the
//! `mimetype` entry is written first and stored by [`Package::write_odf`].
//!
//! Common paragraph styles come from the model's style sheet. Direct formatting — a
//! paragraph's `ParaFormat`, a run's `CharFormat` — is written as automatic styles, generated
//! once per distinct property set and reused, as PLAN §6.3 asks.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use libertum_formats_common::{colour, units};
use libertum_model::{
    Align, Block, Borders, CharFormat, CrossRef, CrossRefKind, Document, HfKind, HfVariant,
    InlineImage, LineSpacing, ListStyle, Media, Orientation, ParaFormat, Paragraph, Section,
    StyleId, TabStopKind, Table, VerticalAlign,
};

use crate::package::Package;
use crate::style::encode_name;
use crate::xml::{escape_attr, escape_text};
use crate::{MIME_TEXT, ODF_VERSION, OdtError};

/// The automatic graphic style every inline picture uses.
const GRAPHIC_STYLE: &str = "LibertumPicture";

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

const NS: &str = concat!(
    "xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" ",
    "xmlns:style=\"urn:oasis:names:tc:opendocument:xmlns:style:1.0\" ",
    "xmlns:text=\"urn:oasis:names:tc:opendocument:xmlns:text:1.0\" ",
    "xmlns:table=\"urn:oasis:names:tc:opendocument:xmlns:table:1.0\" ",
    "xmlns:fo=\"urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0\" ",
    "xmlns:svg=\"urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0\" ",
    "xmlns:draw=\"urn:oasis:names:tc:opendocument:xmlns:drawing:1.0\" ",
    "xmlns:xlink=\"http://www.w3.org/1999/xlink\""
);

pub(crate) fn build(document: &Document, origin: Option<&Package>) -> Result<Package, OdtError> {
    let mut writer = Writer::new(document, origin);
    let body = writer.write_body(document);
    let content = writer.content_xml(body);
    let styles = writer.styles_xml(document);
    let pictures = std::mem::take(&mut writer.pictures);

    let mut package = match origin {
        Some(origin) => origin.clone(),
        None => Package::default(),
    };
    let dropped = unreferenced_pictures(&package, &pictures);
    for name in &dropped {
        package.remove(name);
    }
    for (path, media) in &pictures {
        package.set(path, media.bytes().to_vec());
    }
    package.set_text("content.xml", content);
    package.set_text("styles.xml", styles);
    package.set_text("mimetype", MIME_TEXT.to_owned());
    if !package.contains("meta.xml") {
        package.set_text("meta.xml", meta_xml());
    }
    if !package.contains("settings.xml") {
        package.set_text("settings.xml", settings_xml());
    }
    ensure_manifest(&mut package);
    sync_picture_entries(&mut package, &pictures, &dropped);
    Ok(package)
}

// ------------------------------------------------------------------ package plumbing

/// Pictures the package's old `content.xml` showed that the new one no longer does (an image
/// deleted in the editor, or a GIF that is now stored as PNG). Pictures the old content did
/// not name are kept: something else (a header, a background) may use them.
fn unreferenced_pictures(package: &Package, pictures: &[(String, Arc<Media>)]) -> Vec<String> {
    let Some(old) = package.text("content.xml") else {
        return Vec::new();
    };
    let kept: HashSet<&str> = pictures.iter().map(|(path, _)| path.as_str()).collect();
    package
        .names()
        .filter(|name| name.starts_with("Pictures/") && !kept.contains(name))
        .filter(|name| old.contains(&format!("xlink:href=\"{name}\"")))
        .map(str::to_owned)
        .collect()
}

/// Lists new pictures in the manifest and removes the entries of dropped ones. A manifest
/// that does not exist yet is written in full by [`ensure_manifest`] from the package.
fn sync_picture_entries(
    package: &mut Package,
    pictures: &[(String, Arc<Media>)],
    dropped: &[String],
) {
    let Some(mut manifest) = package.text("META-INF/manifest.xml") else {
        return;
    };
    for name in dropped {
        let needle = format!("manifest:full-path=\"{}\"", escape_attr(name));
        if let Some(at) = manifest.find(&needle)
            && let Some(start) = manifest[..at].rfind("<manifest:file-entry")
            && let Some(length) = manifest[at..].find('>')
            && manifest[at..at + length].ends_with('/')
        {
            manifest.replace_range(start..at + length + 1, "");
        }
    }
    let mut entries = String::new();
    for (path, media) in pictures {
        if !manifest.contains(&format!("manifest:full-path=\"{}\"", escape_attr(path))) {
            entries.push_str(&format!(
                "<manifest:file-entry manifest:full-path=\"{}\" manifest:media-type=\"{}\"/>",
                escape_attr(path),
                media.format().mime()
            ));
        }
    }
    if !entries.is_empty()
        && let Some(at) = manifest.rfind("</manifest:manifest>")
    {
        manifest.insert_str(at, &entries);
    }
    package.set_text("META-INF/manifest.xml", manifest);
}

fn ensure_manifest(package: &mut Package) {
    let needs = match package.text("META-INF/manifest.xml") {
        Some(text) => ["content.xml", "styles.xml", "meta.xml", "settings.xml"]
            .iter()
            .any(|part| !text.contains(&format!("full-path=\"{part}\""))),
        None => true,
    };
    if needs {
        let manifest = manifest_xml(package);
        package.set_text("META-INF/manifest.xml", manifest);
    }
}

fn manifest_xml(package: &Package) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(XML_DECL);
    out.push_str(&format!(
        "<manifest:manifest xmlns:manifest=\"urn:oasis:names:tc:opendocument:xmlns:manifest:1.0\" manifest:version=\"{ODF_VERSION}\">"
    ));
    out.push_str(&format!(
        "<manifest:file-entry manifest:full-path=\"/\" manifest:version=\"{ODF_VERSION}\" manifest:media-type=\"{MIME_TEXT}\"/>"
    ));
    let mut seen = HashSet::new();
    for name in package.names() {
        if name == "mimetype" || name == "META-INF/manifest.xml" {
            continue;
        }
        seen.insert(name);
        out.push_str(&format!(
            "<manifest:file-entry manifest:full-path=\"{}\" manifest:media-type=\"{}\"/>",
            escape_attr(name),
            media_type(name)
        ));
    }
    for name in ["content.xml", "styles.xml", "meta.xml", "settings.xml"] {
        if !seen.contains(name) {
            out.push_str(&format!(
                "<manifest:file-entry manifest:full-path=\"{name}\" manifest:media-type=\"text/xml\"/>"
            ));
        }
    }
    out.push_str("</manifest:manifest>");
    out
}

fn media_type(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".xml") {
        "text/xml"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

fn meta_xml() -> String {
    format!(
        "{XML_DECL}<office:document-meta xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
         xmlns:meta=\"urn:oasis:names:tc:opendocument:xmlns:meta:1.0\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" office:version=\"{ODF_VERSION}\">\
         <office:meta><meta:generator>Libertum</meta:generator></office:meta>\
         </office:document-meta>"
    )
}

fn settings_xml() -> String {
    format!(
        "{XML_DECL}<office:document-settings xmlns:office=\"urn:oasis:names:tc:opendocument:xmlns:office:1.0\" \
         xmlns:config=\"urn:oasis:names:tc:opendocument:xmlns:config:1.0\" office:version=\"{ODF_VERSION}\">\
         <office:settings/></office:document-settings>"
    )
}

// ------------------------------------------------------------------ the writer

struct Writer<'a> {
    /// The package the document was read from, whose pictures are reused.
    origin: Option<&'a Package>,
    /// Pictures written so far: package path and media, in first-use order.
    pictures: Vec<(String, Arc<Media>)>,
    picture_index: HashMap<u64, Vec<usize>>,
    /// ODT name per model paragraph style, in style-id order.
    style_names: Vec<String>,
    auto_para: HashMap<String, String>,
    auto_para_xml: String,
    auto_text: HashMap<String, String>,
    auto_text_xml: String,
    table_styles: HashMap<String, String>,
    table_xml: String,
    used_lists: Vec<ListStyle>,
    next_para: u32,
    next_text: u32,
    next_table: u32,
    next_column: u32,
    next_cell: u32,
    next_row: u32,
    section: usize,
    written: usize,
    /// Note numbers, in document order.
    note_number: u32,
    endnote_number: u32,
    /// List groups already started, so a later run continues their numbering.
    groups_written: HashSet<u64>,
}

impl<'a> Writer<'a> {
    fn new(document: &Document, origin: Option<&'a Package>) -> Self {
        let mut used = HashSet::new();
        let mut style_names = Vec::with_capacity(document.styles.paragraph.len());
        for (index, style) in document.styles.paragraph.iter().enumerate() {
            let id = StyleId(index as u32);
            let base = match id {
                StyleId::BODY => "Standard".to_owned(),
                StyleId::TITLE => "Title".to_owned(),
                StyleId::HEADING_1 => "Heading_20_1".to_owned(),
                StyleId::HEADING_2 => "Heading_20_2".to_owned(),
                StyleId::HEADING_3 => "Heading_20_3".to_owned(),
                StyleId::QUOTE => "Quote".to_owned(),
                _ => encode_name(&style.name, &format!("LibertumStyle{index}")),
            };
            let mut name = base;
            let mut suffix = 2;
            while used.contains(&name) {
                name = format!(
                    "{}_{suffix}",
                    encode_name(&style.name, &format!("Style{index}"))
                );
                suffix += 1;
            }
            used.insert(name.clone());
            style_names.push(name);
        }
        Self {
            origin,
            pictures: Vec::new(),
            picture_index: HashMap::new(),
            style_names,
            auto_para: HashMap::new(),
            auto_para_xml: String::new(),
            auto_text: HashMap::new(),
            auto_text_xml: String::new(),
            table_styles: HashMap::new(),
            table_xml: String::new(),
            used_lists: Vec::new(),
            next_para: 1,
            next_text: 1,
            next_table: 1,
            next_column: 1,
            next_cell: 1,
            next_row: 1,
            section: 0,
            written: 0,
            note_number: 0,
            endnote_number: 0,
            groups_written: HashSet::new(),
        }
    }

    fn common_name(&self, id: StyleId) -> String {
        self.style_names
            .get(id.0 as usize)
            .cloned()
            .unwrap_or_else(|| "Standard".to_owned())
    }

    // -------------------------------------------------------------- body

    fn write_body(&mut self, document: &Document) -> String {
        let blocks: Vec<&Block> = document.body().iter().collect();
        let mut out = String::with_capacity(document.body().len() * 160);
        let mut index = 0usize;
        while index < blocks.len() {
            match blocks[index] {
                Block::Paragraph(paragraph) => match paragraph.props.list {
                    Some(item) => {
                        let mut run: Vec<&Paragraph> = Vec::new();
                        let mut at = index;
                        while let Some(Block::Paragraph(next)) = blocks.get(at) {
                            match next.props.list {
                                Some(next_item)
                                    if next_item.group == item.group
                                        && next_item.style == item.style =>
                                {
                                    run.push(next);
                                    at += 1;
                                }
                                _ => break,
                            }
                        }
                        self.write_list_run(&run, &mut out, true);
                        index = at;
                    }
                    None => {
                        self.write_paragraph(paragraph, &mut out);
                        index += 1;
                    }
                },
                Block::Table(table) => {
                    self.write_table(table, &mut out);
                    self.written += 1;
                    index += 1;
                }
            }
        }
        out
    }

    fn write_paragraph(&mut self, paragraph: &Paragraph, out: &mut String) {
        let section = if paragraph.props.section_start && self.written > 0 {
            self.section += 1;
            self.section
        } else {
            self.section
        };
        self.written += 1;
        let master = (paragraph.props.section_start && section > 0).then(|| master_name(section));
        self.render_paragraph(paragraph, master.as_deref(), out);
    }

    /// Writes a paragraph whose section is already decided (a table cell's text).
    fn render_paragraph(&mut self, paragraph: &Paragraph, master: Option<&str>, out: &mut String) {
        let style = self.paragraph_style(paragraph, master);
        let heading = paragraph.props.outline_level;
        match heading {
            Some(level) => {
                out.push_str(&format!(
                    "<text:h text:style-name=\"{}\" text:outline-level=\"{}\">",
                    escape_attr(&style),
                    level.clamp(1, 9)
                ));
            }
            None => out.push_str(&format!(
                "<text:p text:style-name=\"{}\">",
                escape_attr(&style)
            )),
        }
        self.write_runs(paragraph, out);
        for note in paragraph.notes() {
            let (class, number) = match note.kind {
                libertum_model::NoteKind::Footnote => {
                    self.note_number += 1;
                    ("footnote", self.note_number)
                }
                libertum_model::NoteKind::Endnote => {
                    self.endnote_number += 1;
                    ("endnote", self.endnote_number)
                }
            };
            out.push_str(&format!(
                "<text:note text:note-class=\"{class}\" text:id=\"ftn{number}\">\
                 <text:note-citation>{number}</text:note-citation>\
                 <text:note-body><text:p>{}</text:p></text:note-body></text:note>",
                escape_text(&note.text)
            ));
        }
        out.push_str(if heading.is_some() {
            "</text:h>"
        } else {
            "</text:p>"
        });
    }

    fn write_runs(&mut self, paragraph: &Paragraph, out: &mut String) {
        let text = paragraph.text();
        let clamp = |offset: usize| {
            let mut offset = offset.min(text.len());
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            offset
        };
        // Zero-width anchors (bookmarks, then pictures), in text order.
        let mut anchors: Vec<(usize, Anchor<'_>)> = Vec::new();
        for bookmark in paragraph.bookmarks() {
            anchors.push((clamp(bookmark.offset), Anchor::Bookmark(&bookmark.name)));
        }
        for image in paragraph.images() {
            anchors.push((clamp(image.offset), Anchor::Image(image)));
        }
        anchors.sort_by_key(|(offset, _)| *offset);
        let refs: Vec<(Range<usize>, &CrossRef)> = paragraph
            .cross_refs()
            .iter()
            .map(|cross_ref| {
                (
                    clamp(cross_ref.range.start)..clamp(cross_ref.range.end),
                    cross_ref,
                )
            })
            .filter(|(range, _)| range.start < range.end)
            .collect();
        // Every offset where the text has to be cut: an anchor, or either end of a reference.
        let mut cuts: Vec<usize> = anchors.iter().map(|(offset, _)| *offset).collect();
        for (range, _) in &refs {
            cuts.push(range.start);
            cuts.push(range.end);
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut next_anchor = 0usize;
        let mut position = 0usize;
        let mut pieces: Vec<(usize, usize, Option<String>)> = Vec::new();
        for run in paragraph.runs() {
            let start = clamp(run.range.start);
            let end = clamp(run.range.end).max(start);
            if start > position {
                pieces.push((position, start, None));
            }
            if end > start {
                let style = self.text_style(&run.format);
                pieces.push((start, end, style));
            }
            position = position.max(end);
        }
        if position < text.len() {
            pieces.push((position, text.len(), None));
        }
        for (start, end, style) in pieces {
            self.emit_anchors(out, &anchors, &mut next_anchor, start);
            let mut at = start;
            let first_cut = cuts.partition_point(|cut| *cut <= start);
            for cut in cuts[first_cut..]
                .iter()
                .copied()
                .take_while(|cut| *cut < end)
            {
                write_segment(out, text, at, cut, style.as_deref(), &refs);
                self.emit_anchors(out, &anchors, &mut next_anchor, cut);
                at = cut;
            }
            write_segment(out, text, at, end, style.as_deref(), &refs);
        }
        self.emit_anchors(out, &anchors, &mut next_anchor, text.len());
    }

    /// Writes every anchor at or before `upto` that has not been written yet.
    fn emit_anchors(
        &mut self,
        out: &mut String,
        anchors: &[(usize, Anchor<'_>)],
        next: &mut usize,
        upto: usize,
    ) {
        while let Some((offset, anchor)) = anchors.get(*next) {
            if *offset > upto {
                break;
            }
            match anchor {
                Anchor::Bookmark(name) => out.push_str(&format!(
                    "<text:bookmark text:name=\"{}\"/>",
                    escape_attr(name)
                )),
                Anchor::Image(image) => self.write_image(image, out),
            }
            *next += 1;
        }
    }

    /// Writes a picture as a `draw:frame` anchored as a character, and records its bytes for
    /// the package.
    fn write_image(&mut self, image: &InlineImage, out: &mut String) {
        let path = self.picture_path(&image.media);
        let name = if image.name.is_empty() {
            String::new()
        } else {
            format!(" draw:name=\"{}\"", escape_attr(&image.name))
        };
        out.push_str(&format!(
            "<draw:frame draw:style-name=\"{GRAPHIC_STYLE}\"{name} text:anchor-type=\"as-char\" \
             svg:width=\"{}\" svg:height=\"{}\">\
             <draw:image xlink:href=\"{}\" xlink:type=\"simple\" xlink:show=\"embed\" \
             xlink:actuate=\"onLoad\"/>",
            units::format(image.width_pt),
            units::format(image.height_pt),
            escape_attr(&path)
        ));
        if !image.alt.is_empty() {
            out.push_str(&format!("<svg:desc>{}</svg:desc>", escape_text(&image.alt)));
        }
        out.push_str("</draw:frame>");
    }

    /// The package path of a picture: the entry the package already holds for these bytes, or
    /// a new `Pictures/` entry named after the bytes' hash. One entry per distinct picture.
    fn picture_path(&mut self, media: &Arc<Media>) -> String {
        if let Some(candidates) = self.picture_index.get(&media.hash()) {
            for index in candidates {
                let (path, known) = &self.pictures[*index];
                if Arc::ptr_eq(known, media) || known.bytes() == media.bytes() {
                    return path.clone();
                }
            }
        }
        let existing = self.origin.and_then(|package| {
            package
                .names()
                .filter(|name| name.starts_with("Pictures/"))
                .find(|name| package.get(name) == Some(media.bytes()))
                .map(str::to_owned)
        });
        let path = existing.unwrap_or_else(|| {
            let extension = media.format().extension();
            let mut path = format!("Pictures/{:016x}.{extension}", media.hash());
            // Two different pictures with one hash would need distinct names.
            let mut suffix = 2;
            while self.pictures.iter().any(|(taken, _)| *taken == path)
                || self.origin.is_some_and(|package| package.contains(&path))
            {
                path = format!("Pictures/{:016x}_{suffix}.{extension}", media.hash());
                suffix += 1;
            }
            path
        });
        self.picture_index
            .entry(media.hash())
            .or_default()
            .push(self.pictures.len());
        self.pictures.push((path.clone(), Arc::clone(media)));
        path
    }

    fn paragraph_style(&mut self, paragraph: &Paragraph, master: Option<&str>) -> String {
        let props =
            paragraph_properties(&paragraph.props.format, paragraph.props.page_break_before);
        let master_attr = master
            .map(|master| format!(" style:master-page-name=\"{}\"", escape_attr(master)))
            .unwrap_or_default();
        if props.is_empty() && master_attr.is_empty() {
            return self.common_name(paragraph.props.style);
        }
        let key = format!("{props}|{master_attr}");
        if let Some(name) = self.auto_para.get(&key) {
            return name.clone();
        }
        let name = format!("P{}", self.next_para);
        self.next_para += 1;
        self.auto_para_xml.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{}\"{master_attr}>",
            escape_attr(&self.common_name(paragraph.props.style))
        ));
        self.auto_para_xml.push_str(&props);
        self.auto_para_xml.push_str("</style:style>");
        self.auto_para.insert(key, name.clone());
        name
    }

    fn text_style(&mut self, format: &CharFormat) -> Option<String> {
        if format.is_empty() {
            return None;
        }
        let props = text_props_xml(format);
        if props.is_empty() {
            return None;
        }
        if let Some(name) = self.auto_text.get(&props) {
            return Some(name.clone());
        }
        let name = format!("T{}", self.next_text);
        self.next_text += 1;
        self.auto_text_xml.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"text\"><style:text-properties{props}/></style:style>"
        ));
        self.auto_text.insert(props, name.clone());
        Some(name)
    }

    // -------------------------------------------------------------- lists

    fn write_list_run(&mut self, run: &[&Paragraph], out: &mut String, body: bool) {
        let Some(first) = run.first() else {
            return;
        };
        let Some(item) = first.props.list else {
            return;
        };
        if !self.used_lists.contains(&item.style) {
            self.used_lists.push(item.style);
        }
        let continued = !self.groups_written.insert(item.group);
        let level = build_level(run, 0);
        self.write_level(&level, item.style, out, body, continued, 0);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_level(
        &mut self,
        level: &ListLevel<'_>,
        style: ListStyle,
        out: &mut String,
        body: bool,
        continued: bool,
        depth: u8,
    ) {
        out.push_str(&format!(
            "<text:list text:style-name=\"{}\"{}>",
            list_name(style),
            if continued && depth == 0 {
                " text:continue-numbering=\"true\""
            } else {
                ""
            }
        ));
        for item in &level.items {
            out.push_str("<text:list-item>");
            if let Some(paragraph) = item.paragraph {
                if body {
                    self.write_paragraph(paragraph, out);
                } else {
                    self.render_paragraph(paragraph, None, out);
                }
            }
            if let Some(children) = &item.children {
                self.write_level(children, style, out, body, false, depth + 1);
            }
            out.push_str("</text:list-item>");
        }
        out.push_str("</text:list>");
    }

    fn list_style_definitions(&self) -> String {
        let mut out = String::new();
        for style in &self.used_lists {
            out.push_str(&format!(
                "<text:list-style style:name=\"{}\">",
                list_name(*style)
            ));
            for level in 0..9u8 {
                let space_before =
                    units::format(libertum_model::LIST_INDENT_PT * f32::from(level + 1));
                let min_label = units::format(libertum_model::LIST_HANGING_PT);
                let properties = format!(
                    "<style:list-level-properties text:space-before=\"{space_before}\" text:min-label-width=\"{min_label}\"/>"
                );
                match list_level(*style, level) {
                    ListLevelKind::Bullet(glyph) => out.push_str(&format!(
                        "<text:list-level-style-bullet text:level=\"{}\" text:bullet-char=\"{}\">{properties}</text:list-level-style-bullet>",
                        level + 1,
                        escape_attr(glyph)
                    )),
                    ListLevelKind::Number {
                        format,
                        prefix,
                        suffix,
                    } => {
                        let prefix = prefix
                            .map(|prefix| format!(" style:num-prefix=\"{}\"", escape_attr(prefix)))
                            .unwrap_or_default();
                        out.push_str(&format!(
                            "<text:list-level-style-number text:level=\"{}\" style:num-format=\"{}\"{prefix} style:num-suffix=\"{}\">{properties}</text:list-level-style-number>",
                            level + 1,
                            escape_attr(format),
                            escape_attr(suffix)
                        ));
                    }
                }
            }
            out.push_str("</text:list-style>");
        }
        out
    }

    // -------------------------------------------------------------- tables

    fn write_table(&mut self, table: &Table, out: &mut String) {
        let table_style = self.table_style(table);
        let template = table
            .style
            .as_ref()
            .filter(|style| !style.name.is_empty())
            .map(|style| format!(" table:template-name=\"{}\"", escape_attr(&style.name)))
            .unwrap_or_default();
        out.push_str(&format!(
            "<table:table table:name=\"Table{}\" table:style-name=\"{}\"{template}>",
            table.id.0, table_style
        ));
        for width in &table.widths_pt {
            let column = self.column_style(*width);
            out.push_str(&format!(
                "<table:table-column table:style-name=\"{column}\"/>"
            ));
        }
        let header = usize::from(table.header_rows).min(table.rows.len());
        if header > 0 {
            out.push_str("<table:table-header-rows>");
            for row in 0..header {
                self.write_row(table, row, out);
            }
            out.push_str("</table:table-header-rows>");
        }
        for row in header..table.rows.len() {
            self.write_row(table, row, out);
        }
        out.push_str("</table:table>");
    }

    fn write_row(&mut self, table: &Table, row: usize, out: &mut String) {
        let Some(table_row) = table.rows.get(row) else {
            return;
        };
        out.push_str("<table:table-row");
        if table_row.min_height_pt > 0.0 {
            let style = self.row_style(table_row.min_height_pt);
            out.push_str(&format!(" table:style-name=\"{style}\""));
        }
        out.push('>');
        for (cell_index, cell) in table_row.cells.iter().enumerate() {
            let span = usize::from(cell.col_span.max(1));
            let span_attr = if span > 1 {
                format!(" table:number-columns-spanned=\"{span}\"")
            } else {
                String::new()
            };
            if cell.merged_into_above {
                out.push_str(&format!("<table:covered-table-cell{span_attr}/>"));
                continue;
            }
            let vertical = vertical_span(table, row, cell_index);
            let vertical_attr = if vertical > 1 {
                format!(" table:number-rows-spanned=\"{vertical}\"")
            } else {
                String::new()
            };
            // The resolved fill, so the table looks right in programs that ignore the
            // style name written on the table.
            let style = self.cell_style(table, table.resolved_fill(row, cell));
            out.push_str(&format!(
                "<table:table-cell table:style-name=\"{style}\"{span_attr}{vertical_attr}>"
            ));
            self.write_cell_paragraphs(cell.paragraphs.as_slice(), out);
            out.push_str("</table:table-cell>");
        }
        out.push_str("</table:table-row>");
    }

    fn write_cell_paragraphs(&mut self, paragraphs: &[Paragraph], out: &mut String) {
        let mut index = 0usize;
        while index < paragraphs.len() {
            match paragraphs[index].props.list {
                Some(item) => {
                    let mut run: Vec<&Paragraph> = Vec::new();
                    let mut at = index;
                    while let Some(paragraph) = paragraphs.get(at) {
                        match paragraph.props.list {
                            Some(next) if next.group == item.group && next.style == item.style => {
                                run.push(paragraph);
                                at += 1;
                            }
                            _ => break,
                        }
                    }
                    self.write_list_run(&run, out, false);
                    index = at;
                }
                None => {
                    self.render_paragraph(&paragraphs[index], None, out);
                    index += 1;
                }
            }
        }
    }

    fn table_style(&mut self, table: &Table) -> String {
        let key = format!(
            "{}|{}|{}|{}",
            table.borders.width_pt,
            table
                .borders
                .colour
                .map(|value| value.to_string())
                .join(","),
            table.borders.outer,
            table.borders.inner
        );
        if let Some(name) = self.table_styles.get(&key) {
            return name.clone();
        }
        let name = format!("Table{}", self.next_table);
        self.next_table += 1;
        // ODF draws table lines per cell; the table properties only choose the collapsing
        // model. Libertum's one outer/inner line is written on every cell style below.
        self.table_xml.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"table\"><style:table-properties table:border-model=\"collapsing\"/></style:style>"
        ));
        self.table_styles.insert(key, name.clone());
        name
    }

    fn cell_style(&mut self, table: &Table, fill: Option<[u8; 4]>) -> String {
        let key = format!(
            "cell|{}|{}|{}|{}|{}",
            table.borders.width_pt,
            table
                .borders
                .colour
                .map(|value| value.to_string())
                .join(","),
            table.borders.outer,
            table.borders.inner,
            fill.map(colour::hex).unwrap_or_default()
        );
        if let Some(name) = self.table_styles.get(&key) {
            return name.clone();
        }
        let name = format!("TCell{}", self.next_cell);
        self.next_cell += 1;
        let mut props = String::new();
        if let Some(fill) = fill {
            props.push_str(&format!(" fo:background-color=\"{}\"", colour::hex(fill)));
        }
        if table.borders.is_visible() {
            props.push_str(&format!(
                " fo:border=\"{}pt solid {}\"",
                trim_number(table.borders.width_pt),
                colour::hex(table.borders.colour)
            ));
        } else {
            props.push_str(" fo:border=\"none\"");
        }
        self.table_xml.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"table-cell\"><style:table-cell-properties{props}/></style:style>"
        ));
        self.table_styles.insert(key, name.clone());
        name
    }

    fn column_style(&mut self, width_pt: f32) -> String {
        let key = format!("column|{width_pt}");
        if let Some(name) = self.table_styles.get(&key) {
            return name.clone();
        }
        let name = format!("TCol{}", self.next_column);
        self.next_column += 1;
        self.table_xml.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"table-column\"><style:table-column-properties style:column-width=\"{}\"/></style:style>",
            units::format(width_pt.max(libertum_model::MIN_TABLE_COLUMN_PT))
        ));
        self.table_styles.insert(key, name.clone());
        name
    }

    fn row_style(&mut self, min_height_pt: f32) -> String {
        let key = format!("row|{min_height_pt}");
        if let Some(name) = self.table_styles.get(&key) {
            return name.clone();
        }
        let name = format!("TRow{}", self.next_row);
        self.next_row += 1;
        self.table_xml.push_str(&format!(
            "<style:style style:name=\"{name}\" style:family=\"table-row\"><style:table-row-properties style:row-height=\"{}\" style:use-optimal-row-height=\"false\"/></style:style>",
            units::format(min_height_pt.max(0.0))
        ));
        self.table_styles.insert(key, name.clone());
        name
    }

    // -------------------------------------------------------------- parts

    fn content_xml(&self, body: String) -> String {
        let mut out = String::with_capacity(body.len() + 4096);
        out.push_str(XML_DECL);
        out.push_str(&format!(
            "<office:document-content {NS} office:version=\"{ODF_VERSION}\">"
        ));
        out.push_str("<office:automatic-styles>");
        out.push_str(&self.auto_para_xml);
        out.push_str(&self.auto_text_xml);
        out.push_str(&self.table_xml);
        if !self.pictures.is_empty() {
            out.push_str(&format!(
                "<style:style style:name=\"{GRAPHIC_STYLE}\" style:family=\"graphic\">\n                 <style:graphic-properties text:anchor-type=\"as-char\" style:wrap=\"none\" \n                 style:vertical-pos=\"top\" style:vertical-rel=\"baseline\" \n                 fo:margin-top=\"0pt\" fo:margin-bottom=\"0pt\" fo:border=\"none\"/>\n                 </style:style>"
            ));
        }
        out.push_str(&self.list_style_definitions());
        out.push_str("</office:automatic-styles>");
        out.push_str("<office:body><office:text>");
        out.push_str(&body);
        out.push_str("</office:text></office:body></office:document-content>");
        out
    }

    fn styles_xml(&self, document: &Document) -> String {
        let mut out = String::with_capacity(8192);
        out.push_str(XML_DECL);
        out.push_str(&format!(
            "<office:document-styles {NS} office:version=\"{ODF_VERSION}\">"
        ));
        out.push_str("<office:styles>");
        out.push_str(&self.default_style(document));
        for id in 0..document.styles.paragraph.len() {
            out.push_str(&self.common_style(document, StyleId(id as u32)));
        }
        // Styles used by header, footer and page-number paragraphs. The alignment variants
        // live in the automatic styles below.
        for (name, parent) in [
            ("Header", "Standard"),
            ("Footer", "Standard"),
            ("Page_20_Number", "Standard"),
        ] {
            out.push_str(&format!(
                "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\" style:class=\"extra\"/>"
            ));
        }
        out.push_str("</office:styles>");

        out.push_str("<office:automatic-styles>");
        for (name, parent, align) in [
            ("HFHeaderCentre", "Header", "center"),
            ("HFHeaderRight", "Header", "end"),
            ("HFFooterCentre", "Footer", "center"),
            ("HFFooterRight", "Footer", "end"),
            ("HFPageNumberCentre", "Page_20_Number", "center"),
            ("HFPageNumberRight", "Page_20_Number", "end"),
        ] {
            out.push_str(&format!(
                "<style:style style:name=\"{name}\" style:family=\"paragraph\" style:parent-style-name=\"{parent}\"><style:paragraph-properties fo:text-align=\"{align}\"/></style:style>"
            ));
        }
        let sections = self.section_count(document);
        for index in 0..sections {
            out.push_str(&self.page_layout(document, index));
        }
        out.push_str("</office:automatic-styles>");

        out.push_str("<office:master-styles>");
        for index in 0..sections {
            out.push_str(&self.master_page(document, index));
        }
        out.push_str("</office:master-styles></office:document-styles>");
        out
    }

    fn section_count(&self, document: &Document) -> usize {
        (self.section + 1).max(document.sections.len()).max(1)
    }

    fn section(&self, document: &Document, index: usize) -> Section {
        document
            .sections
            .get(index)
            .or_else(|| document.sections.first())
            .cloned()
            .unwrap_or_default()
    }

    fn default_style(&self, document: &Document) -> String {
        let style = &document.styles;
        let char = CharFormat {
            family: Some(style.defaults.family.clone()),
            size_pt: Some(style.defaults.size_pt),
            weight: Some(style.defaults.weight),
            italic: Some(style.defaults.italic),
            underline: Some(style.defaults.underline),
            strikethrough: Some(style.defaults.strikethrough),
            colour: Some(style.defaults.colour),
            highlight: style.defaults.highlight.map(Some),
            vertical: Some(style.defaults.vertical),
            letter_spacing_pt: Some(style.defaults.letter_spacing_pt),
            ligatures: None,
        };
        let defaults = &style.para_defaults;
        let para = ParaFormat {
            align: Some(defaults.align),
            indent_left_pt: Some(defaults.indent_left_pt),
            indent_right_pt: Some(defaults.indent_right_pt),
            indent_first_pt: Some(defaults.indent_first_pt),
            space_before_pt: Some(defaults.space_before_pt),
            space_after_pt: Some(defaults.space_after_pt),
            line_spacing: Some(defaults.line_spacing),
            keep_with_next: Some(defaults.keep_with_next),
            keep_together: Some(defaults.keep_together),
            widow_control: Some(defaults.widow_control),
            hyphenate: Some(defaults.hyphenate),
            borders: None,
            tab_stops: defaults.tab_stops,
        };
        let para = paragraph_properties(&para, false);
        let char = text_props_xml(&char);
        let char = if char.is_empty() {
            String::new()
        } else {
            format!("<style:text-properties{char}/>")
        };
        format!(
            "<style:default-style style:family=\"paragraph\">{para}{char}</style:default-style>"
        )
    }

    fn common_style(&self, document: &Document, id: StyleId) -> String {
        let Some(style) = document.styles.get(id) else {
            return String::new();
        };
        let name = self.common_name(id);
        let display = escape_attr(&style.name);
        let parent = style
            .base
            .map(|base| {
                format!(
                    " style:parent-style-name=\"{}\"",
                    escape_attr(&self.common_name(base))
                )
            })
            .unwrap_or_default();
        let next = style
            .next
            .map(|next| {
                format!(
                    " style:next-style-name=\"{}\"",
                    escape_attr(&self.common_name(next))
                )
            })
            .unwrap_or_default();
        let outline = style
            .outline_level
            .map(|level| format!(" style:default-outline-level=\"{level}\""))
            .unwrap_or_default();
        let para = paragraph_properties(&style.para, false);
        let char = text_props_xml(&style.char);
        let char = if char.is_empty() {
            String::new()
        } else {
            format!("<style:text-properties{char}/>")
        };
        let mut out = format!(
            "<style:style style:name=\"{}\" style:display-name=\"{display}\" style:family=\"paragraph\" style:class=\"text\"{parent}{next}{outline}>",
            escape_attr(&name)
        );
        out.push_str(&para);
        out.push_str(&char);
        out.push_str("</style:style>");
        out
    }

    fn page_layout(&self, document: &Document, index: usize) -> String {
        let section = self.section(document, index);
        let page = section.page;
        let mut out = format!("<style:page-layout style:name=\"{}\">", layout_name(index));
        let mut props = format!(
            " fo:page-width=\"{}\" fo:page-height=\"{}\" style:print-orientation=\"{}\"",
            units::format(page.width_pt()),
            units::format(page.height_pt()),
            if page.orientation == Orientation::Landscape {
                "landscape"
            } else {
                "portrait"
            }
        );
        props.push_str(&format!(
            " fo:margin-top=\"{}\" fo:margin-right=\"{}\" fo:margin-bottom=\"{}\" fo:margin-left=\"{}\"",
            units::format(page.margins.top),
            units::format(page.margins.right),
            units::format(page.margins.bottom),
            units::format(page.margins.left)
        ));
        if let Some(number) = section.first_page_number {
            props.push_str(&format!(" style:first-page-number=\"{number}\""));
        }
        out.push_str(&format!("<style:page-layout-properties{props}>"));
        if section.columns > 1 {
            out.push_str(&format!(
                "<style:columns fo:column-count=\"{}\" fo:column-gap=\"{}\"/>",
                section.columns,
                units::format(section.column_gutter_pt)
            ));
        }
        out.push_str("</style:page-layout-properties>");
        out.push_str(&format!(
            "<style:header-style><style:header-footer-properties fo:margin-top=\"{}\"/></style:header-style>",
            units::format(section.header_distance_pt)
        ));
        out.push_str(&format!(
            "<style:footer-style><style:header-footer-properties fo:margin-bottom=\"{}\"/></style:footer-style>",
            units::format(section.footer_distance_pt)
        ));
        out.push_str("</style:page-layout>");
        out
    }

    fn master_page(&self, document: &Document, index: usize) -> String {
        let section = self.section(document, index);
        let mut out = format!(
            "<style:master-page style:name=\"{}\" style:page-layout-name=\"{}\">",
            master_name(index),
            layout_name(index)
        );
        if let Some(block) = self.hf_block(&section, HfKind::Header, HfVariant::Default) {
            out.push_str(&format!("<style:header>{block}</style:header>"));
        }
        if let Some(block) = self.hf_block(&section, HfKind::Header, HfVariant::First) {
            out.push_str(&format!("<style:header-first>{block}</style:header-first>"));
        }
        if let Some(block) = self.hf_block(&section, HfKind::Header, HfVariant::Even) {
            out.push_str(&format!("<style:header-left>{block}</style:header-left>"));
        }
        if let Some(block) = self.hf_block(&section, HfKind::Footer, HfVariant::Default) {
            out.push_str(&format!("<style:footer>{block}</style:footer>"));
        }
        if let Some(block) = self.hf_block(&section, HfKind::Footer, HfVariant::First) {
            out.push_str(&format!("<style:footer-first>{block}</style:footer-first>"));
        }
        if let Some(block) = self.hf_block(&section, HfKind::Footer, HfVariant::Even) {
            out.push_str(&format!("<style:footer-left>{block}</style:footer-left>"));
        }
        out.push_str("</style:master-page>");
        out
    }

    /// The paragraphs of one header or footer block, or `None` when the block is absent.
    fn hf_block(&self, section: &Section, kind: HfKind, variant: HfVariant) -> Option<String> {
        let set = section.header_footer(kind);
        let page_number = section.page_numbers.filter(|pos| match kind {
            HfKind::Header => pos.vertical == libertum_model::PageNumberVertical::Top,
            HfKind::Footer => pos.vertical == libertum_model::PageNumberVertical::Bottom,
        });
        let first_text = set.first.as_ref().filter(|text| !text.is_empty());
        let even_text = set.even.as_ref().filter(|text| !text.is_empty());
        // A variant the model switches on but leaves empty falls back to the default text, so
        // no separate ODF block is needed; only real content gets one.
        let active = match variant {
            HfVariant::Default => true,
            HfVariant::First => {
                section.hide_first_page || (set.different_first && first_text.is_some())
            }
            HfVariant::Even => set.different_even_odd && even_text.is_some(),
        };
        if !active {
            return None;
        }
        let text = match variant {
            HfVariant::Default => set.default.as_ref(),
            HfVariant::First if set.different_first && first_text.is_some() => first_text,
            // Page numbers hidden on the first page must not take the header text with them.
            HfVariant::First => set.default.as_ref(),
            HfVariant::Even => even_text,
        }
        .filter(|text| !text.is_empty());
        let include_number = page_number.is_some()
            && match variant {
                HfVariant::Default | HfVariant::Even => true,
                HfVariant::First => !section.hide_first_page,
            };

        let mut out = String::new();
        if let (true, Some(pos)) = (include_number, page_number) {
            let style = match pos.horizontal {
                libertum_model::PageNumberHorizontal::Left => "Page_20_Number",
                libertum_model::PageNumberHorizontal::Centre => "HFPageNumberCentre",
                libertum_model::PageNumberHorizontal::Right => "HFPageNumberRight",
            };
            out.push_str(&format!(
                "<text:p text:style-name=\"{style}\"><text:page-number/></text:p>"
            ));
        }
        if let Some(text) = text {
            let base = match kind {
                HfKind::Header => "Header",
                HfKind::Footer => "Footer",
            };
            let style = match text.align {
                Align::Centre => {
                    if kind == HfKind::Header {
                        "HFHeaderCentre"
                    } else {
                        "HFFooterCentre"
                    }
                }
                Align::End => {
                    if kind == HfKind::Header {
                        "HFHeaderRight"
                    } else {
                        "HFFooterRight"
                    }
                }
                _ => base,
            };
            out.push_str(&format!("<text:p text:style-name=\"{style}\">"));
            write_header_text(&mut out, &text.text);
            out.push_str("</text:p>");
        }
        if out.is_empty() {
            // An empty first-page block suppresses the page number there (`hide_first_page`).
            if variant == HfVariant::First && section.hide_first_page && page_number.is_some() {
                return Some(String::new());
            }
            return None;
        }
        Some(out)
    }
}

// ------------------------------------------------------------------ lists

struct ListLevel<'a> {
    items: Vec<ListItemNode<'a>>,
}

struct ListItemNode<'a> {
    paragraph: Option<&'a Paragraph>,
    children: Option<Box<ListLevel<'a>>>,
}

fn build_level<'a>(items: &[&'a Paragraph], depth: u8) -> ListLevel<'a> {
    let mut level = ListLevel { items: Vec::new() };
    let mut index = 0usize;
    while index < items.len() {
        let Some(item) = items[index].props.list else {
            index += 1;
            continue;
        };
        if usize::from(item.level) < usize::from(depth) {
            index += 1;
            continue;
        }
        if usize::from(item.level) == usize::from(depth) {
            let start = index;
            let mut end = index + 1;
            while end < items.len()
                && items[end]
                    .props
                    .list
                    .is_some_and(|next| usize::from(next.level) > usize::from(depth))
            {
                end += 1;
            }
            let children =
                (end > start + 1).then(|| Box::new(build_level(&items[start + 1..end], depth + 1)));
            level.items.push(ListItemNode {
                paragraph: Some(items[start]),
                children,
            });
            index = end;
        } else {
            // A level was skipped: wrap the deeper run in an item with no text of its own.
            let mut end = index;
            while end < items.len()
                && items[end]
                    .props
                    .list
                    .is_some_and(|next| usize::from(next.level) > usize::from(depth))
            {
                end += 1;
            }
            level.items.push(ListItemNode {
                paragraph: None,
                children: Some(Box::new(build_level(&items[index..end], depth + 1))),
            });
            index = end;
        }
    }
    level
}

fn list_name(style: ListStyle) -> &'static str {
    match style {
        ListStyle::Bullet => "LBullet",
        ListStyle::Numbered => "LNumbered",
        ListStyle::Lettered => "LLettered",
        ListStyle::Harvard => "LHarvard",
    }
}

#[derive(Clone, Copy)]
enum ListLevelKind {
    Bullet(&'static str),
    Number {
        format: &'static str,
        prefix: Option<&'static str>,
        suffix: &'static str,
    },
}

/// The marker a model list style draws at `level`, as ODF list-level attributes.
fn list_level(style: ListStyle, level: u8) -> ListLevelKind {
    let level = usize::from(level % 9);
    match style {
        ListStyle::Bullet => ListLevelKind::Bullet(["•", "◦", "▪"][level % 3]),
        ListStyle::Numbered => match level % 3 {
            0 => ListLevelKind::Number {
                format: "1",
                prefix: None,
                suffix: ".",
            },
            1 => ListLevelKind::Number {
                format: "a",
                prefix: None,
                suffix: ".",
            },
            _ => ListLevelKind::Number {
                format: "i",
                prefix: None,
                suffix: ".",
            },
        },
        ListStyle::Lettered => match level % 3 {
            0 => ListLevelKind::Number {
                format: "A",
                prefix: None,
                suffix: ".",
            },
            1 => ListLevelKind::Number {
                format: "a",
                prefix: None,
                suffix: ".",
            },
            _ => ListLevelKind::Number {
                format: "i",
                prefix: None,
                suffix: ".",
            },
        },
        ListStyle::Harvard => [
            ListLevelKind::Number {
                format: "I",
                prefix: None,
                suffix: ".",
            },
            ListLevelKind::Number {
                format: "A",
                prefix: None,
                suffix: ".",
            },
            ListLevelKind::Number {
                format: "1",
                prefix: None,
                suffix: ".",
            },
            ListLevelKind::Number {
                format: "a",
                prefix: None,
                suffix: ".",
            },
            ListLevelKind::Number {
                format: "1",
                prefix: Some("("),
                suffix: ")",
            },
            ListLevelKind::Number {
                format: "a",
                prefix: Some("("),
                suffix: ")",
            },
            ListLevelKind::Number {
                format: "i",
                prefix: None,
                suffix: ".",
            },
        ][level % 7],
    }
}

// ------------------------------------------------------------------ text

fn write_header_text(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(at) = find_token(rest) {
        let (before, token, after) = match at {
            Token::Page(at) => (&rest[..at], "{page}", &rest[at + 6..]),
            Token::Pages(at) => (&rest[..at], "{pages}", &rest[at + 7..]),
        };
        write_text_strict(out, before);
        out.push_str(if token == "{page}" {
            "<text:page-number/>"
        } else {
            "<text:page-count/>"
        });
        rest = after;
    }
    write_text_strict(out, rest);
}

enum Token {
    Page(usize),
    Pages(usize),
}

fn find_token(text: &str) -> Option<Token> {
    let page = text.find("{page}");
    let pages = text.find("{pages}");
    match (page, pages) {
        (Some(page), Some(pages)) => Some(if page <= pages {
            Token::Page(page)
        } else {
            Token::Pages(pages)
        }),
        (Some(page), None) => Some(Token::Page(page)),
        (None, Some(pages)) => Some(Token::Pages(pages)),
        (None, None) => None,
    }
}

/// Writes body text. A single space stays literal (the common case, and much smaller than
/// one `text:s` element per word); a run of several spaces becomes one literal space plus a
/// `text:s` for the rest, and spaces at the very start or end of the paragraph are always
/// explicit because ODF ignores literal ones there.
fn write_text(out: &mut String, text: &str, first: bool, last: bool) {
    let mut text = text;
    if first {
        let leading = text.len() - text.trim_start_matches(' ').len();
        if leading > 0 {
            write_spaces(out, leading);
            text = &text[leading..];
        }
    }
    let trailing = if last {
        text.len() - text.trim_end_matches(' ').len()
    } else {
        0
    };
    let body = if trailing > 0 {
        &text[..text.len() - trailing]
    } else {
        text
    };

    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut String| {
        if !plain.is_empty() {
            out.push_str(&escape_text(plain));
            plain.clear();
        }
    };
    let mut chars = body.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            ' ' => {
                let mut count = 1usize;
                while chars.peek() == Some(&' ') {
                    chars.next();
                    count += 1;
                }
                if count == 1 {
                    plain.push(' ');
                } else {
                    flush(&mut plain, out);
                    out.push(' ');
                    write_spaces(out, count - 1);
                }
            }
            '\t' => {
                flush(&mut plain, out);
                out.push_str("<text:tab/>");
            }
            '\n' => {
                flush(&mut plain, out);
                out.push_str("<text:line-break/>");
            }
            other => plain.push(other),
        }
    }
    flush(&mut plain, out);
    if trailing > 0 {
        write_spaces(out, trailing);
    }
}

fn write_spaces(out: &mut String, count: usize) {
    if count == 1 {
        out.push_str("<text:s/>");
    } else if count > 1 {
        out.push_str(&format!("<text:s text:c=\"{count}\"/>"));
    }
}

/// Header and footer text is short, so every space is explicit; that also keeps a space next
/// to a `{page}` field unambiguous.
fn write_text_strict(out: &mut String, text: &str) {
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut String| {
        if !plain.is_empty() {
            out.push_str(&escape_text(plain));
            plain.clear();
        }
    };
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            ' ' => {
                flush(&mut plain, out);
                let mut count = 1usize;
                while chars.peek() == Some(&' ') {
                    chars.next();
                    count += 1;
                }
                write_spaces(out, count);
            }
            '\t' => {
                flush(&mut plain, out);
                out.push_str("<text:tab/>");
            }
            '\n' => {
                flush(&mut plain, out);
                out.push_str("<text:line-break/>");
            }
            other => plain.push(other),
        }
    }
    flush(&mut plain, out);
}

/// A zero-width thing anchored at a byte offset of a paragraph.
enum Anchor<'a> {
    Bookmark(&'a str),
    Image(&'a InlineImage),
}

/// Writes `text[start..end]`, inside a `text:bookmark-ref` when a cross-reference covers it.
/// The span for direct formatting stays outside the reference, which is how LibreOffice
/// styles one; the reader joins the pieces of a split reference back together.
fn write_segment(
    out: &mut String,
    text: &str,
    start: usize,
    end: usize,
    style: Option<&str>,
    refs: &[(Range<usize>, &CrossRef)],
) {
    if start >= end {
        return;
    }
    let (first, last) = (start == 0, end == text.len());
    let covering = refs
        .iter()
        .find(|(range, _)| range.start <= start && end <= range.end);
    let Some((_, cross_ref)) = covering else {
        write_text_segment(out, &text[start..end], style, first, last);
        return;
    };
    if let Some(style) = style {
        out.push_str(&format!(
            "<text:span text:style-name=\"{}\">",
            escape_attr(style)
        ));
    }
    let format = match cross_ref.kind {
        CrossRefKind::Text => "text",
        CrossRefKind::Page => "page",
    };
    out.push_str(&format!(
        "<text:bookmark-ref text:reference-format=\"{format}\" text:ref-name=\"{}\">",
        escape_attr(&cross_ref.target)
    ));
    write_text(out, &text[start..end], first, last);
    out.push_str("</text:bookmark-ref>");
    if style.is_some() {
        out.push_str("</text:span>");
    }
}

fn write_text_segment(out: &mut String, text: &str, style: Option<&str>, first: bool, last: bool) {
    if text.is_empty() {
        return;
    }
    match style {
        Some(style) => {
            out.push_str(&format!(
                "<text:span text:style-name=\"{}\">",
                escape_attr(style)
            ));
            write_text(out, text, first, last);
            out.push_str("</text:span>");
        }
        None => write_text(out, text, first, last),
    }
}

// ------------------------------------------------------------------ formatting

/// A paragraph's `style:paragraph-properties` element, or the empty string when it sets
/// nothing.
fn paragraph_properties(format: &ParaFormat, page_break: bool) -> String {
    let mut out = String::new();
    if page_break {
        out.push_str(" fo:break-before=\"page\"");
    }
    if let Some(align) = format.align {
        let value = match align {
            Align::Start => "start",
            Align::Centre => "center",
            Align::End => "end",
            Align::Justify => "justify",
        };
        out.push_str(&format!(" fo:text-align=\"{value}\""));
    }
    if let Some(left) = format.indent_left_pt {
        out.push_str(&format!(" fo:margin-left=\"{}\"", units::format(left)));
    }
    if let Some(right) = format.indent_right_pt {
        out.push_str(&format!(" fo:margin-right=\"{}\"", units::format(right)));
    }
    if let Some(first) = format.indent_first_pt {
        out.push_str(&format!(" fo:text-indent=\"{}\"", units::format(first)));
    }
    if let Some(before) = format.space_before_pt {
        out.push_str(&format!(" fo:margin-top=\"{}\"", units::format(before)));
    }
    if let Some(after) = format.space_after_pt {
        out.push_str(&format!(" fo:margin-bottom=\"{}\"", units::format(after)));
    }
    if let Some(spacing) = format.line_spacing {
        match spacing {
            LineSpacing::Multiple(multiple) => out.push_str(&format!(
                " fo:line-height=\"{}%\"",
                trim_number(multiple * 100.0)
            )),
            LineSpacing::Exactly(points) => {
                out.push_str(&format!(" fo:line-height=\"{}\"", units::format(points)))
            }
            LineSpacing::AtLeast(points) => out.push_str(&format!(
                " style:line-height-at-least=\"{}\"",
                units::format(points)
            )),
            LineSpacing::Leading(points) => out.push_str(&format!(
                " style:line-spacing=\"{}\"",
                units::format(points)
            )),
        }
    }
    if let Some(keep) = format.keep_together {
        out.push_str(&format!(
            " fo:keep-together=\"{}\"",
            if keep { "always" } else { "auto" }
        ));
    }
    if let Some(keep) = format.keep_with_next {
        out.push_str(&format!(
            " fo:keep-with-next=\"{}\"",
            if keep { "always" } else { "auto" }
        ));
    }
    if let Some(widow) = format.widow_control {
        let value = if widow { 2 } else { 0 };
        out.push_str(&format!(" fo:widows=\"{value}\" fo:orphans=\"{value}\""));
    }
    if let Some(hyphenate) = format.hyphenate {
        out.push_str(&format!(" fo:hyphenate=\"{hyphenate}\""));
    }
    if let Some(borders) = format.borders {
        out.push_str(&borders_xml(borders));
    }
    let mut children = String::new();
    if let Some(tabs) = format.tab_stops {
        children.push_str("<style:tab-stops>");
        for stop in tabs.iter() {
            let kind = match stop.kind {
                TabStopKind::Left => "left",
                TabStopKind::Centre => "center",
                TabStopKind::Right => "right",
                TabStopKind::Decimal => "char",
            };
            children.push_str(&format!(
                "<style:tab-stop style:position=\"{}\" style:type=\"{kind}\" style:char=\".\"/>",
                units::format(stop.position_pt)
            ));
        }
        children.push_str("</style:tab-stops>");
    }
    if out.is_empty() && children.is_empty() {
        return String::new();
    }
    if children.is_empty() {
        format!("<style:paragraph-properties{out}/>")
    } else {
        format!("<style:paragraph-properties{out}>{children}</style:paragraph-properties>")
    }
}

/// Border attributes; `format` returns the empty string when there is no box.
fn borders_xml(borders: Borders) -> String {
    if !borders.is_visible() {
        return " fo:border=\"none\"".to_owned();
    }
    let line = format!(
        "{}pt solid {}",
        trim_number(borders.width_pt),
        colour::hex(borders.colour)
    );
    let mut out = String::new();
    let sides = borders.sides;
    if sides.top && sides.right && sides.bottom && sides.left {
        out.push_str(&format!(" fo:border=\"{line}\""));
    } else {
        for (name, present) in [
            ("top", sides.top),
            ("right", sides.right),
            ("bottom", sides.bottom),
            ("left", sides.left),
        ] {
            if present {
                out.push_str(&format!(" fo:border-{name}=\"{line}\""));
            } else {
                out.push_str(&format!(" fo:border-{name}=\"none\""));
            }
        }
    }
    if borders.space_pt > 0.0 {
        out.push_str(&format!(
            " fo:padding=\"{}\"",
            units::format(borders.space_pt)
        ));
    }
    out
}

/// Attribute list for `style:text-properties`.
fn text_props_xml(format: &CharFormat) -> String {
    let mut out = String::new();
    if let Some(family) = &format.family {
        out.push_str(&format!(" fo:font-family=\"{}\"", escape_attr(family)));
    }
    if let Some(size) = format.size_pt {
        out.push_str(&format!(" fo:font-size=\"{}\"", units::format(size)));
    }
    if let Some(weight) = format.weight {
        let value = if weight >= 600 { "bold" } else { "normal" };
        out.push_str(&format!(" fo:font-weight=\"{value}\""));
    }
    if let Some(italic) = format.italic {
        out.push_str(&format!(
            " fo:font-style=\"{}\"",
            if italic { "italic" } else { "normal" }
        ));
    }
    if let Some(underline) = format.underline {
        out.push_str(&format!(
            " style:text-underline-style=\"{}\"",
            if underline { "solid" } else { "none" }
        ));
    }
    if let Some(strike) = format.strikethrough {
        out.push_str(&format!(
            " style:text-line-through-style=\"{}\"",
            if strike { "solid" } else { "none" }
        ));
    }
    if let Some(colour_value) = format.colour {
        out.push_str(&format!(" fo:color=\"{}\"", colour::hex(colour_value)));
    }
    if let Some(highlight) = format.highlight {
        match highlight {
            Some(value) => {
                out.push_str(&format!(" fo:background-color=\"{}\"", colour::hex(value)))
            }
            None => out.push_str(" fo:background-color=\"transparent\""),
        }
    }
    if let Some(vertical) = format.vertical {
        let value = match vertical {
            VerticalAlign::Superscript => "super 58%",
            VerticalAlign::Subscript => "sub 58%",
            VerticalAlign::Baseline => "0% 100%",
        };
        out.push_str(&format!(" style:text-position=\"{value}\""));
    }
    if let Some(spacing) = format.letter_spacing_pt {
        out.push_str(&format!(
            " fo:letter-spacing=\"{}\"",
            units::format(spacing)
        ));
    }
    out
}

// ------------------------------------------------------------------ helpers

fn trim_number(value: f32) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1.0e9 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn master_name(index: usize) -> String {
    if index == 0 {
        "Standard".to_owned()
    } else {
        format!("LibertumSection{index}")
    }
}

fn layout_name(index: usize) -> String {
    format!("pm{index}")
}

fn vertical_span(table: &Table, row: usize, cell: usize) -> usize {
    let Some(table_cell) = table.rows.get(row).and_then(|row| row.cells.get(cell)) else {
        return 1;
    };
    let Some(column) = table.column_of(row, cell) else {
        return 1;
    };
    let mut span = 1;
    for below in (row + 1)..table.rows.len() {
        match table.cell_at_column(below, column) {
            Some((_, cell)) if cell.merged_into_above && cell.col_span == table_cell.col_span => {
                span += 1;
            }
            _ => break,
        }
    }
    span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spaces_and_tabs_are_encoded() {
        let mut out = String::new();
        write_text(&mut out, "a b  c\td", false, false);
        assert_eq!(out, "a b <text:s/>c<text:tab/>d");
        let mut out = String::new();
        write_text(&mut out, " leading", true, false);
        assert_eq!(out, "<text:s/>leading");
        let mut out = String::new();
        write_text(&mut out, "trailing ", false, true);
        assert_eq!(out, "trailing<text:s/>");
    }

    #[test]
    fn header_tokens_become_fields() {
        let mut out = String::new();
        write_header_text(&mut out, "Page {page} of {pages}");
        assert!(out.contains("<text:page-number/>"));
        assert!(out.contains("<text:page-count/>"));
        assert!(out.contains("Page<text:s/>"));
        assert!(out.contains("of<text:s/>"));
    }

    #[test]
    fn list_levels_match_the_model() {
        match list_level(ListStyle::Harvard, 0) {
            ListLevelKind::Number { format, .. } => assert_eq!(format, "I"),
            _ => panic!("expected a number"),
        }
        match list_level(ListStyle::Numbered, 1) {
            ListLevelKind::Number { format, .. } => assert_eq!(format, "a"),
            _ => panic!("expected a number"),
        }
        assert!(matches!(
            list_level(ListStyle::Bullet, 1),
            ListLevelKind::Bullet("◦")
        ));
    }

    #[test]
    fn list_tree_nests_levels_and_skips() {
        use libertum_model::{BlockId, ListItem, ParaProps};
        let paragraph = |level: u8| {
            let props = ParaProps {
                list: Some(ListItem {
                    style: ListStyle::Numbered,
                    level,
                    group: 1,
                }),
                ..ParaProps::default()
            };
            Paragraph::with_runs(BlockId(level as u64), "x", Vec::new()).with_props(props)
        };
        let items = [
            paragraph(0),
            paragraph(1),
            paragraph(1),
            paragraph(0),
            paragraph(2),
        ];
        let refs: Vec<&Paragraph> = items.iter().collect();
        let level = build_level(&refs, 0);
        assert_eq!(level.items.len(), 2, "two top-level items");
        assert_eq!(
            level.items[0]
                .children
                .as_ref()
                .map(|child| child.items.len()),
            Some(2)
        );
        assert!(level.items[1].children.is_some(), "level 2 is wrapped");
    }

    #[test]
    fn paragraph_borders_write_shorthand_and_sides() {
        use libertum_model::BorderSides;
        let all = ParaFormat {
            borders: Some(Borders::line(1.0)),
            ..ParaFormat::default()
        };
        assert!(paragraph_properties(&all, false).contains("fo:border=\"1pt solid #000000\""));
        let one = ParaFormat {
            borders: Some(Borders {
                sides: BorderSides {
                    top: true,
                    ..BorderSides::NONE
                },
                ..Borders::line(1.0)
            }),
            ..ParaFormat::default()
        };
        let xml = paragraph_properties(&one, false);
        assert!(xml.contains("fo:border-top=\"1pt solid #000000\""));
        assert!(xml.contains("fo:border-left=\"none\""));
    }
}
