//! Reading a DOCX package into the model (PLAN §6.2).
//!
//! The body is parsed as a stream: each `w:p` is read into a small tree and mapped, so a
//! 500-page document never needs a tree for the whole part. Unknown *package* parts are kept
//! by [`Package`]; body content the model cannot carry (drawings, fields, tracked changes,
//! nested or empty tables) is refused with [`DocxError::Unsupported`], because saving would
//! silently lose it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use libertum_formats_common::media::media_from_bytes;
use libertum_model::{
    Align, Block, BlockId, Bookmark, BorderSides, Borders, CharFormat, CrossRef, CrossRefKind,
    Document, InlineImage, Ligatures, LineSpacing, ListItem, ListStyle, MAX_LIST_LEVEL,
    MIN_TABLE_COLUMN_PT, Media, Note, NoteKind, Orientation, PaperSize, ParaFormat, ParaProps,
    Paragraph, ParagraphStyle, Run, Section, StyleId, StyleSheet, TabStop, TabStopKind, TabStops,
    Table, TableBorders, TableCell, TableRow, TableStyle, VerticalAlign,
};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::package::Package;
use crate::xml::{self, XmlNode};
use crate::{DocxError, Extras, SectionExtras};

pub(crate) struct Parsed {
    pub document: Document,
    pub extras: Extras,
    pub package: Package,
    pub warnings: Vec<String>,
}

pub(crate) fn read(bytes: &[u8]) -> Result<Parsed, DocxError> {
    let package = Package::read(bytes)?;
    let document_xml = package
        .text("word/document.xml")
        .ok_or(DocxError::NoDocument)?;
    let mut warnings = Vec::new();
    let styles = parse_styles(package.text("word/styles.xml").as_deref(), &mut warnings)?;
    let numbering = parse_numbering(package.text("word/numbering.xml").as_deref());
    let notes = parse_notes(&package, &mut warnings)?;
    let images = Images::new(&package, &document_xml);
    let body = parse_body(
        &document_xml,
        &styles,
        &numbering,
        &notes,
        &images,
        &mut warnings,
    )?;

    let mut blocks = body.blocks;
    let sections = body.sections;
    // A paragraph after a section break opens the next section.
    let mut section = 0usize;
    let mut open_next = false;
    for block in &mut blocks {
        let Some(paragraph) = block.as_paragraph_mut() else {
            continue;
        };
        if open_next {
            section = (section + 1).min(sections.len().saturating_sub(1));
            paragraph.props.section_start = true;
            open_next = false;
        }
        if body.break_after.contains(&paragraph.id) {
            open_next = true;
        }
    }

    let mut document = Document::from_blocks(blocks);
    document.styles = styles.sheet;
    document.sections = sections;

    Ok(Parsed {
        document,
        extras: body.extras,
        package,
        warnings,
    })
}

// ------------------------------------------------------------------ styles

struct StyleDef {
    docx_id: String,
    name: String,
    based_on: Option<String>,
    next: Option<String>,
    char: CharFormat,
    para: ParaFormat,
    outline: Option<u8>,
}

struct Styles {
    sheet: StyleSheet,
    ids: HashMap<String, StyleId>,
    /// Character styles (`w:rStyle`), already resolved through their `basedOn` chain.
    char_styles: HashMap<String, CharFormat>,
    /// Table styles by `w:styleId`, with the fills their conditional formatting gives.
    table_styles: HashMap<String, TableStyle>,
}

impl Styles {
    fn id(&self, docx_id: &str) -> Option<StyleId> {
        self.ids.get(docx_id).copied()
    }
}

struct CharStyleDef {
    based_on: Option<String>,
    format: CharFormat,
}

fn resolve_char_style(
    id: &str,
    defs: &HashMap<String, CharStyleDef>,
    out: &mut HashMap<String, CharFormat>,
    depth: u8,
) -> CharFormat {
    if let Some(format) = out.get(id) {
        return format.clone();
    }
    let Some(def) = defs.get(id) else {
        return CharFormat::default();
    };
    let base = match (&def.based_on, depth) {
        (Some(parent), depth) if depth < 16 => resolve_char_style(parent, defs, out, depth + 1),
        _ => CharFormat::default(),
    };
    let format = merge_char(&base, &def.format);
    out.insert(id.to_owned(), format.clone());
    format
}

/// `over` wins wherever it sets a property.
fn merge_char(base: &CharFormat, over: &CharFormat) -> CharFormat {
    CharFormat {
        family: over.family.clone().or_else(|| base.family.clone()),
        size_pt: over.size_pt.or(base.size_pt),
        weight: over.weight.or(base.weight),
        italic: over.italic.or(base.italic),
        underline: over.underline.or(base.underline),
        strikethrough: over.strikethrough.or(base.strikethrough),
        colour: over.colour.or(base.colour),
        highlight: over.highlight.or(base.highlight),
        vertical: over.vertical.or(base.vertical),
        letter_spacing_pt: over.letter_spacing_pt.or(base.letter_spacing_pt),
        ligatures: over.ligatures.or(base.ligatures),
    }
}

/// The model's built-in styles by the names Word uses for them.
fn builtin_by_name(name: &str) -> Option<StyleId> {
    match name.trim().to_ascii_lowercase().as_str() {
        "normal" | "body" => Some(StyleId::BODY),
        "title" => Some(StyleId::TITLE),
        "heading 1" | "heading1" => Some(StyleId::HEADING_1),
        "heading 2" | "heading2" => Some(StyleId::HEADING_2),
        "heading 3" | "heading3" => Some(StyleId::HEADING_3),
        "heading 4" | "heading4" => Some(StyleId::HEADING_4),
        "quote" => Some(StyleId::QUOTE),
        _ => None,
    }
}

fn builtin_ids() -> HashMap<String, StyleId> {
    [
        ("Normal", StyleId::BODY),
        ("Body", StyleId::BODY),
        ("Title", StyleId::TITLE),
        ("Heading1", StyleId::HEADING_1),
        ("Heading 1", StyleId::HEADING_1),
        ("Heading2", StyleId::HEADING_2),
        ("Heading 2", StyleId::HEADING_2),
        ("Heading3", StyleId::HEADING_3),
        ("Heading 3", StyleId::HEADING_3),
        ("Heading4", StyleId::HEADING_4),
        ("Heading 4", StyleId::HEADING_4),
        ("Quote", StyleId::QUOTE),
    ]
    .into_iter()
    .map(|(name, id)| (name.to_owned(), id))
    .collect()
}

fn parse_styles(xml_text: Option<&str>, warnings: &mut Vec<String>) -> Result<Styles, DocxError> {
    let mut sheet = StyleSheet::default();
    let mut ids = builtin_ids();
    let Some(xml_text) = xml_text else {
        return Ok(Styles {
            sheet,
            ids,
            char_styles: HashMap::new(),
            table_styles: HashMap::new(),
        });
    };
    let root = xml::parse(xml_text, "word/styles.xml")?;

    if let Some(defaults) = root.child("docDefaults") {
        if let Some(rpr) = defaults.child("rPrDefault").and_then(|d| d.child("rPr")) {
            sheet.defaults = sheet.defaults.apply(&parse_char_format(rpr));
        }
        if let Some(ppr) = defaults.child("pPrDefault").and_then(|d| d.child("pPr")) {
            sheet.para_defaults = sheet.para_defaults.apply(&parse_para_format(ppr));
        }
    }

    let mut defs: Vec<StyleDef> = Vec::new();
    for style in root.children_named("style") {
        if style.attr("type").unwrap_or("paragraph") != "paragraph" {
            continue;
        }
        let Some(docx_id) = style.attr("styleId") else {
            continue;
        };
        defs.push(StyleDef {
            docx_id: docx_id.to_owned(),
            name: style
                .child("name")
                .and_then(|n| n.attr("val"))
                .unwrap_or(docx_id)
                .to_owned(),
            based_on: style
                .child("basedOn")
                .and_then(|n| n.attr("val"))
                .map(str::to_owned),
            next: style
                .child("next")
                .and_then(|n| n.attr("val"))
                .map(str::to_owned),
            char: style
                .child("rPr")
                .map(parse_char_format)
                .unwrap_or_default(),
            para: style
                .child("pPr")
                .map(parse_para_format)
                .unwrap_or_default(),
            outline: style
                .child("pPr")
                .and_then(|ppr| ppr.child("outlineLvl"))
                .or_else(|| style.child("outlineLvl"))
                .and_then(|n| n.attr_u32("val"))
                .map(|level| (level as u8 + 1).min(9)),
        });
    }

    // First pass: give every style a model id. Built-in names keep their slot.
    for def in &defs {
        let target = builtin_by_name(&def.name).or(match def.docx_id.as_str() {
            "Normal" => Some(StyleId::BODY),
            "Title" => Some(StyleId::TITLE),
            "Heading1" => Some(StyleId::HEADING_1),
            "Heading2" => Some(StyleId::HEADING_2),
            "Heading3" => Some(StyleId::HEADING_3),
            "Heading4" => Some(StyleId::HEADING_4),
            "Quote" => Some(StyleId::QUOTE),
            _ => None,
        });
        let id = match target {
            Some(id) => id,
            None => {
                sheet.paragraph.push(ParagraphStyle {
                    name: def.name.clone(),
                    base: None,
                    char: CharFormat::default(),
                    para: ParaFormat::default(),
                    next: None,
                    outline_level: None,
                });
                StyleId(sheet.paragraph.len() as u32 - 1)
            }
        };
        ids.insert(def.docx_id.clone(), id);
    }

    // Second pass: fill in the definitions now that every id exists.
    for def in &defs {
        let Some(&id) = ids.get(&def.docx_id) else {
            continue;
        };
        if let Some(slot) = sheet.paragraph.get_mut(id.0 as usize) {
            if !def.name.is_empty() {
                slot.name = def.name.clone();
            }
            // The file's definition replaces the built-in one; properties it leaves out
            // inherit from `docDefaults`, as in Word.
            slot.char = def.char.clone();
            slot.para = def.para;
            slot.outline_level = def.outline;
            slot.base = def
                .based_on
                .as_deref()
                .and_then(|name| ids.get(name).copied());
            slot.next = def.next.as_deref().and_then(|name| ids.get(name).copied());
        }
        if let Some(name) = &def.based_on {
            if !ids.contains_key(name) {
                warnings.push(format!(
                    "style {:?} is based on unknown style {name:?}",
                    def.name
                ));
            }
        }
    }

    let mut char_defs: HashMap<String, CharStyleDef> = HashMap::new();
    for style in root.children_named("style") {
        if style.attr("type") != Some("character") {
            continue;
        }
        let Some(id) = style.attr("styleId") else {
            continue;
        };
        char_defs.insert(
            id.to_owned(),
            CharStyleDef {
                based_on: style
                    .child("basedOn")
                    .and_then(|n| n.attr("val"))
                    .map(str::to_owned),
                format: style
                    .child("rPr")
                    .map(parse_char_format)
                    .unwrap_or_default(),
            },
        );
    }
    let mut char_styles = HashMap::new();
    let keys: Vec<String> = char_defs.keys().cloned().collect();
    for id in keys {
        resolve_char_style(&id, &char_defs, &mut char_styles, 0);
    }

    let table_styles = parse_table_styles(&root);

    Ok(Styles {
        sheet,
        ids,
        char_styles,
        table_styles,
    })
}

/// The table styles of `styles.xml`. The header fill is the shading of `firstRow` and the band
/// fill that of `band2Horz` (the model's "every second body row"); both are looked up through
/// `basedOn`. Other conditional formatting (`band1Horz`, columns, corners) is ignored.
/// Word's default table style, and a name-less style, map to no style at all.
fn parse_table_styles(root: &XmlNode) -> HashMap<String, TableStyle> {
    struct Def<'a> {
        node: &'a XmlNode,
        based_on: Option<&'a str>,
    }
    let mut defs: HashMap<&str, Def<'_>> = HashMap::new();
    for style in root.children_named("style") {
        if style.attr("type") != Some("table") {
            continue;
        }
        let Some(id) = style.attr("styleId") else {
            continue;
        };
        defs.insert(
            id,
            Def {
                node: style,
                based_on: style.child("basedOn").and_then(|n| n.attr("val")),
            },
        );
    }
    let fill = |id: &str, condition: &str| -> Option<[u8; 4]> {
        let mut current = Some(id);
        for _ in 0..8 {
            let def = defs.get(current?)?;
            let own = def
                .node
                .children_named("tblStylePr")
                .find(|pr| pr.attr("type") == Some(condition))
                .and_then(|pr| pr.child("tcPr"))
                .and_then(|pr| pr.child("shd"));
            if let Some(shd) = own {
                return shd
                    .attr("fill")
                    .filter(|fill| *fill != "auto")
                    .and_then(parse_hex_colour);
            }
            current = def.based_on;
        }
        None
    };
    let mut out = HashMap::new();
    for (id, def) in &defs {
        let Some(name) = def.node.child("name").and_then(|n| n.attr("val")) else {
            continue;
        };
        if def.node.attr("default") == Some("1") || *id == "TableNormal" {
            continue;
        }
        out.insert(
            (*id).to_owned(),
            TableStyle {
                name: name.to_owned(),
                header_fill: fill(id, "firstRow"),
                band_fill: fill(id, "band2Horz"),
            },
        );
    }
    out
}

// ------------------------------------------------------------------ numbering

struct Numbering {
    /// `w:numId` to the model's list style and group.
    nums: HashMap<String, (ListStyle, u64)>,
}

fn parse_numbering(xml_text: Option<&str>) -> Numbering {
    let mut numbering = Numbering {
        nums: HashMap::new(),
    };
    let Some(xml_text) = xml_text else {
        return numbering;
    };
    let Ok(root) = xml::parse(xml_text, "word/numbering.xml") else {
        return numbering;
    };
    let mut abstract_styles: HashMap<String, ListStyle> = HashMap::new();
    for abstract_num in root.children_named("abstractNum") {
        let Some(id) = abstract_num.attr("abstractNumId") else {
            continue;
        };
        let format = abstract_num
            .child("lvl")
            .and_then(|lvl| lvl.child("numFmt"))
            .and_then(|fmt| fmt.attr("val"))
            .unwrap_or("decimal");
        abstract_styles.insert(id.to_owned(), list_style(format));
    }
    let mut group = 0u64;
    for num in root.children_named("num") {
        let Some(num_id) = num.attr("numId") else {
            continue;
        };
        let style = num
            .child("abstractNumId")
            .and_then(|n| n.attr("val"))
            .and_then(|id| abstract_styles.get(id))
            .copied()
            .unwrap_or(ListStyle::Numbered);
        group += 1;
        numbering.nums.insert(num_id.to_owned(), (style, group));
    }
    numbering
}

fn list_style(format: &str) -> ListStyle {
    match format.to_ascii_lowercase().as_str() {
        "bullet" => ListStyle::Bullet,
        "upperletter" | "lowerletter" => ListStyle::Lettered,
        "upperroman" | "lowerroman" => ListStyle::Harvard,
        _ => ListStyle::Numbered,
    }
}

// ------------------------------------------------------------------ notes

/// Note bodies by their DOCX id, one table per kind. Separator entries are dropped.
#[derive(Default)]
struct Notes {
    footnotes: HashMap<u32, String>,
    endnotes: HashMap<u32, String>,
}

impl Notes {
    fn text(&self, kind: NoteKind, id: u32) -> Option<&str> {
        match kind {
            NoteKind::Footnote => self.footnotes.get(&id),
            NoteKind::Endnote => self.endnotes.get(&id),
        }
        .map(String::as_str)
    }
}

fn note_kind_name(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Footnote => "footnote",
        NoteKind::Endnote => "endnote",
    }
}

fn parse_notes(package: &Package, warnings: &mut Vec<String>) -> Result<Notes, DocxError> {
    let mut notes = Notes::default();
    for kind in [NoteKind::Footnote, NoteKind::Endnote] {
        let Some(part) = note_part(package, kind) else {
            continue;
        };
        let Some(text) = package.text(&part) else {
            continue;
        };
        let parsed = parse_note_part(&text, &part, kind, warnings)?;
        match kind {
            NoteKind::Footnote => notes.footnotes = parsed,
            NoteKind::Endnote => notes.endnotes = parsed,
        }
    }
    Ok(notes)
}

/// The notes part a document relationship points at, falling back to Word's usual name when
/// the package has no relationship part.
fn note_part(package: &Package, kind: NoteKind) -> Option<String> {
    let (suffix, default) = match kind {
        NoteKind::Footnote => ("/footnotes", "word/footnotes.xml"),
        NoteKind::Endnote => ("/endnotes", "word/endnotes.xml"),
    };
    package.related_part(suffix, default)
}

fn parse_note_part(
    xml_text: &str,
    part: &str,
    kind: NoteKind,
    warnings: &mut Vec<String>,
) -> Result<HashMap<u32, String>, DocxError> {
    let root = xml::parse(xml_text, part)?;
    let element = match kind {
        NoteKind::Footnote => "footnote",
        NoteKind::Endnote => "endnote",
    };
    let mut notes = HashMap::new();
    for note in root.children_named(element) {
        // Word writes a separator and a continuation separator before the real notes; both are
        // layout artefacts, not content.
        match note.attr("type") {
            None | Some("normal") => {}
            Some(_) => continue,
        }
        let Some(id) = note.attr("id").and_then(|value| value.parse::<i64>().ok()) else {
            continue;
        };
        if id < 1 {
            continue;
        }
        let (text, rich) = note_body(note);
        if rich {
            warnings.push(format!(
                "a {} with formatting or several paragraphs was flattened to plain text",
                note_kind_name(kind)
            ));
        }
        if let Ok(id) = u32::try_from(id) {
            notes.insert(id, text);
        }
    }
    Ok(notes)
}

/// A note body as plain text, plus whether anything beyond plain text was dropped.
fn note_body(note: &XmlNode) -> (String, bool) {
    let mut text = String::new();
    let mut rich = false;
    let mut has_reference = false;
    let mut paragraphs = 0usize;
    for child in note.elements() {
        match child.name.as_str() {
            "p" => {
                paragraphs += 1;
                if paragraphs > 1 {
                    rich = true;
                    if !text.is_empty() && !text.ends_with(' ') {
                        text.push(' ');
                    }
                }
                collect_note_text(child, &mut text, &mut rich, &mut has_reference);
            }
            "tbl" | "sdt" | "customXml" => {
                rich = true;
                collect_note_text(child, &mut text, &mut rich, &mut has_reference);
            }
            _ => {}
        }
    }
    // Word puts a leading space after the automatic note marker; it is not part of the note.
    if has_reference {
        if let Some(stripped) = text.strip_prefix(' ') {
            text = stripped.to_owned();
        }
    }
    (text, rich)
}

fn collect_note_text(node: &XmlNode, out: &mut String, rich: &mut bool, has_reference: &mut bool) {
    for child in node.elements() {
        match child.name.as_str() {
            "t" => out.push_str(&child.text()),
            "tab" => out.push('\t'),
            "br" | "cr" => out.push(' '),
            "noBreakHyphen" => out.push('\u{2011}'),
            "softHyphen" | "instrText" | "pPr" => {}
            "footnoteRef" | "endnoteRef" => *has_reference = true,
            "del" | "delText" | "drawing" | "pict" | "object" => *rich = true,
            "hyperlink" | "smartTag" | "fldSimple" | "ins" => {
                *rich = true;
                collect_note_text(child, out, rich, has_reference);
            }
            "rPr" => {
                // A note marker's style is not formatting the model would lose.
                if child
                    .elements()
                    .any(|element| !matches!(element.name.as_str(), "rStyle" | "vertAlign"))
                {
                    *rich = true;
                }
            }
            _ => collect_note_text(child, out, rich, has_reference),
        }
    }
}

// ------------------------------------------------------------------ pictures and fields

fn unsupported(what: &str) -> DocxError {
    DocxError::Unsupported {
        part: "word/document.xml".to_owned(),
        what: what.to_owned(),
    }
}

/// What reading pictures and cross-references needs from the package.
struct Images<'a> {
    package: &'a Package,
    /// Image relationship ids of `document.xml` and the parts they point at.
    parts: HashMap<String, String>,
    decoded: RefCell<HashMap<String, Arc<Media>>>,
    /// Lower-case names of the bookmarks `REF` and `PAGEREF` fields point at. Such a bookmark
    /// is kept even when Word hides it (`_Ref...`), or the reference would lose its target.
    ref_targets: HashSet<String>,
}

impl<'a> Images<'a> {
    fn new(package: &'a Package, document_xml: &str) -> Self {
        let parts = package
            .document_relationships()
            .into_iter()
            .filter(|relationship| relationship.kind.ends_with("/image"))
            .filter_map(|relationship| Some((relationship.id, relationship.part?)))
            .collect();
        Self {
            package,
            parts,
            decoded: RefCell::new(HashMap::new()),
            ref_targets: collect_ref_targets(document_xml),
        }
    }

    /// The image behind a relationship id, and the name of its part.
    fn media(&self, rel_id: &str) -> Result<(Arc<Media>, String), DocxError> {
        let part = self
            .parts
            .get(rel_id)
            .ok_or_else(|| unsupported("a picture whose image part is missing"))?;
        if let Some(media) = self.decoded.borrow().get(part) {
            return Ok((Arc::clone(media), part.clone()));
        }
        let bytes = self
            .package
            .get(part)
            .ok_or_else(|| unsupported("a picture whose image part is missing"))?;
        let media = media_from_bytes(bytes.to_vec())
            .map_err(|_| unsupported("a picture in an image format Libertum cannot read"))?;
        self.decoded
            .borrow_mut()
            .insert(part.clone(), Arc::clone(&media));
        Ok((media, part.clone()))
    }
}

/// What a field instruction means to the model.
enum Instruction {
    CrossRef { target: String, kind: CrossRefKind },
    Other,
}

/// Splits an instruction into words; double quotes group a word that has spaces.
fn instruction_words(instruction: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    for character in instruction.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    words
}

/// `REF name` and `PAGEREF name`, with only the `\h` (hyperlink) and `\*` (result format)
/// switches. Other switches change what the field shows (paragraph numbers, relative
/// position), which a cross-reference of the model cannot say, so they stay refused.
fn parse_instruction(instruction: &str) -> Instruction {
    let words = instruction_words(instruction);
    let mut words = words.iter();
    let kind = match words.next().map(|word| word.to_ascii_uppercase()) {
        Some(word) if word == "REF" => CrossRefKind::Text,
        Some(word) if word == "PAGEREF" => CrossRefKind::Page,
        _ => return Instruction::Other,
    };
    let mut target: Option<String> = None;
    while let Some(word) = words.next() {
        if let Some(switch) = word.strip_prefix('\\') {
            match switch.to_ascii_lowercase().as_str() {
                "h" => {}
                "*" => {
                    words.next();
                }
                _ => return Instruction::Other,
            }
        } else if target.is_none() {
            target = Some(word.clone());
        } else {
            return Instruction::Other;
        }
    }
    match target {
        Some(target) if !target.is_empty() => Instruction::CrossRef { target, kind },
        _ => Instruction::Other,
    }
}

/// The bookmark names the document's `REF` and `PAGEREF` fields point at, found with one pass
/// over `document.xml` before the body is read.
fn collect_ref_targets(xml_text: &str) -> HashSet<String> {
    let mut targets = HashSet::new();
    let mut note = |instruction: &str| {
        if let Instruction::CrossRef { target, .. } = parse_instruction(instruction) {
            targets.insert(target.to_lowercase());
        }
    };
    let mut reader = Reader::from_str(xml_text);
    let mut instruction = String::new();
    let mut in_instruction = false;
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Start(start) | Event::Empty(start) => match start.local_name().as_ref() {
                "instrText" => in_instruction = true,
                "fldSimple" => {
                    for attribute in start.attributes().flatten() {
                        if attribute.key.local_name().as_ref() == "instr" {
                            if let Ok(value) =
                                attribute.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                            {
                                note(&value);
                            }
                        }
                    }
                }
                "fldChar" if !instruction.is_empty() => {
                    note(&instruction);
                    instruction.clear();
                }
                _ => {}
            },
            Event::End(end) if end.local_name().as_ref() == "instrText" => in_instruction = false,
            Event::Text(text) if in_instruction => {
                let raw = text.into_inner();
                if let Ok(value) = quick_xml::escape::unescape(&raw) {
                    instruction.push_str(&value);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    targets
}

/// Where a paragraph's complex-field state machine is.
#[derive(Default)]
enum FieldState {
    #[default]
    Outside,
    /// Between `begin` and `separate`: collecting the instruction.
    Instruction(String),
    /// Between `separate` and `end`: the runs are the cross-reference's cached result.
    Result {
        target: String,
        kind: CrossRefKind,
        segment: usize,
        start: usize,
    },
}

/// Adds the cross-reference whose result is the text of segment `index` from `start` to its
/// end. A result that is empty or that crosses a page break cannot be one text range.
fn finish_cross_ref(
    segments: &mut [Segment],
    index: usize,
    start: usize,
    target: String,
    kind: CrossRefKind,
) -> Result<(), DocxError> {
    if index + 1 != segments.len() {
        return Err(unsupported("a cross-reference that spans a page break"));
    }
    let segment = &mut segments[index];
    let end = segment.text.len();
    if end <= start {
        return Err(unsupported("a cross-reference with no result text"));
    }
    segment.cross_refs.push(CrossRef {
        range: start..end,
        target,
        kind,
    });
    Ok(())
}

/// Reads an inline picture. Floating drawings, shapes, charts, cropped or rotated pictures
/// and linked images are refused: saving would lose what they show.
fn read_drawing(drawing: &XmlNode, images: &Images<'_>) -> Result<InlineImage, DocxError> {
    if drawing.child("anchor").is_some() {
        return Err(unsupported("a floating picture or drawing"));
    }
    let inline = drawing
        .child("inline")
        .ok_or_else(|| unsupported("a picture or drawing"))?;
    let pic = inline
        .child("graphic")
        .and_then(|graphic| graphic.child("graphicData"))
        .and_then(|data| data.child("pic"))
        .ok_or_else(|| unsupported("a drawing that is not a picture"))?;
    let blip_fill = pic.child("blipFill");
    let blip = blip_fill
        .and_then(|fill| fill.child("blip"))
        .ok_or_else(|| unsupported("a picture without an image"))?;
    let embed = blip
        .attr("embed")
        .filter(|id| !id.is_empty())
        .ok_or_else(|| unsupported("a linked picture"))?;
    let cropped = blip_fill
        .and_then(|fill| fill.child("srcRect"))
        .is_some_and(|rect| {
            ["l", "t", "r", "b"]
                .iter()
                .any(|side| rect.attr_f32(side).is_some_and(|value| value != 0.0))
        });
    if cropped {
        return Err(unsupported("a cropped picture"));
    }
    let rotated = pic
        .child("spPr")
        .and_then(|properties| properties.child("xfrm"))
        .and_then(|transform| transform.attr_f32("rot"))
        .is_some_and(|rotation| rotation != 0.0);
    if rotated {
        return Err(unsupported("a rotated picture"));
    }
    let (media, part) = images.media(embed)?;

    let (natural_width, natural_height) = media.natural_size_pt();
    let extent = inline.child("extent");
    let size = |name: &str, natural: f32| {
        extent
            .and_then(|extent| extent.attr_f32(name))
            .filter(|emu| *emu > 0.0)
            .map_or(natural, |emu| emu / 12_700.0)
    };
    let doc_pr = inline.child("docPr");
    let alt = doc_pr
        .and_then(|doc_pr| doc_pr.attr("descr"))
        .unwrap_or_default()
        .to_owned();
    // The picture's own name (Word puts the file name there) wins, even when empty; then the
    // drawing's name unless it is Word's generic "Picture 1"; then the part's file name.
    let generic = |name: &str| {
        let lower = name.to_ascii_lowercase();
        ["picture ", "image ", "graphic "].iter().any(|prefix| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
        })
    };
    let name = match pic
        .child("nvPicPr")
        .and_then(|properties| properties.child("cNvPr"))
        .and_then(|properties| properties.attr("name"))
    {
        Some(name) => name.to_owned(),
        None => match doc_pr.and_then(|doc_pr| doc_pr.attr("name")) {
            Some(name) if !name.is_empty() && !generic(name) => name.to_owned(),
            _ => part.rsplit('/').next().unwrap_or_default().to_owned(),
        },
    };
    let mut image = InlineImage {
        offset: 0,
        media,
        width_pt: size("cx", natural_width),
        height_pt: size("cy", natural_height),
        alt,
        name,
    };
    image.clamp_size();
    Ok(image)
}

// ------------------------------------------------------------------ the body

struct Body {
    blocks: Vec<Block>,
    sections: Vec<Section>,
    extras: Extras,
    /// Model ids of paragraphs that end a section.
    break_after: Vec<BlockId>,
}

fn parse_body(
    xml_text: &str,
    styles: &Styles,
    numbering: &Numbering,
    notes: &Notes,
    images: &Images<'_>,
    warnings: &mut Vec<String>,
) -> Result<Body, DocxError> {
    const PART: &str = "word/document.xml";
    let mut reader = Reader::from_str(xml_text);
    let mut in_body = false;
    let mut blocks: Vec<Block> = Vec::new();
    let mut next_id = 1u64;
    let mut sections: Vec<Section> = Vec::new();
    let mut extras: Extras = Vec::new();
    let mut body_sect: Option<XmlNode> = None;
    let mut break_after = Vec::new();

    loop {
        match reader.read_event().map_err(|e| xml::xml_error(PART, e))? {
            Event::Start(start) if start.local_name().as_ref() == "body" => in_body = true,
            Event::End(end) if end.local_name().as_ref() == "body" => in_body = false,
            Event::Start(start) if in_body => {
                let name = start.local_name().as_ref().to_owned();
                match name.as_str() {
                    "p" => {
                        let node = xml::read_node(&mut reader, &start, PART)?;
                        let (mut parts, sect) =
                            parse_paragraph(&node, styles, numbering, notes, images, warnings)?;
                        for paragraph in &mut parts {
                            paragraph.id = BlockId(next_id);
                            next_id += 1;
                        }
                        if let Some(sect) = sect {
                            let (section, extra) = parse_section(&sect, warnings);
                            sections.push(section);
                            extras.push(extra);
                            if let Some(last) = parts.last() {
                                break_after.push(last.id);
                            }
                        }
                        blocks.extend(parts.into_iter().map(Block::Paragraph));
                    }
                    "tbl" => {
                        let node = xml::read_node(&mut reader, &start, PART)?;
                        let table = parse_table(
                            &node,
                            styles,
                            numbering,
                            notes,
                            images,
                            warnings,
                            &mut next_id,
                        )?;
                        blocks.push(Block::Table(table));
                    }
                    "sectPr" => body_sect = Some(xml::read_node(&mut reader, &start, PART)?),
                    "sdt" | "customXml" | "altChunk" => {
                        return Err(DocxError::Unsupported {
                            part: PART.to_owned(),
                            what: format!("a {name} block"),
                        });
                    }
                    _ => {
                        let _ = xml::read_node(&mut reader, &start, PART)?;
                    }
                }
            }
            Event::Empty(start) if in_body => {
                let name = start.local_name();
                if matches!(name.as_ref(), "tbl" | "drawing" | "pict" | "object") {
                    return Err(DocxError::Unsupported {
                        part: PART.to_owned(),
                        what: format!("<{}>", name.as_ref()),
                    });
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    // The final section is the body-level `w:sectPr` (Word always writes one).
    if let Some(sect) = body_sect {
        let (section, extra) = parse_section(&sect, warnings);
        sections.push(section);
        extras.push(extra);
    }
    if sections.is_empty() {
        sections.push(Section::default());
        extras.push(SectionExtras::default());
    }

    Ok(Body {
        blocks,
        sections,
        extras,
        break_after,
    })
}

/// Parses a section's `w:sectPr` into the model and the XML we must write back unchanged.
fn parse_section(node: &XmlNode, warnings: &mut Vec<String>) -> (Section, SectionExtras) {
    let mut section = Section::default();
    let mut extra = SectionExtras::default();

    if let Some(size) = node.child("pgSz") {
        if let (Some(width), Some(height)) = (
            size.attr_f32("w").map(|twips| twips / 20.0),
            size.attr_f32("h").map(|twips| twips / 20.0),
        ) {
            section.page.orientation = if size
                .attr("orient")
                .is_some_and(|value| value.eq_ignore_ascii_case("landscape"))
            {
                Orientation::Landscape
            } else {
                Orientation::Portrait
            };
            let (long, short) = if width >= height {
                (width, height)
            } else {
                (height, width)
            };
            let mut best = PaperSize::A4;
            let mut best_delta = f32::MAX;
            for paper in PaperSize::ALL {
                let (pw, ph) = paper.points();
                let (paper_long, paper_short) = if pw >= ph { (pw, ph) } else { (ph, pw) };
                let delta = (paper_long - long).abs() + (paper_short - short).abs();
                if delta < best_delta {
                    best_delta = delta;
                    best = paper;
                }
            }
            if best_delta > 8.0 {
                warnings.push(format!(
                    "page size {width:.0}x{height:.0} pt is not a standard paper; using {best:?}"
                ));
            }
            section.page.paper = best;
        }
    }

    if let Some(margin) = node.child("pgMar") {
        let pt = |name: &str| margin.attr_f32(name).map(|twips| twips / 20.0);
        if let Some(top) = pt("top") {
            section.page.margins.top = top;
        }
        if let Some(right) = pt("right") {
            section.page.margins.right = right;
        }
        if let Some(bottom) = pt("bottom") {
            section.page.margins.bottom = bottom;
        }
        if let Some(left) = pt("left") {
            section.page.margins.left = left;
        }
        if let Some(header) = pt("header") {
            section.header_distance_pt = header;
        }
        if let Some(footer) = pt("footer") {
            section.footer_distance_pt = footer;
        }
    }

    if let Some(cols) = node.child("cols") {
        if let Some(count) = cols.attr_u32("num") {
            section.set_columns(count.min(u32::from(libertum_model::MAX_COLUMNS)) as u8);
        }
        if let Some(space) = cols.attr_f32("space") {
            section.set_column_gutter(space / 20.0);
        }
    }

    if let Some(start) = node.child("pgNumType").and_then(|n| n.attr_u32("start")) {
        section.first_page_number = Some(start);
    }

    if node.child("titlePg").is_some() {
        section.headers.different_first = true;
        section.footers.different_first = true;
    }

    // Raw elements we do not model yet: keep them so a save does not drop header/footer
    // references, grid settings or page-number formats.
    for child in node.elements() {
        match child.name.as_str() {
            "headerReference" | "footerReference" | "pgNumType" | "titlePg" | "docGrid" => {
                extra
                    .elements
                    .push((child.name.clone(), child.attrs.clone()));
            }
            _ => {}
        }
    }

    (section, extra)
}

// ------------------------------------------------------------------ paragraphs

struct Segment {
    text: String,
    runs: Vec<Run>,
    notes: Vec<Note>,
    bookmarks: Vec<Bookmark>,
    images: Vec<InlineImage>,
    cross_refs: Vec<CrossRef>,
    starts_page: bool,
}

impl Segment {
    fn new(starts_page: bool) -> Self {
        Self {
            text: String::new(),
            runs: Vec::new(),
            notes: Vec::new(),
            bookmarks: Vec::new(),
            images: Vec::new(),
            cross_refs: Vec::new(),
            starts_page,
        }
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
            && self.runs.is_empty()
            && self.notes.is_empty()
            && self.bookmarks.is_empty()
            && self.images.is_empty()
    }
}

/// Parses a `w:tbl` into a table block, allocating ids for the table and its cell
/// paragraphs. Nested tables are refused (the model does not nest them).
fn parse_table(
    node: &XmlNode,
    styles: &Styles,
    numbering: &Numbering,
    notes: &Notes,
    images: &Images<'_>,
    warnings: &mut Vec<String>,
    next_id: &mut u64,
) -> Result<Table, DocxError> {
    const PART: &str = "word/document.xml";
    let tblpr = node.child("tblPr");
    let borders = parse_table_borders(tblpr);
    let mut widths: Vec<f32> = node
        .child("tblGrid")
        .map(|grid| {
            grid.children_named("gridCol")
                .filter_map(|col| col.attr_f32("w"))
                .map(|twips| (twips / 20.0).max(MIN_TABLE_COLUMN_PT))
                .collect()
        })
        .unwrap_or_default();

    let mut rows = Vec::new();
    let mut header_rows = 0u8;
    let mut still_header = true;
    for row in node.children_named("tr") {
        let trpr = row.child("trPr");
        let header = trpr.and_then(|p| p.child("tblHeader")).is_some();
        if still_header && header {
            header_rows = header_rows.saturating_add(1);
        } else if !header {
            still_header = false;
        }
        // Word's default is a row that may break across pages.
        let keep_together = trpr
            .and_then(|p| p.child("cantSplit"))
            .is_some_and(|n| n.on());
        let min_height_pt = trpr
            .and_then(|p| p.child("trHeight"))
            .and_then(|h| h.attr_f32("val"))
            .map_or(0.0, |twips| twips / 20.0);
        let mut cells = Vec::new();
        for cell in row.children_named("tc") {
            let tcpr = cell.child("tcPr");
            let span = tcpr
                .and_then(|p| p.child("gridSpan"))
                .and_then(|n| n.attr_u32("val"))
                .map_or(1u8, |v| v.clamp(1, 63) as u8);
            // A vMerge continuation cell is covered by the cell above; "restart" starts a new
            // merge and is an ordinary cell for the model.
            let merged_into_above = tcpr
                .and_then(|p| p.child("vMerge"))
                .is_some_and(|n| n.attr("val") != Some("restart"));
            let fill = tcpr
                .and_then(|p| p.child("shd"))
                .and_then(|n| n.attr("fill"))
                .filter(|fill| *fill != "auto")
                .and_then(parse_hex_colour);
            let mut paragraphs = Vec::new();
            for child in cell.elements() {
                match child.name.as_str() {
                    "tcPr" => {}
                    "p" => {
                        let (mut parts, _sect) =
                            parse_paragraph(child, styles, numbering, notes, images, warnings)?;
                        for paragraph in &mut parts {
                            paragraph.id = BlockId(*next_id);
                            *next_id += 1;
                        }
                        paragraphs.append(&mut parts);
                    }
                    "tbl" => {
                        return Err(DocxError::Unsupported {
                            part: PART.to_owned(),
                            what: "a nested table".to_owned(),
                        });
                    }
                    _ => {}
                }
            }
            if paragraphs.is_empty() {
                paragraphs.push(Paragraph::new(BlockId(*next_id), ""));
                *next_id += 1;
            }
            cells.push(TableCell {
                col_span: span,
                merged_into_above,
                paragraphs,
                fill,
            });
        }
        if cells.is_empty() {
            continue;
        }
        rows.push(TableRow {
            cells,
            min_height_pt,
            keep_together,
        });
    }

    if rows.is_empty() {
        return Err(DocxError::Unsupported {
            part: PART.to_owned(),
            what: "an empty table".to_owned(),
        });
    }
    let columns = rows.iter().map(TableRow::span).max().unwrap_or(1).max(1);
    if widths.len() < columns {
        let total = tblpr
            .and_then(|p| p.child("tblW"))
            .and_then(|w| w.attr_f32("w"))
            .map_or(72.0 * columns as f32, |twips| twips / 20.0);
        let each = (total / columns as f32).max(MIN_TABLE_COLUMN_PT);
        widths.resize(columns, each);
    }

    let id = BlockId(*next_id);
    *next_id += 1;
    let style = match tblpr
        .and_then(|p| p.child("tblStyle"))
        .and_then(|n| n.attr("val"))
    {
        Some(id) => match styles.table_styles.get(id) {
            Some(style) => Some(style.clone()),
            None => {
                if id != "TableNormal" {
                    warnings.push(format!(
                        "table style {id:?} has no definition and was dropped"
                    ));
                }
                None
            }
        },
        None => None,
    };
    let mut table = Table {
        style,
        id,
        widths_pt: widths,
        rows,
        header_rows,
        borders,
    };
    table.normalise();
    Ok(table)
}

/// Table borders from `w:tblPr/w:tblBorders`; Word's default when the element is absent.
fn parse_table_borders(tblpr: Option<&XmlNode>) -> TableBorders {
    let Some(borders) = tblpr.and_then(|p| p.child("tblBorders")) else {
        return TableBorders::default();
    };
    let present = |name: &str| {
        borders.child(name).is_some_and(|side| {
            side.attr("val")
                .is_none_or(|val| !matches!(val, "none" | "nil"))
        })
    };
    let outer = ["top", "left", "bottom", "right"]
        .iter()
        .any(|name| present(name));
    let inner = ["insideH", "insideV"].iter().any(|name| present(name));
    if !outer && !inner {
        return TableBorders::none();
    }
    let mut width_pt = 0.0f32;
    let mut colour = None;
    for name in ["top", "left", "bottom", "right", "insideH", "insideV"] {
        let Some(side) = borders.child(name) else {
            continue;
        };
        if side
            .attr("val")
            .is_some_and(|val| matches!(val, "none" | "nil"))
        {
            continue;
        }
        // `w:sz` is in eighths of a point.
        width_pt = width_pt.max(side.attr_f32("sz").map_or(0.0, |sz| sz / 8.0));
        colour = colour.or_else(|| side.attr("color").and_then(parse_hex_colour));
    }
    TableBorders {
        width_pt,
        colour: colour.unwrap_or([0, 0, 0, 255]),
        outer,
        inner,
    }
}

fn parse_paragraph(
    node: &XmlNode,
    styles: &Styles,
    numbering: &Numbering,
    notes: &Notes,
    images: &Images<'_>,
    warnings: &mut Vec<String>,
) -> Result<(Vec<Paragraph>, Option<XmlNode>), DocxError> {
    let ppr = node.child("pPr");
    let sect = ppr.and_then(|p| p.child("sectPr")).cloned();
    let props = parse_props(ppr, styles, numbering);

    let mut segments = vec![Segment::new(props.page_break_before)];
    let mut field = FieldState::Outside;

    for child in node.elements() {
        match child.name.as_str() {
            "pPr" => {}
            "r" => parse_run(
                child,
                styles,
                &mut segments,
                notes,
                images,
                &mut field,
                warnings,
            )?,
            "bookmarkStart" => {
                let name = child.attr("name").unwrap_or_default();
                // Word's hidden bookmarks start with an underscore (`_GoBack`, `_Toc...`); keep
                // one only when a cross-reference points at it.
                let wanted =
                    !name.starts_with('_') || images.ref_targets.contains(&name.to_lowercase());
                if !name.is_empty() && wanted {
                    let segment = segments.last_mut().expect("segment");
                    let offset = segment.text.len();
                    segment.bookmarks.push(Bookmark {
                        name: name.to_owned(),
                        offset,
                    });
                }
            }
            "bookmarkEnd" => {}
            "hyperlink" | "smartTag" | "customXml" => {
                for run in child.children_named("r") {
                    parse_run(
                        run,
                        styles,
                        &mut segments,
                        notes,
                        images,
                        &mut field,
                        warnings,
                    )?;
                }
                warnings.push("a hyperlink or content control was flattened to text".to_owned());
            }
            "ins" | "del" => {
                return Err(DocxError::Unsupported {
                    part: "word/document.xml".to_owned(),
                    what: "tracked changes".to_owned(),
                });
            }
            "fldSimple" => {
                let Instruction::CrossRef { target, kind } =
                    parse_instruction(child.attr("instr").unwrap_or_default())
                else {
                    return Err(unsupported("a field"));
                };
                let index = segments.len() - 1;
                let start = segments[index].text.len();
                for run in child.children_named("r") {
                    parse_run(
                        run,
                        styles,
                        &mut segments,
                        notes,
                        images,
                        &mut field,
                        warnings,
                    )?;
                }
                finish_cross_ref(&mut segments, index, start, target, kind)?;
            }
            _ => {}
        }
    }
    if !matches!(field, FieldState::Outside) {
        return Err(unsupported("a field"));
    }
    if segments.len() > 1 && segments.last().is_some_and(Segment::is_empty) {
        // A page break at the very end of the paragraph leaves nothing to put on the new page.
        segments.pop();
    }

    let mut paragraphs = Vec::with_capacity(segments.len());
    for segment in segments {
        let Segment {
            text,
            runs,
            notes,
            bookmarks,
            images,
            cross_refs,
            starts_page,
        } = segment;
        let mut para = Paragraph::with_runs(BlockId(0), text, runs);
        let mut para_props: ParaProps = props;
        para_props.page_break_before = starts_page;
        para = para.with_props(para_props);
        if !notes.is_empty() {
            para.set_notes(notes);
        }
        if !bookmarks.is_empty() {
            para.set_bookmarks(bookmarks);
        }
        if !images.is_empty() {
            para.set_images(images);
        }
        if !cross_refs.is_empty() {
            para.set_cross_refs(cross_refs);
        }
        paragraphs.push(para);
    }
    Ok((paragraphs, sect))
}

fn parse_run(
    run: &XmlNode,
    styles: &Styles,
    segments: &mut Vec<Segment>,
    notes: &Notes,
    images: &Images<'_>,
    field: &mut FieldState,
    warnings: &mut Vec<String>,
) -> Result<(), DocxError> {
    let direct = run.child("rPr").map(parse_char_format).unwrap_or_default();
    let styled = run
        .child("rPr")
        .and_then(|rpr| rpr.child("rStyle"))
        .and_then(|style| style.attr("val"))
        .and_then(|id| styles.char_styles.get(id))
        .cloned()
        .unwrap_or_default();
    let format = merge_char(&styled, &direct);
    for child in run.elements() {
        // Between `begin` and `separate` the run only carries the instruction.
        if matches!(field, FieldState::Instruction(_))
            && !matches!(child.name.as_str(), "rPr" | "instrText" | "fldChar")
        {
            continue;
        }
        match child.name.as_str() {
            "rPr" | "lastRenderedPageBreak" => {}
            "t" => {
                let text = child.text();
                append_text(segments.last_mut().expect("segment"), &text, &format);
            }
            "tab" => append_text(segments.last_mut().expect("segment"), "\t", &format),
            "noBreakHyphen" => {
                append_text(segments.last_mut().expect("segment"), "\u{2011}", &format);
            }
            "softHyphen" => {}
            "cr" => append_text(segments.last_mut().expect("segment"), " ", &format),
            "br" => {
                if child
                    .attr("type")
                    .is_some_and(|kind| kind.eq_ignore_ascii_case("page"))
                {
                    segments.push(Segment::new(true));
                } else {
                    append_text(segments.last_mut().expect("segment"), " ", &format);
                    warnings.push("a line break inside a paragraph became a space".to_owned());
                }
            }
            "drawing" => {
                let mut image = read_drawing(child, images)?;
                if let Some(segment) = segments.last_mut() {
                    image.offset = segment.text.len();
                    segment.images.push(image);
                }
            }
            "pict" | "object" => return Err(unsupported("a picture or drawing")),
            "fldChar" => match (child.attr("fldCharType"), &mut *field) {
                (Some("begin"), FieldState::Outside) => {
                    *field = FieldState::Instruction(String::new());
                }
                (Some("separate"), FieldState::Instruction(instruction)) => {
                    match parse_instruction(instruction) {
                        Instruction::CrossRef { target, kind } => {
                            let index = segments.len() - 1;
                            *field = FieldState::Result {
                                target,
                                kind,
                                segment: index,
                                start: segments[index].text.len(),
                            };
                        }
                        Instruction::Other => return Err(unsupported("a field")),
                    }
                }
                (Some("end"), FieldState::Result { .. }) => {
                    if let FieldState::Result {
                        target,
                        kind,
                        segment,
                        start,
                    } = std::mem::take(field)
                    {
                        finish_cross_ref(segments, segment, start, target, kind)?;
                    }
                }
                _ => return Err(unsupported("a field")),
            },
            "instrText" => match &mut *field {
                FieldState::Instruction(instruction) => instruction.push_str(&child.text()),
                _ => return Err(unsupported("a field")),
            },
            "delText" => {
                return Err(DocxError::Unsupported {
                    part: "word/document.xml".to_owned(),
                    what: "tracked changes".to_owned(),
                });
            }
            "footnoteReference" | "endnoteReference" => {
                let kind = if child.name.as_str() == "footnoteReference" {
                    NoteKind::Footnote
                } else {
                    NoteKind::Endnote
                };
                let id = child.attr("id").and_then(|value| value.parse::<u32>().ok());
                let text = id.and_then(|id| notes.text(kind, id));
                match (id.is_some(), text) {
                    (true, Some(text)) => {
                        if let Some(segment) = segments.last_mut() {
                            let offset = segment.text.len();
                            segment.notes.push(Note {
                                kind,
                                offset,
                                text: text.to_owned(),
                            });
                        }
                    }
                    _ => warnings.push(format!(
                        "a {} reference to a missing note was dropped",
                        note_kind_name(kind)
                    )),
                }
            }
            "commentReference" => {
                return Err(DocxError::Unsupported {
                    part: "word/document.xml".to_owned(),
                    what: "comments".to_owned(),
                });
            }
            _ => {}
        }
    }
    Ok(())
}

fn append_text(segment: &mut Segment, text: &str, format: &CharFormat) {
    if text.is_empty() {
        return;
    }
    let start = segment.text.len();
    segment.text.push_str(text);
    segment.runs.push(Run {
        range: start..segment.text.len(),
        format: format.clone(),
    });
}

// ------------------------------------------------------------------ formatting

fn parse_char_format(rpr: &XmlNode) -> CharFormat {
    let mut format = CharFormat::default();
    if let Some(fonts) = rpr.child("rFonts") {
        if let Some(family) = fonts.attr("ascii").or_else(|| fonts.attr("hAnsi")) {
            format.family = Some(family.to_owned().into());
        }
    }
    if let Some(size) = rpr.child("sz").and_then(|n| n.attr_f32("val")) {
        format.size_pt = Some(size / 2.0);
    }
    if let Some(bold) = rpr.child("b") {
        format.weight = Some(if bold.on() { 700 } else { 400 });
    }
    if let Some(italic) = rpr.child("i") {
        format.italic = Some(italic.on());
    }
    if let Some(underline) = rpr.child("u") {
        format.underline = Some(
            underline
                .attr("val")
                .is_none_or(|value| !value.eq_ignore_ascii_case("none")),
        );
    }
    if let Some(strike) = rpr.child("strike") {
        format.strikethrough = Some(strike.on());
    }
    if let Some(colour) = rpr.child("color").and_then(|n| n.attr("val")) {
        if let Some(rgba) = parse_hex_colour(colour) {
            format.colour = Some(rgba);
        }
    }
    if let Some(highlight) = rpr.child("highlight").and_then(|n| n.attr("val")) {
        if let Some(rgba) = highlight_colour(highlight) {
            format.highlight = Some(Some(rgba));
        }
    }
    if let Some(vertical) = rpr.child("vertAlign").and_then(|n| n.attr("val")) {
        format.vertical = match vertical.to_ascii_lowercase().as_str() {
            "superscript" => Some(VerticalAlign::Superscript),
            "subscript" => Some(VerticalAlign::Subscript),
            "baseline" => Some(VerticalAlign::Baseline),
            _ => None,
        };
    }
    if let Some(spacing) = rpr.child("spacing").and_then(|n| n.attr_f32("val")) {
        format.letter_spacing_pt = Some(spacing / 20.0);
    }
    if let Some(ligatures) = rpr.child("ligatures").and_then(|n| n.attr("val")) {
        format.ligatures = match ligatures.to_ascii_lowercase().as_str() {
            "none" => Some(Ligatures::None),
            "standard" => Some(Ligatures::Standard),
            "all" | "contextual" => Some(Ligatures::All),
            _ => None,
        };
    }
    format
}

fn parse_para_format(ppr: &XmlNode) -> ParaFormat {
    let mut format = ParaFormat::default();
    if let Some(align) = ppr.child("jc").and_then(|n| n.attr("val")) {
        format.align = match align.to_ascii_lowercase().as_str() {
            "center" | "centre" => Some(Align::Centre),
            "right" | "end" => Some(Align::End),
            "both" | "justify" | "distribute" => Some(Align::Justify),
            "left" | "start" => Some(Align::Start),
            _ => None,
        };
    }
    if let Some(indent) = ppr.child("ind") {
        let pt = |name: &str| indent.attr_f32(name).map(|twips| twips / 20.0);
        format.indent_left_pt = pt("left").or_else(|| pt("start"));
        format.indent_right_pt = pt("right").or_else(|| pt("end"));
        if let Some(first) = pt("firstLine") {
            format.indent_first_pt = Some(first);
        } else if let Some(hanging) = pt("hanging") {
            format.indent_first_pt = Some(-hanging);
        }
    }
    if let Some(spacing) = ppr.child("spacing") {
        let pt = |name: &str| spacing.attr_f32(name).map(|twips| twips / 20.0);
        format.space_before_pt = pt("before");
        format.space_after_pt = pt("after");
        if let Some(line) = spacing.attr_f32("line") {
            let rule = spacing.attr("lineRule").unwrap_or("auto");
            format.line_spacing = Some(match rule {
                "exact" => LineSpacing::Exactly(line / 20.0),
                "atLeast" => LineSpacing::AtLeast(line / 20.0),
                _ => LineSpacing::Multiple((line / 240.0).max(0.1)),
            });
        }
    }
    if let Some(keep) = ppr.child("keepLines") {
        format.keep_together = Some(keep.on());
    }
    if let Some(keep) = ppr.child("keepNext") {
        format.keep_with_next = Some(keep.on());
    }
    if let Some(widow) = ppr.child("widowControl") {
        format.widow_control = Some(widow.on());
    }
    if let Some(hyphen) = ppr.child("suppressAutoHyphens") {
        format.hyphenate = Some(!hyphen.on());
    }
    if let Some(border) = ppr.child("pBdr") {
        let mut sides = BorderSides::NONE;
        let mut width = 0.0f32;
        let mut colour = [0, 0, 0, 255];
        let mut space = 0.0f32;
        let mut any = false;
        for (name, side) in [
            ("top", &mut sides.top),
            ("left", &mut sides.left),
            ("bottom", &mut sides.bottom),
            ("right", &mut sides.right),
        ] {
            let Some(line) = border.child(name) else {
                continue;
            };
            let visible = line
                .attr("val")
                .is_none_or(|value| !matches!(value, "nil" | "none"));
            if !visible {
                continue;
            }
            *side = true;
            any = true;
            if let Some(size) = line.attr_f32("sz") {
                width = width.max(size / 8.0);
            }
            if let Some(value) = line.attr("color").and_then(parse_hex_colour) {
                colour = value;
            }
            if let Some(value) = line.attr_f32("space") {
                space = value / 20.0;
            }
        }
        format.borders = Some(if any {
            Borders {
                sides,
                width_pt: width.max(0.25),
                colour,
                space_pt: space,
            }
        } else {
            Borders::none()
        });
    }
    if let Some(tabs) = ppr.child("tabs") {
        let mut stops = TabStops::new();
        for tab in tabs.children_named("tab") {
            let Some(position) = tab.attr_f32("pos") else {
                continue;
            };
            let kind = match tab
                .attr("val")
                .unwrap_or("left")
                .to_ascii_lowercase()
                .as_str()
            {
                "center" | "centre" => TabStopKind::Centre,
                "right" | "end" => TabStopKind::Right,
                "decimal" => TabStopKind::Decimal,
                _ => TabStopKind::Left,
            };
            let _ = stops.push(TabStop {
                position_pt: position / 20.0,
                kind,
            });
        }
        format.tab_stops = Some(stops);
    }
    format
}

fn parse_props(ppr: Option<&XmlNode>, styles: &Styles, numbering: &Numbering) -> ParaProps {
    let mut props = ParaProps::default();
    let Some(ppr) = ppr else {
        return props;
    };
    if let Some(style) = ppr.child("pStyle").and_then(|n| n.attr("val")) {
        if let Some(id) = styles.id(style) {
            props.style = id;
        }
    }
    props.format = parse_para_format(ppr);
    if let Some(break_before) = ppr.child("pageBreakBefore") {
        props.page_break_before = break_before.on();
    }
    if let Some(outline) = ppr.child("outlineLvl").and_then(|n| n.attr_u32("val")) {
        props.outline_level = Some((outline as u8 + 1).min(9));
    }
    if let Some(numbering_props) = ppr.child("numPr") {
        let num_id = numbering_props
            .child("numId")
            .and_then(|n| n.attr("val"))
            .unwrap_or_default();
        let level = numbering_props
            .child("ilvl")
            .and_then(|n| n.attr("val"))
            .and_then(|value| value.parse::<u8>().ok())
            .unwrap_or(0)
            .min(MAX_LIST_LEVEL);
        if let Some((style, group)) = numbering.nums.get(num_id) {
            props.list = Some(ListItem {
                style: *style,
                level,
                group: *group,
            });
            // The model derives a list item's indents from its level.
            props.format.indent_left_pt = None;
            props.format.indent_first_pt = None;
        }
    }
    props
}

fn parse_hex_colour(value: &str) -> Option<[u8; 4]> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto") || value.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&value[0..2], 16).ok()?;
    let g = u8::from_str_radix(&value[2..4], 16).ok()?;
    let b = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some([r, g, b, 255])
}

fn highlight_colour(name: &str) -> Option<[u8; 4]> {
    let rgb = match name.to_ascii_lowercase().as_str() {
        "yellow" => [0xFF, 0xFF, 0x00],
        "green" => [0x00, 0xFF, 0x00],
        "cyan" => [0x00, 0xFF, 0xFF],
        "magenta" => [0xFF, 0x00, 0xFF],
        "blue" => [0x00, 0x00, 0xFF],
        "red" => [0xFF, 0x00, 0x00],
        "darkblue" => [0x00, 0x00, 0x80],
        "darkcyan" => [0x00, 0x80, 0x80],
        "darkgreen" => [0x00, 0x80, 0x00],
        "darkmagenta" => [0x80, 0x00, 0x80],
        "darkred" => [0x80, 0x00, 0x00],
        "darkyellow" => [0x80, 0x80, 0x00],
        "gray" | "grey" => [0x80, 0x80, 0x80],
        "lightgray" | "lightgrey" => [0xC0, 0xC0, 0xC0],
        "black" => [0x00, 0x00, 0x00],
        "white" => [0xFF, 0xFF, 0xFF],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], 255])
}
