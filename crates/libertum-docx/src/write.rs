//! Writing the model back as a DOCX package (PLAN §6.2).
//!
//! Every package part the importer kept stays in the package; this module overwrites only
//! `word/document.xml`, `word/styles.xml`, `word/numbering.xml` and the footnotes/endnotes
//! parts, and makes sure the package content types and relationships mention them. A new
//! document gets a complete minimal package (content types, relationships, core/app
//! properties).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use libertum_formats_common::toc::LIBERTUM_TOC_TAG;
use libertum_model::{
    Align, Block, BlockId, CharFormat, CrossRefKind, Document, InlineImage, LineSpacing, ListStyle,
    Media, NoteKind, Orientation, PageSetup, ParaFormat, Paragraph, Section, StyleId, StyleSheet,
    TabStopKind, Table, TableStyle, VerticalAlign,
};

use crate::package::Package;
use crate::xml::{escape_attr, escape_text};
use crate::{DocxError, Extras, SectionExtras};

pub(crate) const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
pub(crate) const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub(crate) const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub(crate) const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub(crate) const REL_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const PIC_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
/// Part-name prefix for the picture parts Libertum writes. They are named by content hash, and
/// every save replaces the ones a previous save wrote.
const MEDIA_PREFIX: &str = "word/media/lbt-";
/// Relationship ids Libertum gives its picture relationships.
const IMAGE_REL_PREFIX: &str = "rIdImg";
const EMU_PER_POINT: f32 = 12_700.0;
const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

const CT_DOCUMENT: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const CT_STYLES: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml";
const CT_NUMBERING: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml";
const CT_FOOTNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml";
const CT_ENDNOTES: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml";
pub(crate) const CT_HEADER: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml";
pub(crate) const CT_FOOTER: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml";
pub(crate) const CT_SETTINGS: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml";
const CT_CORE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
const CT_APP: &str = "application/vnd.openxmlformats-officedocument.extended-properties+xml";

pub(crate) fn build(
    document: &Document,
    extras: &Extras,
    origin: Option<&Package>,
    toc_pages: &HashMap<BlockId, u32>,
) -> Result<Package, DocxError> {
    let note_ids = note_ids(document);
    let footnotes_part = notes_part(NoteKind::Footnote, &note_ids.footnotes);
    let endnotes_part = notes_part(NoteKind::Endnote, &note_ids.endnotes);
    let table_styles = table_styles(document);
    // Watermarks live in the headers: work out their edits first, since new headers add
    // references to the sections' properties.
    let mut extras = extras.clone();
    // Edited headers and footers first (they become parts the watermark is then added to).
    let mut staged = origin.cloned().unwrap_or_default();
    let furniture_edits = crate::headers::plan(document, &mut extras, origin);
    furniture_edits.clone().apply(&mut staged);
    let watermark_edits = crate::watermark::plan(document, &mut extras, Some(&staged));
    let mut header_edits = furniture_edits.then(watermark_edits);
    if document.sections.iter().any(|s| s.page_colour.is_some()) {
        header_edits.require_display_background();
    }
    let (document_part, media) =
        document_xml(document, &extras, &note_ids, &table_styles, toc_pages);
    let mut styles_part = styles_xml(
        &document.styles,
        &table_styles,
        document
            .sections
            .first()
            .and_then(|section| section.language.as_deref()),
    );
    if let Some(toc_styles) = toc_styles_xml(document)
        && let Some(end) = styles_part.rfind("</w:styles>")
    {
        styles_part.insert_str(end, &toc_styles);
    }
    let numbering_part = numbering_xml(document);

    let mut package = match origin {
        Some(origin) => {
            let mut package = origin.clone();
            // Pictures written by an earlier save are replaced by the current set.
            package.remove_prefix(MEDIA_PREFIX);
            strip_image_relationships(&mut package);
            package.set_text("word/document.xml", document_part);
            package.set_text("word/styles.xml", styles_part);
            match &numbering_part {
                Some(numbering) => package.set_text("word/numbering.xml", numbering.clone()),
                // Keep the part (and its relationship) valid when the last list is removed.
                None if package.contains("word/numbering.xml") => {
                    package.set_text("word/numbering.xml", empty_numbering_xml());
                }
                None => {}
            }
            ensure_notes_part(&mut package, NoteKind::Footnote, footnotes_part);
            ensure_notes_part(&mut package, NoteKind::Endnote, endnotes_part);
            ensure_content_type(&mut package, "word/document.xml", CT_DOCUMENT);
            ensure_content_type(&mut package, "word/styles.xml", CT_STYLES);
            if package.contains("word/numbering.xml") {
                ensure_content_type(&mut package, "word/numbering.xml", CT_NUMBERING);
                ensure_relationship(&mut package, "numbering.xml", "numbering");
            }
            ensure_relationship(&mut package, "styles.xml", "styles");
            package
        }
        None => new_package(
            document_part,
            styles_part,
            numbering_part,
            footnotes_part,
            endnotes_part,
        ),
    };
    add_media(&mut package, &media);
    header_edits.apply(&mut package);
    crate::headers::set_language2(
        &mut package,
        document
            .sections
            .first()
            .and_then(|section| section.language2.as_deref()),
    );

    if !package.contains("docProps/core.xml") {
        package.set_text("docProps/core.xml", core_properties());
        ensure_content_type(&mut package, "docProps/core.xml", CT_CORE);
    }
    if !package.contains("docProps/app.xml") {
        package.set_text("docProps/app.xml", app_properties());
        ensure_content_type(&mut package, "docProps/app.xml", CT_APP);
    }
    ensure_package_relationships(&mut package);
    Ok(package)
}

// ------------------------------------------------------------------ package plumbing

fn new_package(
    document: String,
    styles: String,
    numbering: Option<String>,
    footnotes: Option<String>,
    endnotes: Option<String>,
) -> Package {
    let mut package = Package::default();
    package.set_text("word/document.xml", document);
    package.set_text("word/styles.xml", styles);
    if let Some(numbering) = numbering {
        package.set_text("word/numbering.xml", numbering);
    }
    if let Some(footnotes) = footnotes {
        package.set_text("word/footnotes.xml", footnotes);
    }
    if let Some(endnotes) = endnotes {
        package.set_text("word/endnotes.xml", endnotes);
    }
    package.set_text("docProps/core.xml", core_properties());
    package.set_text("docProps/app.xml", app_properties());
    package.set_text("[Content_Types].xml", content_types(&package));
    package.set_text("_rels/.rels", package_relationships());
    package.set_text(
        "word/_rels/document.xml.rels",
        document_relationships(&package),
    );
    package
}

fn content_types(package: &Package) -> String {
    let mut out = String::from(XML_DECL);
    out.push_str(&format!(
        "<Types xmlns=\"{CT_NS}\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>"
    ));
    let mut add = |part: &str, content_type: &str| {
        out.push_str(&format!(
            "<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/>"
        ));
    };
    add("word/document.xml", CT_DOCUMENT);
    add("word/styles.xml", CT_STYLES);
    if package.contains("word/numbering.xml") {
        add("word/numbering.xml", CT_NUMBERING);
    }
    if package.contains("word/footnotes.xml") {
        add("word/footnotes.xml", CT_FOOTNOTES);
    }
    if package.contains("word/endnotes.xml") {
        add("word/endnotes.xml", CT_ENDNOTES);
    }
    if package.contains("docProps/core.xml") {
        add("docProps/core.xml", CT_CORE);
    }
    if package.contains("docProps/app.xml") {
        add("docProps/app.xml", CT_APP);
    }
    out.push_str("</Types>");
    out
}

fn package_relationships() -> String {
    format!(
        "{XML_DECL}<Relationships xmlns=\"{REL_NS}\">\
         <Relationship Id=\"rId1\" Type=\"{REL_TYPE}/officeDocument\" Target=\"word/document.xml\"/>\
         <Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/>\
         <Relationship Id=\"rId3\" Type=\"{REL_TYPE}/extended-properties\" Target=\"docProps/app.xml\"/>\
         </Relationships>"
    )
}

fn document_relationships(package: &Package) -> String {
    let mut out = String::from(XML_DECL);
    out.push_str(&format!("<Relationships xmlns=\"{REL_NS}\">"));
    let mut next = 1u32;
    let add = |out: &mut String, kind: &str, target: &str, next: &mut u32| {
        out.push_str(&format!(
            "<Relationship Id=\"rId{next}\" Type=\"{REL_TYPE}/{kind}\" Target=\"{target}\"/>"
        ));
        *next += 1;
    };
    add(&mut out, "styles", "styles.xml", &mut next);
    if package.contains("word/numbering.xml") {
        add(&mut out, "numbering", "numbering.xml", &mut next);
    }
    if package.contains("word/footnotes.xml") {
        add(&mut out, "footnotes", "footnotes.xml", &mut next);
    }
    if package.contains("word/endnotes.xml") {
        add(&mut out, "endnotes", "endnotes.xml", &mut next);
    }
    out.push_str("</Relationships>");
    out
}

/// Writes a note part when the document has notes, and replaces a stale part with empty
/// separators when the last note was removed, so the relationship stays valid. A package that
/// already points at a non-standard notes part keeps that name.
fn ensure_notes_part(package: &mut Package, kind: NoteKind, part: Option<String>) {
    let (suffix, default, content_type, relationship) = match kind {
        NoteKind::Footnote => (
            "/footnotes",
            "word/footnotes.xml",
            CT_FOOTNOTES,
            "footnotes",
        ),
        NoteKind::Endnote => ("/endnotes", "word/endnotes.xml", CT_ENDNOTES, "endnotes"),
    };
    let name = package
        .related_part(suffix, default)
        .unwrap_or_else(|| default.to_owned());
    match part {
        Some(text) => package.set_text(&name, text),
        None if package.contains(&name) => package.set_text(&name, empty_notes_xml(kind)),
        None => return,
    }
    let target = name
        .strip_prefix("word/")
        .map(str::to_owned)
        .unwrap_or_else(|| format!("../{name}"));
    ensure_content_type(package, &name, content_type);
    ensure_relationship(package, &target, relationship);
}

/// Adds the content-type override for `part` if the package does not declare it.
pub(crate) fn ensure_content_type(package: &mut Package, part: &str, content_type: &str) {
    let name = "[Content_Types].xml";
    match package.text(name) {
        Some(text) => {
            if !text.contains(&format!("PartName=\"/{part}\"")) {
                let insert = format!(
                    "<Override PartName=\"/{part}\" ContentType=\"{content_type}\"/></Types>"
                );
                package.set_text(name, text.replacen("</Types>", &insert, 1));
            }
        }
        None => {
            let types = format!(
                "{XML_DECL}<Types xmlns=\"{CT_NS}\">\
                 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
                 <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
                 <Override PartName=\"/{part}\" ContentType=\"{content_type}\"/></Types>"
            );
            package.set_text(name, types);
        }
    }
}

/// Adds a relationship from `document.xml` to `target` (a part beside it) if there is none.
pub(crate) fn ensure_relationship(package: &mut Package, target: &str, kind: &str) {
    let name = "word/_rels/document.xml.rels";
    match package.text(name) {
        Some(text) => {
            if !text.contains(&format!("Target=\"{target}\"")) {
                let id = next_relationship_id(&text);
                let insert = format!(
                    "<Relationship Id=\"{id}\" Type=\"{REL_TYPE}/{kind}\" Target=\"{target}\"/></Relationships>"
                );
                package.set_text(name, text.replacen("</Relationships>", &insert, 1));
            }
        }
        None => {
            package.set_text(
                name,
                format!(
                    "{XML_DECL}<Relationships xmlns=\"{REL_NS}\">\
                     <Relationship Id=\"rId1\" Type=\"{REL_TYPE}/{kind}\" Target=\"{target}\"/></Relationships>"
                ),
            );
        }
    }
}

/// A picture part to write, with the relationship id `document.xml` refers to it by.
struct MediaPart {
    part: String,
    rel_id: String,
    media: Arc<Media>,
}

/// Removes the picture relationships an earlier save wrote (their parts were removed with
/// the `word/media/lbt-` prefix). Other producers' relationships stay.
fn strip_image_relationships(package: &mut Package) {
    let name = "word/_rels/document.xml.rels";
    let Some(text) = package.text(name) else {
        return;
    };
    let marker = format!("Id=\"{IMAGE_REL_PREFIX}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<Relationship ") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        // Relationship elements are always self-closing.
        let Some(end) = tail.find("/>") else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let element = &tail[..end + 2];
        if !element.contains(&marker) {
            out.push_str(element);
        }
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    package.set_text(name, out);
}

/// Adds the picture parts, their relationships and the `Default` content types.
fn add_media(package: &mut Package, media: &[MediaPart]) {
    for part in media {
        package.set(&part.part, part.media.bytes().to_vec());
        ensure_default_content_type(
            package,
            part.media.format().extension(),
            part.media.format().mime(),
        );
        let target = part.part.strip_prefix("word/").unwrap_or(&part.part);
        let relationship = format!(
            "<Relationship Id=\"{}\" Type=\"{REL_TYPE}/image\" Target=\"{target}\"/>",
            part.rel_id
        );
        let name = "word/_rels/document.xml.rels";
        match package.text(name) {
            Some(text) => {
                let insert = format!("{relationship}</Relationships>");
                package.set_text(name, text.replacen("</Relationships>", &insert, 1));
            }
            None => package.set_text(
                name,
                format!(
                    "{XML_DECL}<Relationships xmlns=\"{REL_NS}\">{relationship}</Relationships>"
                ),
            ),
        }
    }
}

/// Adds `<Default Extension=...>` to the content types if the package has none for it.
fn ensure_default_content_type(package: &mut Package, extension: &str, content_type: &str) {
    let name = "[Content_Types].xml";
    let Some(text) = package.text(name) else {
        return;
    };
    let declared = text
        .to_ascii_lowercase()
        .contains(&format!("extension=\"{extension}\""));
    if declared {
        return;
    }
    let Some(open) = text.find("<Types") else {
        return;
    };
    let Some(close) = text[open..].find('>') else {
        return;
    };
    let at = open + close + 1;
    let mut updated = String::with_capacity(text.len() + 64);
    updated.push_str(&text[..at]);
    updated.push_str(&format!(
        "<Default Extension=\"{extension}\" ContentType=\"{content_type}\"/>"
    ));
    updated.push_str(&text[at..]);
    package.set_text(name, updated);
}

fn ensure_package_relationships(package: &mut Package) {
    if !package.contains("_rels/.rels") {
        package.set_text("_rels/.rels", package_relationships());
    }
}

fn next_relationship_id(rels: &str) -> String {
    let mut max = 0u32;
    for part in rels.split("Id=\"rId").skip(1) {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(number) = digits.parse::<u32>() {
            max = max.max(number);
        }
    }
    format!("rId{}", max + 1)
}

fn core_properties() -> String {
    format!(
        "{XML_DECL}<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\
         <dc:creator>Libertum</dc:creator><cp:lastModifiedBy>Libertum</cp:lastModifiedBy>\
         </cp:coreProperties>"
    )
}

fn app_properties() -> String {
    format!(
        "{XML_DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\">\
         <Application>Libertum</Application></Properties>"
    )
}

fn empty_numbering_xml() -> String {
    format!("{XML_DECL}<w:numbering xmlns:w=\"{W_NS}\"/>")
}

// ------------------------------------------------------------------ notes

/// DOCX ids for the notes, plus the note bodies each part has to carry. Footnote and endnote
/// ids are separate sequences, as in Word.
#[derive(Default)]
struct NoteIds {
    ids: HashMap<(BlockId, usize), u32>,
    footnotes: Vec<(u32, String)>,
    endnotes: Vec<(u32, String)>,
}

fn note_ids(document: &Document) -> NoteIds {
    let mut note_ids = NoteIds::default();
    let mut next_footnote = 1u32;
    let mut next_endnote = 1u32;
    for paragraph in document.paragraphs() {
        for (index, note) in paragraph.notes().iter().enumerate() {
            let id = match note.kind {
                NoteKind::Footnote => {
                    let id = next_footnote;
                    next_footnote += 1;
                    note_ids.footnotes.push((id, note.text.clone()));
                    id
                }
                NoteKind::Endnote => {
                    let id = next_endnote;
                    next_endnote += 1;
                    note_ids.endnotes.push((id, note.text.clone()));
                    id
                }
            };
            note_ids.ids.insert((paragraph.id, index), id);
        }
    }
    note_ids
}

fn note_names(kind: NoteKind) -> (&'static str, &'static str, &'static str) {
    match kind {
        NoteKind::Footnote => ("footnotes", "footnote", "footnoteRef"),
        NoteKind::Endnote => ("endnotes", "endnote", "endnoteRef"),
    }
}

fn notes_part(kind: NoteKind, notes: &[(u32, String)]) -> Option<String> {
    if notes.is_empty() {
        return None;
    }
    Some(notes_part_xml(kind, notes))
}

/// A note part with Word's separator and continuation-separator entries and one plain-text
/// paragraph per note.
fn notes_part_xml(kind: NoteKind, notes: &[(u32, String)]) -> String {
    let (root, item, reference) = note_names(kind);
    let mut out = String::with_capacity(512 + notes.len() * 128);
    out.push_str(XML_DECL);
    out.push_str(&format!("<w:{root} xmlns:w=\"{W_NS}\">"));
    let separator = |kind_attr: &str, id: &str, element: &str| {
        format!(
            "<w:{item} w:type=\"{kind_attr}\" w:id=\"{id}\"><w:p>\
             <w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
             <w:r><w:{element}/></w:r></w:p></w:{item}>"
        )
    };
    out.push_str(&separator("separator", "-1", "separator"));
    out.push_str(&separator(
        "continuationSeparator",
        "0",
        "continuationSeparator",
    ));
    for (id, text) in notes {
        out.push_str(&format!(
            "<w:{item} w:id=\"{id}\"><w:p>\
             <w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:{reference}/></w:r>{}\
             </w:p></w:{item}>",
            run_xml(text, &CharFormat::default())
        ));
    }
    out.push_str(&format!("</w:{root}>"));
    out
}

fn empty_notes_xml(kind: NoteKind) -> String {
    notes_part_xml(kind, &[])
}

fn note_reference_xml(kind: NoteKind, id: u32) -> String {
    let element = match kind {
        NoteKind::Footnote => "footnoteReference",
        NoteKind::Endnote => "endnoteReference",
    };
    format!(
        "<w:r><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr><w:{element} w:id=\"{id}\"/></w:r>"
    )
}

/// One reference per note, anchored at its byte offset. Offsets past the end are clamped and
/// offsets inside a character move back to the character's start, so the output is always
/// valid XML and slices always fall on character boundaries.
fn note_references(paragraph: &Paragraph, note_ids: &NoteIds) -> Vec<(usize, NoteKind, u32)> {
    let text = paragraph.text();
    let mut references: Vec<(usize, NoteKind, u32)> = paragraph
        .notes()
        .iter()
        .enumerate()
        .filter_map(|(index, note)| {
            let id = *note_ids.ids.get(&(paragraph.id, index))?;
            let mut offset = note.offset.min(text.len());
            while offset > 0 && !text.is_char_boundary(offset) {
                offset -= 1;
            }
            Some((offset, note.kind, id))
        })
        .collect();
    references.sort_by_key(|(offset, _, _)| *offset);
    references
}

// ------------------------------------------------------------------ document.xml

/// Numbers and registries that run through the whole `document.xml`.
struct Counters {
    bookmarks: u32,
    drawings: u32,
    media: Vec<MediaPart>,
    /// Lower-case names of bookmarks that a text cross-reference points at. Their bookmark
    /// runs to the end of its paragraph, so Word's own refresh of a `REF` field shows the
    /// paragraph text, as the model says it does.
    text_targets: HashSet<String>,
    /// Block ids of the headings the table of contents points at: each gets a `_Toc` bookmark.
    toc_targets: HashSet<u64>,
    /// Page number of each of those headings, as the layout resolved them.
    toc_pages: HashMap<u64, u32>,
    /// Counter for the ids of the content controls that wrap a table of contents.
    controls: u32,
}

impl Counters {
    fn new(document: &Document, toc_pages: &HashMap<BlockId, u32>) -> Self {
        let text_targets = document
            .paragraphs()
            .flat_map(|paragraph| paragraph.cross_refs())
            .filter(|cross_ref| cross_ref.kind == CrossRefKind::Text)
            .map(|cross_ref| cross_ref.target.to_lowercase())
            .collect();
        let toc_targets = document
            .paragraphs()
            .filter_map(|paragraph| paragraph.props.toc_target)
            .map(|target| target.0)
            .filter(|target| *target != 0)
            .collect();
        Self {
            bookmarks: 0,
            drawings: 0,
            media: Vec::new(),
            text_targets,
            toc_targets,
            toc_pages: toc_pages.iter().map(|(id, page)| (id.0, *page)).collect(),
            controls: 0,
        }
    }

    /// The relationship id for `media`, registering a part for it on first use.
    fn media_rel(&mut self, media: &Arc<Media>) -> String {
        if let Some(part) = self
            .media
            .iter()
            .find(|part| Arc::ptr_eq(&part.media, media) || *part.media == **media)
        {
            return part.rel_id.clone();
        }
        let rel_id = format!("{IMAGE_REL_PREFIX}{}", self.media.len() + 1);
        self.media.push(MediaPart {
            part: format!(
                "{MEDIA_PREFIX}{:016x}.{}",
                media.hash(),
                media.format().extension()
            ),
            rel_id: rel_id.clone(),
            media: Arc::clone(media),
        });
        rel_id
    }
}

fn document_xml(
    document: &Document,
    extras: &Extras,
    note_ids: &NoteIds,
    table_styles: &TableStyles,
    toc_pages: &HashMap<BlockId, u32>,
) -> (String, Vec<MediaPart>) {
    let paragraphs: Vec<&Paragraph> = document
        .body()
        .iter()
        .filter_map(|block| block.as_paragraph())
        .collect();
    // The section each paragraph belongs to.
    let mut section_of = Vec::with_capacity(paragraphs.len());
    let mut section = 0usize;
    for paragraph in &paragraphs {
        if paragraph.props.section_start && !section_of.is_empty() {
            section += 1;
        }
        section_of.push(section);
    }

    let list_ids = list_ids(document);
    let mut ids = Counters::new(document, toc_pages);
    let body_blocks: Vec<&Block> = document.body().iter().collect();
    let toc_runs = toc_runs(&body_blocks);
    let mut out = String::with_capacity(document.body().len() * 160);
    out.push_str(XML_DECL);
    out.push_str(&format!(
        "<w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\" xmlns:wp=\"{WP_NS}\">"
    ));
    // The page colour comes before the body; `settings.xml` must also turn on
    // `w:displayBackgroundShape` or Word does not show it.
    if let Some(colour) = document.sections.first().and_then(|s| s.page_colour) {
        out.push_str(&format!("<w:background w:color=\"{}\"/>", fill_hex(colour)));
    }
    out.push_str("<w:body>");
    let mut paragraph_index = 0usize;
    for (body_index, block) in document.body().iter().enumerate() {
        match block {
            Block::Paragraph(paragraph) => {
                let index = paragraph_index;
                paragraph_index += 1;
                let ends_section =
                    index + 1 < paragraphs.len() && section_of[index + 1] != section_of[index];
                let sect = if ends_section {
                    let section_index = section_of[index];
                    let extra = extras.get(section_index).cloned().unwrap_or_default();
                    document
                        .sections
                        .get(section_index)
                        .map(|section| sect_pr(section, &extra))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                if toc_runs.iter().any(|run| run.open_at == body_index) {
                    out.push_str(&toc_open_xml(&mut ids));
                }
                if paragraph.props.toc_target.is_some() {
                    let run = toc_runs
                        .iter()
                        .find(|run| (run.first_entry..=run.last_entry).contains(&body_index));
                    let width = document
                        .sections
                        .get(section_of[index])
                        .map_or(9026, |section| text_width_twips(section.page));
                    out.push_str(&toc_entry_xml(
                        paragraph,
                        run.is_some_and(|run| run.first_entry == body_index),
                        run.is_some_and(|run| run.last_entry == body_index),
                        width,
                        &ids,
                    ));
                } else {
                    out.push_str(&paragraph_xml(
                        paragraph, &list_ids, note_ids, &sect, &mut ids,
                    ));
                }
                if toc_runs.iter().any(|run| run.last_entry == body_index) {
                    out.push_str("</w:sdtContent></w:sdt>");
                }
            }
            Block::Table(table) => out.push_str(&table_xml(
                table,
                &list_ids,
                note_ids,
                table_styles,
                &mut ids,
            )),
        }
    }
    // The final section's properties belong to the body.
    let last = section_of.last().copied().unwrap_or(0);
    if let Some(section) = document.sections.get(last) {
        let extra = extras.get(last).cloned().unwrap_or_default();
        out.push_str(&sect_pr(section, &extra));
    }
    out.push_str("</w:body></w:document>");
    (out, ids.media)
}

/// A table block (`w:tbl`): grid, borders, header rows, cell fills and cell paragraphs.
fn table_xml(
    table: &Table,
    list_ids: &HashMap<(u8, u64), u32>,
    note_ids: &NoteIds,
    table_styles: &TableStyles,
    ids: &mut Counters,
) -> String {
    let style = table
        .style
        .as_ref()
        .and_then(|style| table_styles.get(style));
    // CT_TblPr order: tblStyle, tblW, tblBorders, tblLayout, tblLook.
    let mut out = String::from("<w:tbl><w:tblPr>");
    if let Some(def) = style {
        out.push_str(&format!("<w:tblStyle w:val=\"{}\"/>", escape_attr(&def.id)));
    }
    out.push_str("<w:tblW w:w=\"0\" w:type=\"auto\"/>");
    if table.borders.is_visible() {
        let borders = &table.borders;
        let colour = format!(
            "{:02X}{:02X}{:02X}",
            borders.colour[0], borders.colour[1], borders.colour[2]
        );
        let size = (borders.width_pt * 8.0).round().clamp(2.0, 96.0) as u32;
        let side = |name: &str, on: bool| {
            if on {
                format!(
                    "<w:{name} w:val=\"single\" w:sz=\"{size}\" w:space=\"0\" w:color=\"{colour}\"/>"
                )
            } else {
                format!("<w:{name} w:val=\"nil\"/>")
            }
        };
        out.push_str("<w:tblBorders>");
        out.push_str(&side("top", borders.outer));
        out.push_str(&side("left", borders.outer));
        out.push_str(&side("bottom", borders.outer));
        out.push_str(&side("right", borders.outer));
        out.push_str(&side("insideH", borders.inner));
        out.push_str(&side("insideV", borders.inner));
        out.push_str("</w:tblBorders>");
    }
    out.push_str("<w:tblLayout w:type=\"fixed\"/>");
    if let Some(def) = style {
        out.push_str(&tbl_look_xml(table, &def.style));
    }
    out.push_str("</w:tblPr><w:tblGrid>");
    for width in &table.widths_pt {
        out.push_str(&format!(
            "<w:gridCol w:w=\"{}\"/>",
            (width * 20.0).round().max(0.0) as u32
        ));
    }
    out.push_str("</w:tblGrid>");
    for (row_index, row) in table.rows.iter().enumerate() {
        let header = row_index < usize::from(table.header_rows);
        out.push_str("<w:tr>");
        if header || row.keep_together || row.min_height_pt > 0.0 {
            out.push_str("<w:trPr>");
            if row.keep_together {
                out.push_str("<w:cantSplit/>");
            }
            if header {
                out.push_str("<w:tblHeader/>");
            }
            if row.min_height_pt > 0.0 {
                out.push_str(&format!(
                    "<w:trHeight w:val=\"{}\" w:hRule=\"atLeast\"/>",
                    (row.min_height_pt * 20.0).round().max(0.0) as u32
                ));
            }
            out.push_str("</w:trPr>");
        }
        let mut column = 0usize;
        for cell in &row.cells {
            let span = usize::from(cell.col_span.max(1));
            let width: f32 = table.widths_pt.iter().skip(column).take(span).sum();
            out.push_str("<w:tc><w:tcPr>");
            out.push_str(&format!(
                "<w:tcW w:w=\"{}\" w:type=\"dxa\"/>",
                (width * 20.0).round().max(0.0) as u32
            ));
            if span > 1 {
                out.push_str(&format!("<w:gridSpan w:val=\"{span}\"/>"));
            }
            if cell.merged_into_above {
                out.push_str("<w:vMerge/>");
            }
            if let Some(fill) = cell.fill {
                out.push_str(&format!(
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{:02X}{:02X}{:02X}\"/>",
                    fill[0], fill[1], fill[2]
                ));
            }
            out.push_str("</w:tcPr>");
            for paragraph in &cell.paragraphs {
                out.push_str(&paragraph_xml(paragraph, list_ids, note_ids, "", ids));
            }
            out.push_str("</w:tc>");
            column += span;
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    out
}

fn paragraph_xml(
    paragraph: &Paragraph,
    list_ids: &HashMap<(u8, u64), u32>,
    note_ids: &NoteIds,
    sect: &str,
    ids: &mut Counters,
) -> String {
    let mut out = String::from("<w:p>");
    let ppr = paragraph_props_xml(paragraph, list_ids, sect);
    if !ppr.is_empty() {
        out.push_str("<w:pPr>");
        out.push_str(&ppr);
        out.push_str("</w:pPr>");
    }
    // A heading the table of contents points at carries the bookmark its entry links to.
    let toc_bookmark = ids.toc_targets.contains(&paragraph.id.0).then(|| {
        let id = ids.bookmarks;
        ids.bookmarks += 1;
        out.push_str(&format!(
            "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/>",
            toc_bookmark_name(paragraph.id)
        ));
        id
    });
    let text = paragraph.text();
    let references = note_references(paragraph, note_ids);
    enum Anchor<'a> {
        Note(NoteKind, u32),
        Bookmark(&'a str),
        Image(&'a InlineImage),
        FieldBegin(String),
        FieldEnd,
        /// A reading over the text up to the matching `RubyEnd`.
        RubyBegin(&'a str),
        RubyEnd,
    }
    /// Closing a field comes before anything else at its offset and opening one after, so
    /// adjacent references never nest and a picture at a boundary stays outside the field.
    fn priority(anchor: &Anchor<'_>) -> u8 {
        match anchor {
            Anchor::FieldEnd | Anchor::RubyEnd => 0,
            Anchor::FieldBegin(_) | Anchor::RubyBegin(_) => 2,
            _ => 1,
        }
    }
    fn anchor_xml(anchor: &Anchor<'_>, ids: &mut Counters, pending_ends: &mut Vec<u32>) -> String {
        match anchor {
            Anchor::Note(kind, id) => note_reference_xml(*kind, *id),
            Anchor::Bookmark(name) => {
                let id = ids.bookmarks;
                ids.bookmarks += 1;
                let start = format!(
                    "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/>",
                    escape_attr(name)
                );
                if ids.text_targets.contains(&name.to_lowercase()) {
                    // Closed at the end of the paragraph, so `REF` yields the paragraph text.
                    pending_ends.push(id);
                    start
                } else {
                    format!("{start}<w:bookmarkEnd w:id=\"{id}\"/>")
                }
            }
            Anchor::Image(image) => image_xml(image, ids),
            Anchor::FieldBegin(instruction) => format!(
                "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\">{}</w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>",
                escape_text(instruction)
            ),
            Anchor::FieldEnd => "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>".to_owned(),
            Anchor::RubyBegin(reading) => format!(
                "<w:r><w:ruby><w:rubyPr><w:rubyAlign w:val=\"distributeSpace\"/><w:hps w:val=\"10\"/><w:hpsRaise w:val=\"18\"/><w:hpsBaseText w:val=\"21\"/><w:lid w:val=\"ja-JP\"/></w:rubyPr><w:rt><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:rt><w:rubyBase>",
                escape_text(reading)
            ),
            Anchor::RubyEnd => "</w:rubyBase></w:ruby></w:r>".to_owned(),
        }
    }
    let mut anchors: Vec<(usize, Anchor)> = references
        .iter()
        .map(|(offset, kind, id)| (*offset, Anchor::Note(*kind, *id)))
        .chain(
            paragraph
                .bookmarks()
                .iter()
                .map(|bookmark| (bookmark.offset, Anchor::Bookmark(bookmark.name.as_str()))),
        )
        .chain(
            paragraph
                .images()
                .iter()
                .map(|image| (image.offset.min(text.len()), Anchor::Image(image))),
        )
        .collect();
    for cross_ref in paragraph.cross_refs() {
        let end = cross_ref.range.end.min(text.len());
        let start = cross_ref.range.start.min(end);
        if start == end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        anchors.push((
            start,
            Anchor::FieldBegin(cross_ref_instruction(&cross_ref.target, cross_ref.kind)),
        ));
        anchors.push((end, Anchor::FieldEnd));
    }
    for ruby in paragraph.ruby() {
        let end = ruby.range.end.min(text.len());
        let start = ruby.range.start.min(end);
        if start == end || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        anchors.push((start, Anchor::RubyBegin(ruby.text.as_str())));
        anchors.push((end, Anchor::RubyEnd));
    }
    anchors.sort_by_key(|(offset, anchor)| (*offset, priority(anchor)));
    let mut pending_ends: Vec<u32> = Vec::new();
    let mut next_anchor = 0usize;
    let mut emit = |start: usize, end: usize, format: &CharFormat, out: &mut String| {
        let start = start.min(text.len());
        let end = end.min(text.len()).max(start);
        // An anchor at this position belongs before the text that follows it.
        while let Some((offset, anchor)) = anchors.get(next_anchor) {
            if *offset > start {
                break;
            }
            out.push_str(&anchor_xml(anchor, ids, &mut pending_ends));
            next_anchor += 1;
        }
        let mut position = start;
        while let Some((offset, anchor)) = anchors.get(next_anchor) {
            if *offset >= end {
                break;
            }
            let at = (*offset).max(position);
            if at > position {
                out.push_str(&run_xml(&text[position..at], format));
            }
            out.push_str(&anchor_xml(anchor, ids, &mut pending_ends));
            next_anchor += 1;
            position = at;
        }
        if position < end {
            out.push_str(&run_xml(&text[position..end], format));
        }
    };
    let mut position = 0usize;
    for run in paragraph.runs() {
        let start = run.range.start.min(text.len());
        let end = run.range.end.min(text.len());
        if start > position {
            emit(position, start, &CharFormat::default(), &mut out);
        }
        emit(start, end, &run.format, &mut out);
        position = position.max(end);
    }
    if position < text.len() {
        emit(position, text.len(), &CharFormat::default(), &mut out);
    }
    // Anchors at the very end of the paragraph.
    while let Some((_, anchor)) = anchors.get(next_anchor) {
        out.push_str(&anchor_xml(anchor, ids, &mut pending_ends));
        next_anchor += 1;
    }
    for id in pending_ends {
        out.push_str(&format!("<w:bookmarkEnd w:id=\"{id}\"/>"));
    }
    if let Some(id) = toc_bookmark {
        out.push_str(&format!("<w:bookmarkEnd w:id=\"{id}\"/>"));
    }
    out.push_str("</w:p>");
    out
}

// ------------------------------------------------------------------ table of contents

/// A table of contents in the body: the blocks that go inside one content control. The control
/// opens at `open_at` (the title above the entries, when there is one) and closes after
/// `last_entry`.
struct TocRun {
    open_at: usize,
    first_entry: usize,
    last_entry: usize,
}

fn toc_runs(blocks: &[&Block]) -> Vec<TocRun> {
    let is_entry = |block: &Block| {
        block
            .as_paragraph()
            .is_some_and(|paragraph| paragraph.props.toc_target.is_some())
    };
    let mut runs = Vec::new();
    let mut index = 0usize;
    while index < blocks.len() {
        if !is_entry(blocks[index]) {
            index += 1;
            continue;
        }
        let first_entry = index;
        while index < blocks.len() && is_entry(blocks[index]) {
            index += 1;
        }
        let title = first_entry
            .checked_sub(1)
            .filter(|before| matches!(blocks[*before], Block::Paragraph(_)));
        runs.push(TocRun {
            open_at: title.unwrap_or(first_entry),
            first_entry,
            last_entry: index - 1,
        });
    }
    runs
}

/// The name of the bookmark on the heading an entry points at, in the form Word uses.
fn toc_bookmark_name(heading: BlockId) -> String {
    format!("_Toc{}", heading.0)
}

/// Width of the text area in twips, where a table of contents puts its right-aligned page
/// numbers.
fn text_width_twips(page: libertum_model::PageSetup) -> u32 {
    let (width, _) = paper_points(page);
    ((width - page.margins.left - page.margins.right).max(72.0) * 20.0).round() as u32
}

/// How deep an entry sits, 1 to 9: Libertum indents each level by 18 pt.
fn toc_level(paragraph: &Paragraph) -> usize {
    let indent = paragraph
        .props
        .format
        .indent_left_pt
        .unwrap_or(0.0)
        .max(0.0);
    ((indent / 18.0).round() as usize + 1).clamp(1, 9)
}

/// Opens the content control Word uses for a table of contents. Its tag says Libertum wrote it,
/// so reading it back needs no warning.
fn toc_open_xml(ids: &mut Counters) -> String {
    ids.controls += 1;
    format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{LIBERTUM_TOC_TAG}\"/><w:id w:val=\"{}\"/><w:docPartObj><w:docPartGallery w:val=\"Table of Contents\"/><w:docPartUnique/></w:docPartObj></w:sdtPr><w:sdtContent>",
        1_000_000 + ids.controls
    )
}

/// One entry of the table of contents: a paragraph in the `TOC n` style holding the heading's
/// text as a link to its `_Toc` bookmark, then a right-aligned tab and a `PAGEREF` field with
/// the page number. The first entry also opens the `TOC` field and the last closes it, as Word
/// writes it, so Word's Update Table rebuilds the whole list.
fn toc_entry_xml(
    paragraph: &Paragraph,
    first: bool,
    last: bool,
    width_twips: u32,
    ids: &Counters,
) -> String {
    let target = paragraph
        .props
        .toc_target
        .filter(|target| target.0 != 0)
        .map(|target| (target, ids.toc_pages.get(&target.0).copied()));
    let mut out = format!(
        "<w:p><w:pPr><w:pStyle w:val=\"TOC{}\"/>",
        toc_level(paragraph)
    );
    if matches!(target, Some((_, Some(_)))) {
        out.push_str(&format!(
            "<w:tabs><w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{width_twips}\"/></w:tabs>"
        ));
    }
    out.push_str("</w:pPr>");
    if first {
        out.push_str(
            "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> TOC \\o \"1-9\" \\h \\z \\u </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>",
        );
    }
    let text = run_xml(paragraph.text(), &CharFormat::default());
    match target {
        Some((heading, page)) => {
            let name = toc_bookmark_name(heading);
            out.push_str(&format!(
                "<w:hyperlink w:anchor=\"{name}\" w:history=\"1\">{text}"
            ));
            if let Some(page) = page {
                out.push_str(&format!(
                    "<w:r><w:tab/></w:r><w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\"> PAGEREF {name} \\h </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>{page}</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>"
                ));
            }
            out.push_str("</w:hyperlink>");
        }
        None => out.push_str(&text),
    }
    if last {
        out.push_str("<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>");
    }
    out.push_str("</w:p>");
    out
}

/// The `TOC 1` to `TOC 9` paragraph styles, written when the document has a table of contents.
/// Word's own definitions are the same shape: based on Normal, indented a level at a time.
fn toc_styles_xml(document: &Document) -> Option<String> {
    document
        .paragraphs()
        .any(|paragraph| paragraph.props.toc_target.is_some())
        .then(|| {
            (1..=9)
                .map(|level| {
                    format!(
                        "<w:style w:type=\"paragraph\" w:styleId=\"TOC{level}\"><w:name w:val=\"toc {level}\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/><w:uiPriority w:val=\"39\"/><w:unhideWhenUsed/><w:pPr><w:spacing w:after=\"100\"/><w:ind w:left=\"{}\"/></w:pPr></w:style>",
                        (level - 1) * 360
                    )
                })
                .collect()
        })
}

/// The field instruction for a cross-reference: `REF` for the text, `PAGEREF` for the page.
fn cross_ref_instruction(target: &str, kind: CrossRefKind) -> String {
    let keyword = match kind {
        CrossRefKind::Text => "REF",
        CrossRefKind::Page => "PAGEREF",
    };
    if target.chars().any(char::is_whitespace) {
        format!(" {keyword} \"{}\" \\h ", target.replace('"', "'"))
    } else {
        format!(" {keyword} {target} \\h ")
    }
}

/// An inline picture (`w:drawing/wp:inline`) at its natural place among the runs.
fn image_xml(image: &InlineImage, ids: &mut Counters) -> String {
    let rel_id = ids.media_rel(&image.media);
    ids.drawings += 1;
    let drawing_id = ids.drawings;
    let emu = |points: f32| (points * EMU_PER_POINT).round().max(1.0) as i64;
    let (cx, cy) = (emu(image.width_pt), emu(image.height_pt));
    format!(
        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/><wp:docPr id=\"{drawing_id}\" name=\"Picture {drawing_id}\" descr=\"{alt}\"/><wp:cNvGraphicFramePr><a:graphicFrameLocks xmlns:a=\"{A_NS}\" noChangeAspect=\"1\"/></wp:cNvGraphicFramePr><a:graphic xmlns:a=\"{A_NS}\"><a:graphicData uri=\"{PIC_NS}\"><pic:pic xmlns:pic=\"{PIC_NS}\"><pic:nvPicPr><pic:cNvPr id=\"{drawing_id}\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed=\"{rel_id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
        alt = escape_attr(&image.alt),
        name = escape_attr(&image.name),
    )
}

fn paragraph_props_xml(
    paragraph: &Paragraph,
    list_ids: &HashMap<(u8, u64), u32>,
    sect: &str,
) -> String {
    let props = &paragraph.props;
    let list = props.list.and_then(|list| {
        list_ids
            .get(&(list_style_code(list.style), list.group))
            .map(|&num_id| (num_id, list.level))
    });
    let mut out = String::new();
    out.push_str(&format!(
        "<w:pStyle w:val=\"{}\"/>",
        style_docx_id(props.style)
    ));
    out.push_str(&ppr_keep_xml(&props.format));
    if props.page_break_before {
        out.push_str("<w:pageBreakBefore/>");
    }
    out.push_str(&ppr_middle_xml(&props.format, list));
    out.push_str(&ppr_late_xml(&props.format));
    if let Some(level) = props.outline_level {
        out.push_str(&format!(
            "<w:outlineLvl w:val=\"{}\"/>",
            level.saturating_sub(1)
        ));
    }
    out.push_str(sect);
    out
}

fn style_docx_id(id: StyleId) -> String {
    match id {
        StyleId::BODY => "Normal".to_owned(),
        StyleId::TITLE => "Title".to_owned(),
        StyleId::HEADING_1 => "Heading1".to_owned(),
        StyleId::HEADING_2 => "Heading2".to_owned(),
        StyleId::HEADING_3 => "Heading3".to_owned(),
        StyleId::HEADING_4 => "Heading4".to_owned(),
        StyleId::QUOTE => "Quote".to_owned(),
        StyleId(other) => format!("Libertum{other}"),
    }
}

fn run_xml(text: &str, format: &CharFormat) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::from("<w:r>");
    out.push_str(&char_format_xml(format));
    for piece in text.split_inclusive('\t') {
        let (body, tab) = match piece.strip_suffix('\t') {
            Some(body) => (body, true),
            None => (piece, false),
        };
        if !body.is_empty() {
            out.push_str(&format!(
                "<w:t xml:space=\"preserve\">{}</w:t>",
                escape_text(body)
            ));
        }
        if tab {
            out.push_str("<w:tab/>");
        }
    }
    out.push_str("</w:r>");
    out
}

fn char_format_xml(format: &CharFormat) -> String {
    let inner = char_props_xml(format);
    if inner.is_empty() {
        String::new()
    } else {
        format!("<w:rPr>{inner}</w:rPr>")
    }
}

fn char_props_xml(format: &CharFormat) -> String {
    let mut out = String::new();
    if let Some(family) = &format.family {
        let family = escape_attr(family);
        out.push_str(&format!(
            "<w:rFonts w:ascii=\"{family}\" w:hAnsi=\"{family}\"/>"
        ));
    }
    if let Some(weight) = format.weight {
        out.push_str(if weight >= 600 {
            "<w:b/>"
        } else {
            "<w:b w:val=\"0\"/>"
        });
    }
    if let Some(italic) = format.italic {
        out.push_str(if italic {
            "<w:i/>"
        } else {
            "<w:i w:val=\"0\"/>"
        });
    }
    if let Some(underline) = format.underline {
        out.push_str(if underline {
            "<w:u w:val=\"single\"/>"
        } else {
            "<w:u w:val=\"none\"/>"
        });
    }
    if let Some(strike) = format.strikethrough {
        out.push_str(if strike {
            "<w:strike/>"
        } else {
            "<w:strike w:val=\"0\"/>"
        });
    }
    if let Some(size) = format.size_pt {
        out.push_str(&format!(
            "<w:sz w:val=\"{}\"/>",
            (size * 2.0).round().clamp(2.0, 3276.0) as u32
        ));
    }
    if let Some([r, g, b, _]) = format.colour {
        out.push_str(&format!("<w:color w:val=\"{r:02X}{g:02X}{b:02X}\"/>"));
    }
    if let Some(highlight) = format.highlight {
        match highlight {
            None => out.push_str("<w:highlight w:val=\"none\"/>"),
            Some([r, g, b, _]) => match highlight_name([r, g, b]) {
                Some(name) => out.push_str(&format!("<w:highlight w:val=\"{name}\"/>")),
                None => out.push_str(&format!(
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{r:02X}{g:02X}{b:02X}\"/>"
                )),
            },
        }
    }
    if let Some(vertical) = format.vertical {
        let value = match vertical {
            VerticalAlign::Superscript => "superscript",
            VerticalAlign::Subscript => "subscript",
            VerticalAlign::Baseline => "baseline",
        };
        out.push_str(&format!("<w:vertAlign w:val=\"{value}\"/>"));
    }
    if let Some(spacing) = format.letter_spacing_pt {
        out.push_str(&format!(
            "<w:spacing w:val=\"{}\"/>",
            (spacing * 20.0).round() as i32
        ));
    }
    out
}

fn highlight_name([r, g, b]: [u8; 3]) -> Option<&'static str> {
    Some(match [r, g, b] {
        [0xFF, 0xFF, 0x00] => "yellow",
        [0x00, 0xFF, 0x00] => "green",
        [0x00, 0xFF, 0xFF] => "cyan",
        [0xFF, 0x00, 0xFF] => "magenta",
        [0x00, 0x00, 0xFF] => "blue",
        [0xFF, 0x00, 0x00] => "red",
        [0x00, 0x00, 0x80] => "darkBlue",
        [0x00, 0x80, 0x80] => "darkCyan",
        [0x00, 0x80, 0x00] => "darkGreen",
        [0x80, 0x00, 0x80] => "darkMagenta",
        [0x80, 0x00, 0x00] => "darkRed",
        [0x80, 0x80, 0x00] => "darkYellow",
        [0x80, 0x80, 0x80] => "darkGray",
        [0xC0, 0xC0, 0xC0] => "lightGray",
        [0x00, 0x00, 0x00] => "black",
        [0xFF, 0xFF, 0xFF] => "white",
        _ => return None,
    })
}

/// `w:keepNext` and `w:keepLines` (CT_PPr order).
fn ppr_keep_xml(format: &ParaFormat) -> String {
    let mut out = String::new();
    if let Some(keep) = format.keep_with_next {
        out.push_str(if keep {
            "<w:keepNext/>"
        } else {
            "<w:keepNext w:val=\"0\"/>"
        });
    }
    if let Some(keep) = format.keep_together {
        out.push_str(if keep {
            "<w:keepLines/>"
        } else {
            "<w:keepLines w:val=\"0\"/>"
        });
    }
    out
}

/// `w:widowControl`, `w:numPr`, `w:pBdr`, `w:tabs` and `w:suppressAutoHyphens`
/// (CT_PPr order; page breaks and outlines are added by the caller).
fn ppr_middle_xml(format: &ParaFormat, list: Option<(u32, u8)>) -> String {
    let mut out = String::new();
    if let Some(widow) = format.widow_control {
        out.push_str(if widow {
            "<w:widowControl/>"
        } else {
            "<w:widowControl w:val=\"0\"/>"
        });
    }
    if let Some((num_id, level)) = list {
        out.push_str(&format!(
            "<w:numPr><w:ilvl w:val=\"{level}\"/><w:numId w:val=\"{num_id}\"/></w:numPr>"
        ));
    }
    if let Some(borders) = format.borders
        && borders.is_visible()
    {
        out.push_str("<w:pBdr>");
        let sides: [(&str, bool); 4] = [
            ("top", borders.sides.top),
            ("left", borders.sides.left),
            ("bottom", borders.sides.bottom),
            ("right", borders.sides.right),
        ];
        let [r, g, b, _] = borders.colour;
        for (name, present) in sides {
            if present {
                out.push_str(&format!(
                        "<w:{name} w:val=\"single\" w:sz=\"{}\" w:space=\"{}\" w:color=\"{r:02X}{g:02X}{b:02X}\"/>",
                        (borders.width_pt * 8.0).round().clamp(2.0, 96.0) as u32,
                        (borders.space_pt * 20.0).round() as i32
                    ));
            }
        }
        out.push_str("</w:pBdr>");
    }
    if let Some(tabs) = format.tab_stops
        && !tabs.is_empty()
    {
        out.push_str("<w:tabs>");
        for stop in tabs.iter() {
            let kind = match stop.kind {
                TabStopKind::Left => "left",
                TabStopKind::Centre => "center",
                TabStopKind::Right => "right",
                TabStopKind::Decimal => "decimal",
            };
            out.push_str(&format!(
                "<w:tab w:val=\"{kind}\" w:pos=\"{}\"/>",
                (stop.position_pt * 20.0).round() as i32
            ));
        }
        out.push_str("</w:tabs>");
    }
    if let Some(hyphenate) = format.hyphenate
        && !hyphenate
    {
        out.push_str("<w:suppressAutoHyphens/>");
    }
    out
}

/// `w:spacing`, `w:ind` and `w:jc` (CT_PPr order).
fn ppr_late_xml(format: &ParaFormat) -> String {
    let mut out = String::new();
    let mut spacing = String::new();
    if let Some(before) = format.space_before_pt {
        spacing.push_str(&format!(" w:before=\"{}\"", (before * 20.0).round() as i32));
    }
    if let Some(after) = format.space_after_pt {
        spacing.push_str(&format!(" w:after=\"{}\"", (after * 20.0).round() as i32));
    }
    if let Some(line) = format.line_spacing {
        let (value, rule) = match line {
            LineSpacing::Multiple(multiple) => ((multiple * 240.0).round() as i32, "auto"),
            LineSpacing::Exactly(points) => ((points * 20.0).round() as i32, "exact"),
            LineSpacing::AtLeast(points) => ((points * 20.0).round() as i32, "atLeast"),
            // "Single plus N points": Word has no direct equivalent, so write the equivalent
            // multiple for a 12 pt body (the document size is not known here).
            LineSpacing::Leading(points) => {
                ((240.0 + points / 14.4 * 240.0).round() as i32, "auto")
            }
        };
        spacing.push_str(&format!(" w:line=\"{value}\" w:lineRule=\"{rule}\""));
    }
    if !spacing.is_empty() {
        out.push_str(&format!("<w:spacing{spacing}/>"));
    }
    let mut indent = String::new();
    if let Some(left) = format.indent_left_pt {
        indent.push_str(&format!(" w:left=\"{}\"", (left * 20.0).round() as i32));
    }
    if let Some(right) = format.indent_right_pt {
        indent.push_str(&format!(" w:right=\"{}\"", (right * 20.0).round() as i32));
    }
    if let Some(first) = format.indent_first_pt {
        if first >= 0.0 {
            indent.push_str(&format!(
                " w:firstLine=\"{}\"",
                (first * 20.0).round() as i32
            ));
        } else {
            indent.push_str(&format!(
                " w:hanging=\"{}\"",
                (-first * 20.0).round() as i32
            ));
        }
    }
    if !indent.is_empty() {
        out.push_str(&format!("<w:ind{indent}/>"));
    }
    if let Some(align) = format.align {
        let value = match align {
            Align::Start => "left",
            Align::Centre => "center",
            Align::End => "right",
            Align::Justify => "both",
        };
        out.push_str(&format!("<w:jc w:val=\"{value}\"/>"));
    }
    out
}

// ------------------------------------------------------------------ sections

fn sect_pr(section: &Section, extra: &SectionExtras) -> String {
    let mut out = String::from("<w:sectPr>");
    let emit = |out: &mut String, name: &str, attrs: &[(String, String)]| {
        out.push_str(&format!("<w:{name}"));
        for (key, value) in attrs {
            // A header or footer reference points at its part with `r:id`; the reader keeps
            // only local names.
            let prefix = if key == "id" { "r" } else { "w" };
            out.push_str(&format!(" {prefix}:{key}=\"{}\"", escape_attr(value)));
        }
        out.push_str("/>");
    };
    // CT_SectPr order: header/footer references, pgSz, pgMar, pgNumType, cols, titlePg,
    // docGrid. Word repairs documents that put these out of order.
    for (name, attrs) in extra
        .elements
        .iter()
        .filter(|(name, _)| matches!(name.as_str(), "headerReference" | "footerReference"))
    {
        emit(&mut out, name, attrs);
    }

    let (width, height) = paper_points(section.page);
    let landscape = section.page.orientation == Orientation::Landscape;
    out.push_str(&format!(
        "<w:pgSz w:w=\"{}\" w:h=\"{}\"{}/>",
        (width * 20.0).round() as u32,
        (height * 20.0).round() as u32,
        if landscape {
            " w:orient=\"landscape\""
        } else {
            ""
        }
    ));
    let margins = section.page.margins;
    out.push_str(&format!(
        "<w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"0\"/>",
        (margins.top * 20.0).round() as i32,
        (margins.right * 20.0).round() as i32,
        (margins.bottom * 20.0).round() as i32,
        (margins.left * 20.0).round() as i32,
        (section.header_distance_pt * 20.0).round() as i32,
        (section.footer_distance_pt * 20.0).round() as i32,
    ));
    match extra.elements.iter().find(|(name, _)| name == "pgNumType") {
        Some((name, attrs)) => emit(&mut out, name, attrs),
        None => {
            if let Some(start) = section.first_page_number {
                out.push_str(&format!("<w:pgNumType w:start=\"{start}\"/>"));
            }
        }
    }
    if section.columns > 1 {
        out.push_str(&format!(
            "<w:cols w:num=\"{}\" w:space=\"{}\"/>",
            section.columns,
            (section.column_gutter_pt * 20.0).round() as i32
        ));
    }
    match extra.elements.iter().find(|(name, _)| name == "titlePg") {
        Some((name, attrs)) => emit(&mut out, name, attrs),
        None if section.headers.different_first || section.footers.different_first => {
            out.push_str("<w:titlePg/>");
        }
        None => {}
    }
    // Vertical text, right to left. CT_SectPr puts it after titlePg and before docGrid.
    if section.vertical {
        out.push_str("<w:textDirection w:val=\"tbRl\"/>");
    }
    if let Some((name, attrs)) = extra.elements.iter().find(|(name, _)| name == "docGrid") {
        emit(&mut out, name, attrs);
    }
    out.push_str("</w:sectPr>");
    out
}

pub(crate) fn paper_points(setup: PageSetup) -> (f32, f32) {
    let (width, height) = setup.paper.points();
    match setup.orientation {
        Orientation::Portrait => (width, height),
        Orientation::Landscape => (height, width),
    }
}

// ------------------------------------------------------------------ table styles

/// One table style definition to write, and the id tables refer to it by.
struct TableStyleDef {
    /// The model style as tables carry it.
    style: TableStyle,
    id: String,
    /// The name written; differs from `style.name` only when two different styles share a name.
    name: String,
}

#[derive(Default)]
struct TableStyles {
    defs: Vec<TableStyleDef>,
}

impl TableStyles {
    fn get(&self, style: &TableStyle) -> Option<&TableStyleDef> {
        self.defs.iter().find(|def| def.style == *style)
    }
}

/// One definition for each distinct table style in the document. Ids are `Tbl` plus the
/// name's letters and digits, made unique; Word needs unique names too, so a second style
/// with the same name but other fills gets a number after its name.
fn table_styles(document: &Document) -> TableStyles {
    let mut styles = TableStyles::default();
    let mut ids: HashSet<String> = HashSet::new();
    let mut names: HashSet<String> = HashSet::new();
    for block in document.body().iter() {
        let Block::Table(table) = block else {
            continue;
        };
        let Some(style) = &table.style else {
            continue;
        };
        if styles.get(style).is_some() {
            continue;
        }
        let letters: String = style
            .name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let base_id = format!("Tbl{letters}");
        let mut id = base_id.clone();
        let mut counter = 1u32;
        while !ids.insert(id.to_lowercase()) {
            counter += 1;
            id = format!("{base_id}{counter}");
        }
        let mut name = style.name.clone();
        let mut counter = 1u32;
        while !names.insert(name.to_lowercase()) {
            counter += 1;
            name = format!("{} {counter}", style.name);
        }
        styles.defs.push(TableStyleDef {
            style: style.clone(),
            id,
            name,
        });
    }
    styles
}

fn fill_hex([r, g, b, _]: [u8; 4]) -> String {
    format!("{r:02X}{g:02X}{b:02X}")
}

/// `w:tblLook`: which conditional parts of the style the table uses. The first row counts
/// when the table has a header row and the style paints something, so that Word starts the
/// bands after it, as the model does. Vertical banding is always off.
fn tbl_look_xml(table: &Table, style: &TableStyle) -> String {
    let first_row =
        table.header_rows > 0 && (style.header_fill.is_some() || style.band_fill.is_some());
    let no_h_band = style.band_fill.is_none();
    let value =
        (if first_row { 0x0020 } else { 0 }) | (if no_h_band { 0x0200 } else { 0 }) | 0x0400;
    format!(
        "<w:tblLook w:val=\"{value:04X}\" w:firstRow=\"{}\" w:lastRow=\"0\" w:firstColumn=\"0\" w:lastColumn=\"0\" w:noHBand=\"{}\" w:noVBand=\"1\"/>",
        u8::from(first_row),
        u8::from(no_h_band)
    )
}

/// Word's own default table style, which the styles below build on.
const TABLE_NORMAL_XML: &str = "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/><w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/><w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/><w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>";

/// `w:style w:type="table"`: the header fill on `firstRow`, and the band fill on `band2Horz`.
/// With the first row counted, Word numbers the first body row `band1Horz` and the second
/// `band2Horz`, so the model's "every second body row" is `band2Horz`; with no header row the
/// first row is `band1Horz`, which gives the same rows.
fn table_style_xml(def: &TableStyleDef) -> String {
    let shade = |fill: &str| {
        format!("<w:tcPr><w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{fill}\"/></w:tcPr>")
    };
    let mut out = format!(
        "<w:style w:type=\"table\" w:styleId=\"{}\"><w:name w:val=\"{}\"/><w:basedOn w:val=\"TableNormal\"/><w:uiPriority w:val=\"59\"/><w:tblPr><w:tblStyleRowBandSize w:val=\"1\"/><w:tblStyleColBandSize w:val=\"1\"/></w:tblPr>",
        escape_attr(&def.id),
        escape_attr(&def.name)
    );
    let style = &def.style;
    match (style.header_fill, style.band_fill) {
        (Some(header), _) => {
            out.push_str(&format!(
                "<w:tblStylePr w:type=\"firstRow\">{}</w:tblStylePr>",
                shade(&fill_hex(header))
            ));
        }
        // An explicit "no fill" first row keeps the first row out of the banding.
        (None, Some(_)) => out.push_str(&format!(
            "<w:tblStylePr w:type=\"firstRow\">{}</w:tblStylePr>",
            shade("auto")
        )),
        (None, None) => {}
    }
    if let Some(band) = style.band_fill {
        out.push_str(&format!(
            "<w:tblStylePr w:type=\"band2Horz\">{}</w:tblStylePr>",
            shade(&fill_hex(band))
        ));
    }
    out.push_str("</w:style>");
    out
}

// ------------------------------------------------------------------ styles.xml

fn styles_xml(styles: &StyleSheet, table_styles: &TableStyles, language: Option<&str>) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(XML_DECL);
    out.push_str(&format!("<w:styles xmlns:w=\"{W_NS}\">"));
    out.push_str("<w:docDefaults>");
    out.push_str("<w:rPrDefault><w:rPr>");
    let defaults = &styles.defaults;
    out.push_str(&char_props_xml(&CharFormat {
        family: Some(defaults.family.clone()),
        size_pt: Some(defaults.size_pt),
        weight: Some(defaults.weight),
        italic: Some(defaults.italic),
        underline: Some(defaults.underline),
        strikethrough: Some(defaults.strikethrough),
        colour: Some(defaults.colour),
        highlight: defaults.highlight.map(Some),
        vertical: Some(defaults.vertical),
        letter_spacing_pt: Some(defaults.letter_spacing_pt),
        ligatures: None,
    }));
    // The document's language; the tag came through `language_tag`, so it is safe to print.
    // `w:lang` is the last child Libertum writes here, as the schema's order wants.
    if let Some(language) = language {
        out.push_str(&format!("<w:lang w:val=\"{language}\"/>"));
    }
    out.push_str("</w:rPr></w:rPrDefault>");
    let para_defaults = &styles.para_defaults;
    let defaults_format = ParaFormat {
        align: Some(para_defaults.align),
        indent_left_pt: Some(para_defaults.indent_left_pt),
        indent_right_pt: Some(para_defaults.indent_right_pt),
        indent_first_pt: Some(para_defaults.indent_first_pt),
        space_before_pt: Some(para_defaults.space_before_pt),
        space_after_pt: Some(para_defaults.space_after_pt),
        line_spacing: Some(para_defaults.line_spacing),
        keep_with_next: Some(para_defaults.keep_with_next),
        keep_together: Some(para_defaults.keep_together),
        widow_control: Some(para_defaults.widow_control),
        hyphenate: Some(para_defaults.hyphenate),
        borders: para_defaults.borders,
        tab_stops: para_defaults.tab_stops,
    };
    out.push_str("<w:pPrDefault><w:pPr>");
    out.push_str(&ppr_keep_xml(&defaults_format));
    out.push_str(&ppr_middle_xml(&defaults_format, None));
    out.push_str(&ppr_late_xml(&defaults_format));
    out.push_str("</w:pPr></w:pPrDefault></w:docDefaults>");

    for (index, style) in styles.paragraph.iter().enumerate() {
        let id = StyleId(index as u32);
        let docx_id = style_docx_id(id);
        let (name, default) = match id {
            StyleId::BODY => ("Normal".to_owned(), " w:default=\"1\""),
            StyleId::TITLE => ("Title".to_owned(), ""),
            StyleId::HEADING_1 => ("heading 1".to_owned(), ""),
            StyleId::HEADING_2 => ("heading 2".to_owned(), ""),
            StyleId::HEADING_3 => ("heading 3".to_owned(), ""),
            StyleId::HEADING_4 => ("heading 4".to_owned(), ""),
            StyleId::QUOTE => ("Quote".to_owned(), ""),
            _ => (style.name.clone(), ""),
        };
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\"{default} w:styleId=\"{}\"><w:name w:val=\"{}\"/>",
            escape_attr(&docx_id),
            escape_attr(&name)
        ));
        if let Some(base) = style.base {
            out.push_str(&format!(
                "<w:basedOn w:val=\"{}\"/>",
                escape_attr(&style_docx_id(base))
            ));
        }
        if let Some(next) = style.next {
            out.push_str(&format!(
                "<w:next w:val=\"{}\"/>",
                escape_attr(&style_docx_id(next))
            ));
        }
        out.push_str("<w:qFormat/>");
        let mut para = ppr_keep_xml(&style.para);
        para.push_str(&ppr_middle_xml(&style.para, None));
        para.push_str(&ppr_late_xml(&style.para));
        if let Some(level) = style.outline_level {
            para.push_str(&format!(
                "<w:outlineLvl w:val=\"{}\"/>",
                level.saturating_sub(1)
            ));
        }
        if !para.is_empty() {
            out.push_str(&format!("<w:pPr>{para}</w:pPr>"));
        }
        out.push_str(&char_format_xml(&style.char));
        out.push_str("</w:style>");
    }
    if !table_styles.defs.is_empty() {
        out.push_str(TABLE_NORMAL_XML);
        for def in &table_styles.defs {
            out.push_str(&table_style_xml(def));
        }
    }
    out.push_str("</w:styles>");
    out
}

// ------------------------------------------------------------------ numbering.xml

fn list_style_code(style: ListStyle) -> u8 {
    match style {
        ListStyle::Bullet => 1,
        ListStyle::Numbered => 2,
        ListStyle::Lettered => 3,
        ListStyle::Harvard => 4,
    }
}

/// `(style, group)` to the `w:numId` written for it, one per list actually used.
fn list_ids(document: &Document) -> HashMap<(u8, u64), u32> {
    let mut ids = HashMap::new();
    let mut next = 1u32;
    for paragraph in document.paragraphs() {
        let Some(list) = paragraph.props.list else {
            continue;
        };
        let key = (list_style_code(list.style), list.group);
        ids.entry(key).or_insert_with(|| {
            let id = next;
            next += 1;
            id
        });
    }
    ids
}

fn numbering_xml(document: &Document) -> Option<String> {
    let ids = list_ids(document);
    if ids.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(4096);
    out.push_str(XML_DECL);
    out.push_str(&format!("<w:numbering xmlns:w=\"{W_NS}\">"));
    let mut ordered: Vec<((u8, u64), u32)> = ids.into_iter().collect();
    ordered.sort_by_key(|(_, id)| *id);
    for (index, ((code, _group), _id)) in ordered.iter().enumerate() {
        out.push_str(&format!("<w:abstractNum w:abstractNumId=\"{index}\">"));
        for level in 0..9u8 {
            let style = match code {
                1 => ListStyle::Bullet,
                2 => ListStyle::Numbered,
                3 => ListStyle::Lettered,
                _ => ListStyle::Harvard,
            };
            let (format, text) = level_format(style, level);
            out.push_str(&format!(
                "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{format}\"/>\
                 <w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/>\
                 <w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
                escape_attr(&text),
                360 * i32::from(level + 1)
            ));
        }
        out.push_str("</w:abstractNum>");
    }
    for (index, ((_, _group), id)) in ordered.iter().enumerate() {
        out.push_str(&format!(
            "<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"{index}\"/></w:num>"
        ));
    }
    out.push_str("</w:numbering>");
    Some(out)
}

fn level_format(style: ListStyle, level: u8) -> (&'static str, String) {
    match style {
        ListStyle::Bullet => {
            let glyph = match level % 3 {
                0 => "\u{2022}",
                1 => "\u{25E6}",
                _ => "\u{25AA}",
            };
            ("bullet", glyph.to_owned())
        }
        ListStyle::Numbered => ("decimal", format!("%{}.", level + 1)),
        ListStyle::Lettered => {
            let format = match level % 3 {
                0 => "upperLetter",
                1 => "lowerLetter",
                _ => "lowerRoman",
            };
            (format, format!("%{}.", level + 1))
        }
        ListStyle::Harvard => {
            let format = match level % 4 {
                0 => "upperRoman",
                1 => "upperLetter",
                2 => "decimal",
                _ => "lowerLetter",
            };
            (format, format!("%{}.", level + 1))
        }
    }
}
