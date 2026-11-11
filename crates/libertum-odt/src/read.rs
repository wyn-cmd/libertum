//! Reading an ODT package into the model (PLAN §6.3).
//!
//! `content.xml` is read as a stream: the style blocks before `office:body` are read into
//! [`Styles`], then each paragraph, list, table and section is parsed as it arrives, so a
//! 500-page document never needs a tree for the whole part. Unknown *package* parts are kept
//! by [`Package`]; body content the model cannot carry (footnotes, text sections, comments,
//! tracked changes, drawings) is refused with [`OdtError::Unsupported`], because saving would
//! silently lose it.
//!
//! A picture in a `draw:frame` anchored as a character becomes an inline image whose bytes
//! are read from the package; any other frame or drawing is refused. A `text:bookmark-ref`
//! showing a bookmark's text or page becomes a cross-reference over its cached text.
//!
//! Page sections are ODF master pages: a paragraph whose style names a different
//! `style:master-page-name` starts a new section (and a new page). Headers, footers, page
//! numbers and page setup travel with the master page.

use std::collections::HashMap;
use std::sync::Arc;

use libertum_formats_common::media::media_from_bytes;
use libertum_model::{
    Align, Block, BlockId, Document, HeaderFooter, InlineImage, ListItem, ListStyle,
    MAX_LIST_LEVEL, MIN_TABLE_COLUMN_PT, Media, PageNumberHorizontal, PageNumberPos,
    PageNumberVertical, ParaProps, Paragraph, Section, Table, TableBorders, TableCell, TableRow,
    TableStyle,
};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::package::Package;
use crate::style::{self, FrameRef, InlineText, PART_CONTENT, PART_STYLES, ParagraphRef, Styles};
use crate::xml::XmlNode;
use crate::{MIME_TEXT, OdtError};

pub(crate) struct Parsed {
    pub document: Document,
    pub package: Package,
    pub warnings: Vec<String>,
}

pub(crate) fn read(bytes: &[u8]) -> Result<Parsed, OdtError> {
    let package = Package::read(bytes)?;
    let mimetype = package.text("mimetype").unwrap_or_default();
    if mimetype.trim() != MIME_TEXT {
        return Err(OdtError::NotOdt);
    }
    let content = package.text(PART_CONTENT).ok_or(OdtError::NoContent)?;
    let mut warnings = Vec::new();
    let mut styles = Styles::new();
    if let Some(styles_text) = package.text(PART_STYLES) {
        styles.read_styles_part(&styles_text, &mut warnings)?;
    }

    let mut state = BodyState::new(&styles, &package);
    read_content(&content, &mut styles, &mut state, &mut warnings)?;
    let BodyState {
        blocks, sections, ..
    } = state;

    let mut document = Document::from_blocks(blocks);
    document.styles = styles.sheet.clone();
    document.sections = if sections.is_empty() {
        vec![Section::default()]
    } else {
        sections
    };
    Ok(Parsed {
        document,
        package,
        warnings,
    })
}

/// Reads `content.xml` as a stream.
fn read_content(
    text: &str,
    styles: &mut Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
) -> Result<(), OdtError> {
    let mut reader = Reader::from_str(text);
    loop {
        match reader.read_event().map_err(xml_error)? {
            Event::Start(start) => {
                let name = start.local_name().as_ref().to_owned();
                match name.as_str() {
                    "document-content" => {}
                    "font-face-decls" | "automatic-styles" => {
                        let node = style::read_node(&mut reader, &start, PART_CONTENT)?;
                        styles.read_content_styles(&node, warnings)?;
                    }
                    "body" => {
                        styles.prepare(warnings);
                        // The body's own master page is whichever master the first paragraph
                        // names; until then the default one stands in.
                        state.sections = vec![build_section(
                            styles,
                            &styles.default_master_name(),
                            warnings,
                        )?];
                        state.current_master = styles.default_master_name();
                        read_body(&mut reader, styles, state, warnings)?;
                    }
                    _ => {
                        let _ = style::read_node(&mut reader, &start, PART_CONTENT)?;
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // A file with no body still has styles worth keeping.
    styles.prepare(warnings);
    Ok(())
}

/// Reads `office:body/office:text`, whose children are the document's blocks.
fn read_body(
    reader: &mut Reader<&[u8]>,
    styles: &Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
) -> Result<(), OdtError> {
    let mut in_text = false;
    loop {
        match reader.read_event().map_err(xml_error)? {
            Event::Start(start) => {
                let name = start.local_name().as_ref().to_owned();
                if name == "text" {
                    in_text = true;
                    continue;
                }
                if !in_text {
                    let _ = style::read_node(reader, &start, PART_CONTENT)?;
                    continue;
                }
                read_text_child(reader, &start, styles, state, warnings)?;
            }
            Event::End(end) if end.local_name().as_ref() == "body" => return Ok(()),
            Event::End(end) if end.local_name().as_ref() == "text" => in_text = false,
            Event::Eof => return Ok(()),
            _ => {}
        }
    }
}

fn read_text_child(
    reader: &mut Reader<&[u8]>,
    start: &quick_xml::events::BytesStart<'_>,
    styles: &Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
) -> Result<(), OdtError> {
    let name = start.local_name().as_ref().to_owned();
    match name.as_str() {
        "p" | "h" => {
            let node = style::read_node(reader, start, PART_CONTENT)?;
            let paragraph = parse_paragraph(&node, styles, None, state, warnings, true)?;
            state.push(Block::Paragraph(paragraph));
        }
        "list" => {
            let node = style::read_node(reader, start, PART_CONTENT)?;
            let paragraphs = parse_list(&node, 0, None, styles, state, warnings, true)?;
            for paragraph in paragraphs {
                state.push(Block::Paragraph(paragraph));
            }
        }
        "table" => {
            let node = style::read_node(reader, start, PART_CONTENT)?;
            let table = parse_table(&node, styles, state, warnings)?;
            state.push(Block::Table(table));
        }
        "section" => {
            return Err(OdtError::Unsupported {
                part: PART_CONTENT.to_owned(),
                what: "a text section".to_owned(),
            });
        }
        "table-of-content" | "illustration-index" | "table-index" | "object-index"
        | "user-index" | "alphabetical-index" | "bibliography" | "index-title" | "index-body" => {
            return Err(OdtError::Unsupported {
                part: PART_CONTENT.to_owned(),
                what: "a table of contents or index".to_owned(),
            });
        }
        "tracked-changes" | "change" | "change-start" | "change-end" => {
            return Err(OdtError::Unsupported {
                part: PART_CONTENT.to_owned(),
                what: "tracked changes".to_owned(),
            });
        }
        "annotation" | "annotation-end" => {
            return Err(OdtError::Unsupported {
                part: PART_CONTENT.to_owned(),
                what: "comments".to_owned(),
            });
        }
        // A frame or shape between paragraphs is anchored to the page: nothing in the model
        // can hold it, and skipping it would lose it on the next save.
        name if style::is_drawing(name) => {
            return Err(style::unsupported(
                PART_CONTENT,
                "a picture or drawing anchored to the page",
            ));
        }
        _ => {
            let _ = style::read_node(reader, start, PART_CONTENT)?;
        }
    }
    Ok(())
}

/// Everything the body reader has to carry between blocks.
struct BodyState<'a> {
    /// The package being read, for the pictures the body refers to.
    package: &'a Package,
    /// Pictures already loaded, by package path, so a picture used twice is read once.
    media: HashMap<String, Arc<Media>>,
    blocks: Vec<Block>,
    sections: Vec<Section>,
    current_master: String,
    next_id: u64,
    next_group: u64,
    /// Last list group per list style, for `text:continue-numbering`.
    last_group: HashMap<String, u64>,
}

impl<'a> BodyState<'a> {
    fn new(styles: &Styles, package: &'a Package) -> Self {
        Self {
            package,
            media: HashMap::new(),
            blocks: Vec::new(),
            sections: vec![Section::default()],
            current_master: styles.default_master_name(),
            next_id: 1,
            next_group: 1,
            last_group: HashMap::new(),
        }
    }

    fn alloc_id(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    fn push(&mut self, block: Block) {
        self.blocks.push(block);
    }

    /// Loads the picture an inline frame shows and gives it the frame's size.
    fn image(
        &mut self,
        frame: FrameRef,
        warnings: &mut Vec<String>,
    ) -> Result<InlineImage, OdtError> {
        let mut found: Option<Arc<Media>> = None;
        let mut missing: Option<String> = None;
        for href in &frame.hrefs {
            let Some((path, bytes)) = self.picture_bytes(href) else {
                missing.get_or_insert_with(|| href.clone());
                continue;
            };
            if let Some(media) = self.media.get(&path) {
                found = Some(Arc::clone(media));
                break;
            }
            // GIF and BMP are converted to PNG; vector and metafile pictures cannot be read.
            if let Ok(media) = media_from_bytes(bytes.to_vec()) {
                self.media.insert(path, Arc::clone(&media));
                found = Some(media);
                break;
            }
        }
        let Some(media) = found else {
            return Err(match missing {
                Some(href) => OdtError::Content {
                    part: PART_CONTENT.to_owned(),
                    message: format!("the picture {href} is not in the package"),
                },
                None => style::unsupported(
                    PART_CONTENT,
                    &format!(
                        "a picture in a format Libertum cannot read ({})",
                        frame.hrefs.join(", ")
                    ),
                ),
            });
        };
        if frame.hrefs.len() > 1 {
            warnings.push(
                "a picture stored in several encodings kept only the one Libertum can read"
                    .to_owned(),
            );
        }
        let (natural_width, natural_height) = media.natural_size_pt();
        let (width_pt, height_pt) = match (frame.width_pt, frame.height_pt) {
            (Some(width), Some(height)) => (width, height),
            (Some(width), None) => (width, width * natural_height / natural_width.max(0.01)),
            (None, Some(height)) => (height * natural_width / natural_height.max(0.01), height),
            (None, None) => (natural_width, natural_height),
        };
        Ok(InlineImage {
            offset: frame.offset,
            media,
            width_pt,
            height_pt,
            alt: frame.alt,
            name: frame.name,
        })
    }

    /// The package entry an `xlink:href` names, with its path. The reference is tried as
    /// written and with its percent escapes decoded.
    fn picture_bytes(&self, href: &str) -> Option<(String, &'a [u8])> {
        let path = href.strip_prefix("./").unwrap_or(href);
        if let Some(bytes) = self.package.get(path) {
            return Some((path.to_owned(), bytes));
        }
        let decoded = percent_decode(path);
        let bytes = self.package.get(&decoded)?;
        Some((decoded, bytes))
    }
}

/// Decodes `%XX` escapes in a package reference. Malformed escapes are kept as written.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0usize;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| text.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Starts a new section when this paragraph's style names a different master page.
fn ensure_section(
    styles: &Styles,
    state: &mut BodyState<'_>,
    paragraph: &ParagraphRef,
    props: &mut ParaProps,
    warnings: &mut Vec<String>,
) -> Result<(), OdtError> {
    let Some(master) = paragraph.master_page.as_deref() else {
        return Ok(());
    };
    if master == state.current_master {
        return Ok(());
    }
    let section = build_section(styles, master, warnings)?;
    if state.blocks.is_empty() {
        // The first block decides which master page section 0 uses.
        state.sections.clear();
        state.sections.push(section);
    } else {
        state.sections.push(section);
        props.section_start = true;
    }
    state.current_master = master.to_owned();
    Ok(())
}

/// Builds a model section from a master page: page setup, columns, page numbers, headers and
/// footers.
fn build_section(
    styles: &Styles,
    master_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Section, OdtError> {
    let mut section = styles.section_for(master_name);
    let Some(master) = styles.master(master_name) else {
        return Ok(section);
    };

    let header = style::read_header_footer(master.header.as_ref(), styles, warnings, PART_CONTENT)?;
    let footer = style::read_header_footer(master.footer.as_ref(), styles, warnings, PART_CONTENT)?;
    if !header.text.is_empty() {
        section.headers.default = Some(HeaderFooter::new(header.text.clone()));
        if let Some(default) = section.headers.default.as_mut() {
            default.align = header.align;
        }
    }
    if !footer.text.is_empty() {
        section.footers.default = Some(HeaderFooter::new(footer.text.clone()));
        if let Some(default) = section.footers.default.as_mut() {
            default.align = footer.align;
        }
    }
    if header.page_number {
        section.page_numbers = Some(PageNumberPos {
            vertical: PageNumberVertical::Top,
            horizontal: horizontal(header.number_align),
            mirror: false,
        });
    } else if footer.page_number {
        section.page_numbers = Some(PageNumberPos {
            vertical: PageNumberVertical::Bottom,
            horizontal: horizontal(footer.number_align),
            mirror: false,
        });
    }

    let first_header =
        style::read_header_footer(master.header_first.as_ref(), styles, warnings, PART_CONTENT)?;
    let first_footer =
        style::read_header_footer(master.footer_first.as_ref(), styles, warnings, PART_CONTENT)?;
    let default_number = header.page_number || footer.page_number;
    let first_number = first_header.page_number || first_footer.page_number;
    let first_blocks = master.header_first.is_some() || master.footer_first.is_some();
    // A first-page block without the page-number field is how Libertum writes
    // `hide_first_page`; it also hides a header or footer the model cannot suppress, which is
    // reported rather than silently dropped.
    if first_blocks && default_number && !first_number {
        section.hide_first_page = true;
    }
    let first_header_differs = master.header_first.is_some()
        && !first_header.text.is_empty()
        && first_header.text != header.text;
    if first_header_differs {
        section.headers.different_first = true;
        section.headers.first = Some(HeaderFooter::new(first_header.text));
        if let Some(first) = section.headers.first.as_mut() {
            first.align = first_header.align;
        }
    } else if master.header_first.is_some()
        && first_header.text.is_empty()
        && !default_number
        && !header.text.is_empty()
    {
        warnings.push(
            "an empty first-page header cannot be represented; the header shows on the first page"
                .to_owned(),
        );
    }
    let first_footer_differs = master.footer_first.is_some()
        && !first_footer.text.is_empty()
        && first_footer.text != footer.text;
    if first_footer_differs {
        section.footers.different_first = true;
        section.footers.first = Some(HeaderFooter::new(first_footer.text));
        if let Some(first) = section.footers.first.as_mut() {
            first.align = first_footer.align;
        }
    } else if master.footer_first.is_some()
        && first_footer.text.is_empty()
        && !default_number
        && !footer.text.is_empty()
    {
        warnings.push(
            "an empty first-page footer cannot be represented; the footer shows on the first page"
                .to_owned(),
        );
    }

    let even_header =
        style::read_header_footer(master.header_left.as_ref(), styles, warnings, PART_CONTENT)?;
    let even_footer =
        style::read_header_footer(master.footer_left.as_ref(), styles, warnings, PART_CONTENT)?;
    if master.header_left.is_some() {
        if even_header.text.is_empty() {
            warnings.push(
                "an empty even-page header cannot suppress the header on even pages; ignored"
                    .to_owned(),
            );
        } else {
            section.headers.different_even_odd = true;
            section.headers.even = Some(HeaderFooter::new(even_header.text));
            if let Some(even) = section.headers.even.as_mut() {
                even.align = even_header.align;
            }
        }
    }
    if master.footer_left.is_some() {
        if even_footer.text.is_empty() {
            warnings.push(
                "an empty even-page footer cannot suppress the footer on even pages; ignored"
                    .to_owned(),
            );
        } else {
            section.footers.different_even_odd = true;
            section.footers.even = Some(HeaderFooter::new(even_footer.text));
            if let Some(even) = section.footers.even.as_mut() {
                even.align = even_footer.align;
            }
        }
    }

    Ok(section)
}

fn horizontal(align: Align) -> PageNumberHorizontal {
    match align {
        Align::Centre => PageNumberHorizontal::Centre,
        Align::End => PageNumberHorizontal::Right,
        _ => PageNumberHorizontal::Left,
    }
}

fn parse_paragraph(
    node: &XmlNode,
    styles: &Styles,
    list: Option<ListItem>,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
    body: bool,
) -> Result<Paragraph, OdtError> {
    let style_name = node.attr("style-name");
    let paragraph_style = styles.paragraph(style_name);
    let resolved = styles
        .sheet
        .resolve_char(paragraph_style.style)
        .apply(&paragraph_style.char);
    let mut inline = InlineText::default();
    style::read_inline(
        node,
        styles,
        &paragraph_style.char,
        &resolved,
        &mut inline,
        warnings,
        PART_CONTENT,
    )?;
    if inline.has_page_number {
        warnings.push("a page-number field in body text became the text {page}".to_owned());
    }
    let outline = node
        .attr_u32("outline-level")
        .map(|level| (level as u8).clamp(1, 9))
        .or(paragraph_style.outline)
        .or_else(|| (node.name == "h").then_some(1));
    let mut props = ParaProps {
        style: paragraph_style.style,
        format: paragraph_style.format,
        page_break_before: paragraph_style.page_break_before,
        outline_level: outline,
        list,
        ..ParaProps::default()
    };
    if list.is_some() {
        // The model derives a list item's indents from its level.
        props.format.indent_left_pt = None;
        props.format.indent_first_pt = None;
    }
    if body {
        ensure_section(styles, state, &paragraph_style, &mut props, warnings)?;
    }
    let id = state.alloc_id();
    let mut paragraph = Paragraph::with_runs(id, inline.text, inline.runs).with_props(props);
    if !inline.notes.is_empty() {
        paragraph.set_notes(
            inline
                .notes
                .into_iter()
                .map(|(kind, offset, text)| libertum_model::Note { kind, offset, text })
                .collect(),
        );
    }
    if !inline.bookmarks.is_empty() {
        paragraph.set_bookmarks(inline.bookmarks);
    }
    if !inline.frames.is_empty() {
        let mut images = Vec::with_capacity(inline.frames.len());
        for frame in inline.frames {
            images.push(state.image(frame, warnings)?);
        }
        paragraph.set_images(images);
    }
    if !inline.cross_refs.is_empty() {
        paragraph.set_cross_refs(inline.cross_refs);
    }
    Ok(paragraph)
}

/// Parses a `text:list` (nested lists included). `group` is fixed for nested lists and `None`
/// at the top level, where `text:continue-numbering` decides whether the previous list
/// continues.
fn parse_list(
    node: &XmlNode,
    level: u8,
    group: Option<u64>,
    styles: &Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
    body: bool,
) -> Result<Vec<Paragraph>, OdtError> {
    let style_name = node.attr("style-name");
    let list_style = styles.list_style(style_name).unwrap_or(ListStyle::Bullet);
    let group = match group {
        Some(group) => group,
        None => {
            let continued = style::list_continues(node)
                .then(|| style_name.and_then(|name| state.last_group.get(name).copied()))
                .flatten();
            match continued {
                Some(group) => group,
                None => {
                    let group = state.next_group;
                    state.next_group += 1;
                    group
                }
            }
        }
    };
    if let Some(name) = style_name {
        state.last_group.insert(name.to_owned(), group);
    }
    let level = level.min(MAX_LIST_LEVEL);
    let mut paragraphs = Vec::new();
    for child in node.elements() {
        match child.name.as_str() {
            "list-item" => {
                let mut first = true;
                for item in child.elements() {
                    match item.name.as_str() {
                        "p" | "h" => {
                            let list = first.then_some(ListItem {
                                style: list_style,
                                level,
                                group,
                            });
                            paragraphs
                                .push(parse_paragraph(item, styles, list, state, warnings, body)?);
                            first = false;
                        }
                        "list" => paragraphs.extend(parse_list(
                            item,
                            level + 1,
                            Some(group),
                            styles,
                            state,
                            warnings,
                            body,
                        )?),
                        "list-header" => {
                            for paragraph in item.children_named("p") {
                                paragraphs.push(parse_paragraph(
                                    paragraph, styles, None, state, warnings, body,
                                )?);
                            }
                        }
                        _ => {}
                    }
                }
            }
            "list-header" => {
                for paragraph in child.children_named("p") {
                    paragraphs.push(parse_paragraph(
                        paragraph, styles, None, state, warnings, body,
                    )?);
                }
            }
            _ => {}
        }
    }
    Ok(paragraphs)
}

fn parse_table(
    node: &XmlNode,
    styles: &Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
) -> Result<Table, OdtError> {
    let table_style = node.attr("style-name");

    let mut widths: Vec<f32> = Vec::new();
    read_columns(node, styles, &mut widths);

    let mut rows: Vec<TableRow> = Vec::new();
    let mut header_rows = 0u8;
    let mut borders: Option<TableBorders> = None;
    read_rows(
        node,
        table_style,
        styles,
        state,
        warnings,
        &mut rows,
        &mut header_rows,
        &mut borders,
        false,
    )?;

    if rows.is_empty() {
        return Err(OdtError::Unsupported {
            part: PART_CONTENT.to_owned(),
            what: "an empty table".to_owned(),
        });
    }
    let columns = rows.iter().map(TableRow::span).max().unwrap_or(1).max(1);
    if widths.len() < columns {
        widths.resize(columns, 72.0);
    }
    widths.truncate(columns);
    for width in &mut widths {
        *width = width.max(MIN_TABLE_COLUMN_PT);
    }

    let borders = borders.unwrap_or_else(|| styles.table_borders(table_style, None));
    let id = state.alloc_id();
    let mut table = Table {
        style: None,
        id,
        widths_pt: widths,
        rows,
        header_rows,
        borders,
    };
    table.normalise();
    if let Some(name) = node.attr("template-name").filter(|name| !name.is_empty()) {
        adopt_table_style(&mut table, name, warnings);
    }
    Ok(table)
}

/// Maps `table:template-name` back to a built-in table style.
///
/// The writer stores each cell's resolved fill in its cell style, so the table looks right
/// in programs that know nothing of the template. Here the fills the style would give are
/// taken off the cells again, leaving only those the user set. A cell whose own fill equals
/// the style's fill for its row therefore comes back without one; it draws the same.
///
/// The style is adopted only when the file's fills agree with it (every cell of a header or
/// banded row is filled). Otherwise, and for a template name that is not a built-in, the
/// fills stay on the cells and the table has no style.
fn adopt_table_style(table: &mut Table, name: &str, warnings: &mut Vec<String>) {
    let Some((style, _)) = TableStyle::builtin(name) else {
        warnings.push(format!(
            "the table style {name:?} is not one of Libertum's; its fills were kept on the cells"
        ));
        return;
    };
    table.style = Some(style);
    let agrees = table.rows.iter().enumerate().all(|(row, table_row)| {
        table.style_fill(row).is_none()
            || table_row
                .cells
                .iter()
                .all(|cell| cell.merged_into_above || cell.fill.is_some())
    });
    if !agrees {
        table.style = None;
        warnings.push(format!(
            "the table style {name:?} does not match the table's fills; the fills were kept on the cells"
        ));
        return;
    }
    for row in 0..table.rows.len() {
        let Some(fill) = table.style_fill(row) else {
            continue;
        };
        for cell in &mut table.rows[row].cells {
            if cell.fill == Some(fill) {
                cell.fill = None;
            }
        }
    }
}

/// Reads `table:table-column` children, including those wrapped in `table:table-columns`.
fn read_columns(node: &XmlNode, styles: &Styles, widths: &mut Vec<f32>) {
    for child in node.elements() {
        match child.name.as_str() {
            "table-column" => {
                let width = styles.column_width(child.attr("style-name"));
                let repeat = child
                    .attr_u32("number-columns-repeated")
                    .unwrap_or(1)
                    .clamp(1, 256);
                // A column without a width gets a default; it is replaced below when the rows
                // show a different column count.
                for _ in 0..repeat {
                    widths.push(width.unwrap_or(72.0).max(MIN_TABLE_COLUMN_PT));
                }
            }
            "table-columns" => read_columns(child, styles, widths),
            _ => {}
        }
    }
}

/// Reads `table:table-row` elements, including the groups that hold header rows.
#[allow(clippy::too_many_arguments)]
fn read_rows(
    node: &XmlNode,
    table_style: Option<&str>,
    styles: &Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
    rows: &mut Vec<TableRow>,
    header_rows: &mut u8,
    borders: &mut Option<TableBorders>,
    in_header: bool,
) -> Result<(), OdtError> {
    for child in node.elements() {
        match child.name.as_str() {
            "table-row" => {
                let repeat = child
                    .attr_u32("number-rows-repeated")
                    .unwrap_or(1)
                    .clamp(1, 256);
                let row = parse_row(child, table_style, styles, state, warnings, borders)?;
                for _ in 0..repeat {
                    if in_header && *header_rows as usize == rows.len() {
                        *header_rows = header_rows.saturating_add(1);
                    }
                    rows.push(row.clone());
                }
            }
            "table-header-rows" => read_rows(
                child,
                table_style,
                styles,
                state,
                warnings,
                rows,
                header_rows,
                borders,
                true,
            )?,
            "table-row-group" | "table-rows" | "table-body" => read_rows(
                child,
                table_style,
                styles,
                state,
                warnings,
                rows,
                header_rows,
                borders,
                in_header,
            )?,
            // Column definitions and captions are handled elsewhere or not modelled.
            _ => {}
        }
    }
    Ok(())
}

fn parse_row(
    node: &XmlNode,
    table_style: Option<&str>,
    styles: &Styles,
    state: &mut BodyState<'_>,
    warnings: &mut Vec<String>,
    borders: &mut Option<TableBorders>,
) -> Result<TableRow, OdtError> {
    let min_height_pt = styles.row_height(node.attr("style-name")).unwrap_or(0.0);
    let mut cells = Vec::new();
    for child in node.elements() {
        match child.name.as_str() {
            "table-cell" | "covered-table-cell" => {
                let covered = child.name == "covered-table-cell";
                let span = child
                    .attr_u32("number-columns-spanned")
                    .map_or(1u8, |value| value.clamp(1, 63) as u8);
                let repeat = child
                    .attr_u32("number-columns-repeated")
                    .unwrap_or(1)
                    .clamp(1, 256);
                let cell_style = child.attr("style-name");
                if borders.is_none() {
                    *borders = Some(styles.table_borders(table_style, cell_style));
                }
                let fill = styles.cell_fill(cell_style);
                let mut paragraphs = Vec::new();
                if !covered {
                    for item in child.elements() {
                        match item.name.as_str() {
                            "p" | "h" => paragraphs
                                .push(parse_paragraph(item, styles, None, state, warnings, false)?),
                            "list" => paragraphs
                                .extend(parse_list(item, 0, None, styles, state, warnings, false)?),
                            "table" => {
                                return Err(OdtError::Unsupported {
                                    part: PART_CONTENT.to_owned(),
                                    what: "a nested table".to_owned(),
                                });
                            }
                            _ => {}
                        }
                    }
                }
                if paragraphs.is_empty() {
                    let id = state.alloc_id();
                    paragraphs.push(Paragraph::new(id, ""));
                }
                for _ in 0..repeat {
                    cells.push(TableCell {
                        col_span: span,
                        merged_into_above: covered,
                        paragraphs: paragraphs.clone(),
                        fill,
                    });
                }
            }
            _ => {}
        }
    }
    if cells.is_empty() {
        let id = state.alloc_id();
        cells.push(TableCell::new(Paragraph::new(id, "")));
    }
    Ok(TableRow {
        cells,
        min_height_pt,
        keep_together: true,
    })
}

fn xml_error(error: quick_xml::Error) -> OdtError {
    OdtError::Xml {
        part: PART_CONTENT.to_owned(),
        message: error.to_string(),
    }
}
