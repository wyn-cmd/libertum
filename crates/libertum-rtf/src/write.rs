//! Writing RTF 1.9.1 (PLAN §6.5).
//!
//! The writer emits a complete document: header, font table, colour table, style sheet,
//! list tables and the body. Every non-ASCII character is written as `\uN?` (signed UTF-16
//! code units, two for a surrogate pair), all text is escaped, and property changes are
//! grouped per paragraph and per run rather than written character by character.
//!
//! The generated tables come from the model, like the DOCX and ODT writers regenerate
//! `styles.xml` and `content.xml`: `\stylesheet` is built from [`StyleSheet`], the font table
//! from every family the document uses, and the colour table from every colour. Raw fragments
//! preserved by the reader are re-emitted from [`Rtf`](crate::Rtf)'s preamble and paragraph
//! snippets.

use std::collections::HashMap;

use libertum_model::{
    Align, Block, BlockId, CharFormat, CrossRef, CrossRefKind, Document, HeaderFooter, HfKind,
    HfVariant, ImageFormat, InlineImage, LineSpacing, ListStyle, MIN_TABLE_COLUMN_PT, Note,
    NoteKind, Orientation, ParaFormat, Paragraph, ResolvedChar, ResolvedPara, Section, StyleId,
    StyleSheet, TabStopKind, Table, TableBorders, VerticalAlign, floor_char_boundary,
};

use crate::{HeaderKey, RtfError, Snippet};

/// Builds the whole RTF stream.
pub(crate) fn build(
    document: &Document,
    preamble: &[Vec<u8>],
    snippets: &HashMap<BlockId, Vec<Snippet>>,
    header_snippets: &HashMap<HeaderKey, Vec<Vec<u8>>>,
    stylesheet_extras: &[Vec<u8>],
) -> Result<Vec<u8>, RtfError> {
    let fonts = FontTable::collect(document);
    let colours = ColourTable::collect(document);
    let lists = ListPlan::collect(document);
    let ctx = Context {
        styles: &document.styles,
        fonts,
        colours,
        lists,
        snippets,
        header_snippets,
        stylesheet_extras,
    };

    let mut out = Vec::with_capacity(document.word_count() * 8 + 4096);
    out.extend_from_slice(b"{\\rtf1\\ansi\\ansicpg1252\\deff0\\uc1\n");
    write_font_table(&mut out, &ctx);
    write_colour_table(&mut out, &ctx);
    write_style_sheet(&mut out, &ctx);
    write_list_tables(&mut out, &ctx);
    for raw in preamble {
        out.extend_from_slice(raw);
        out.push(b'\n');
    }
    write_body(&mut out, document, &ctx);
    out.push(b'}');
    Ok(out)
}

struct Context<'a> {
    styles: &'a StyleSheet,
    fonts: FontTable,
    colours: ColourTable,
    lists: ListPlan,
    snippets: &'a HashMap<BlockId, Vec<Snippet>>,
    header_snippets: &'a HashMap<HeaderKey, Vec<Vec<u8>>>,
    stylesheet_extras: &'a [Vec<u8>],
}

// ---------------------------------------------------------------------- tables

struct FontTable {
    families: Vec<String>,
    lookup: HashMap<String, usize>,
}

impl FontTable {
    fn collect(document: &Document) -> Self {
        let mut table = Self {
            families: Vec::new(),
            lookup: HashMap::new(),
        };
        let default = document.styles.defaults.family.to_string();
        table.add(if default.is_empty() {
            "Cambria"
        } else {
            &default
        });
        for style in &document.styles.paragraph {
            if let Some(family) = &style.char.family {
                table.add(family);
            }
        }
        for paragraph in document.paragraphs() {
            for run in paragraph.runs() {
                if let Some(family) = &run.format.family {
                    table.add(family);
                }
            }
        }
        table
    }

    fn add(&mut self, family: &str) {
        if family.is_empty() || self.lookup.contains_key(family) {
            return;
        }
        let index = self.families.len();
        self.families.push(family.to_owned());
        self.lookup.insert(family.to_owned(), index);
    }

    fn index(&self, family: &str) -> usize {
        self.lookup.get(family).copied().unwrap_or(0)
    }
}

struct ColourTable {
    /// Index 0 is RTF's "automatic" colour and is never emitted as an explicit entry.
    colours: Vec<[u8; 4]>,
    lookup: HashMap<[u8; 4], usize>,
}

impl ColourTable {
    fn collect(document: &Document) -> Self {
        let mut colours = Self {
            colours: vec![[0, 0, 0, 255]],
            lookup: HashMap::new(),
        };
        colours.add(document.styles.defaults.colour);
        if let Some(highlight) = document.styles.defaults.highlight {
            colours.add(highlight);
        }
        for style in &document.styles.paragraph {
            if let Some(colour) = style.char.colour {
                colours.add(colour);
            }
            if let Some(Some(highlight)) = style.char.highlight {
                colours.add(highlight);
            }
        }
        for paragraph in document.paragraphs() {
            if let Some(borders) = paragraph.props.format.borders {
                colours.add(borders.colour);
            }
            for run in paragraph.runs() {
                if let Some(colour) = run.format.colour {
                    colours.add(colour);
                }
                if let Some(Some(highlight)) = run.format.highlight {
                    colours.add(highlight);
                }
            }
        }
        for block in document.body().iter() {
            if let Some(table) = block.as_table() {
                table_colours(table, &mut colours);
            }
        }
        colours
    }

    fn add(&mut self, colour: [u8; 4]) {
        if colour[3] == 0 || self.lookup.contains_key(&colour) {
            return;
        }
        let index = self.colours.len();
        self.colours.push(colour);
        self.lookup.insert(colour, index);
    }

    fn index(&self, colour: [u8; 4]) -> usize {
        self.lookup.get(&colour).copied().unwrap_or(0)
    }
}

fn table_colours(table: &Table, colours: &mut ColourTable) {
    colours.add(table.borders.colour);
    for (row_index, row) in table.rows.iter().enumerate() {
        for cell in &row.cells {
            // The resolved fill (the cell's own, else the table style's) is what is written.
            if let Some(fill) = table.resolved_fill(row_index, cell) {
                colours.add(fill);
            }
        }
    }
}

struct ListPlan {
    entries: Vec<ListEntry>,
    lookup: HashMap<(u8, u64), u16>,
}

struct ListEntry {
    style: ListStyle,
    ls: u16,
    list_id: i32,
}

impl ListPlan {
    fn collect(document: &Document) -> Self {
        let mut plan = Self {
            entries: Vec::new(),
            lookup: HashMap::new(),
        };
        for paragraph in document.paragraphs() {
            let Some(item) = paragraph.props.list else {
                continue;
            };
            let key = (list_style_code(item.style), item.group);
            if plan.lookup.contains_key(&key) {
                continue;
            }
            let ls = (plan.entries.len() + 1).min(u16::MAX as usize) as u16;
            let list_id = 1000 + plan.entries.len() as i32;
            plan.lookup.insert(key, ls);
            plan.entries.push(ListEntry {
                style: item.style,
                ls,
                list_id,
            });
        }
        plan
    }

    fn ls(&self, style: ListStyle, group: u64) -> Option<u16> {
        self.lookup.get(&(list_style_code(style), group)).copied()
    }
}

fn list_style_code(style: ListStyle) -> u8 {
    match style {
        ListStyle::Bullet => 1,
        ListStyle::Numbered => 2,
        ListStyle::Lettered => 3,
        ListStyle::Harvard => 4,
    }
}

// ---------------------------------------------------------------------- header tables

fn write_font_table(out: &mut Vec<u8>, ctx: &Context<'_>) {
    out.extend_from_slice(b"{\\fonttbl\n");
    for (index, family) in ctx.fonts.families.iter().enumerate() {
        out.extend_from_slice(b"{\\f");
        out.extend_from_slice(index.to_string().as_bytes());
        out.extend_from_slice(b"\\fcharset0 ");
        write_font_name(out, family);
        out.extend_from_slice(b";}\n");
    }
    out.extend_from_slice(b"}\n");
}

fn write_font_name(out: &mut Vec<u8>, family: &str) {
    // A font name ends at `;`, so a semicolon cannot be represented. Everything else is
    // ordinary RTF text.
    for character in family.chars() {
        match character {
            '\\' | '{' | '}' => {
                out.push(b'\\');
                out.push(character as u8);
            }
            ';' | '\r' | '\n' => out.push(b' '),
            character if character.is_ascii() && (character as u32) >= 0x20 => {
                out.push(character as u8);
            }
            character => write_unicode_char(out, character),
        }
    }
}

fn write_colour_table(out: &mut Vec<u8>, ctx: &Context<'_>) {
    out.extend_from_slice(b"{\\colortbl;");
    for colour in ctx.colours.colours.iter().skip(1) {
        out.extend_from_slice(
            format!("\\red{}\\green{}\\blue{};", colour[0], colour[1], colour[2]).as_bytes(),
        );
    }
    out.extend_from_slice(b"}\n");
}

fn write_style_sheet(out: &mut Vec<u8>, ctx: &Context<'_>) {
    out.extend_from_slice(b"{\\stylesheet\n");
    for (index, style) in ctx.styles.paragraph.iter().enumerate() {
        out.push(b'{');
        ctrl_i(out, "s", index as i32);
        ctrl_i(
            out,
            "snext",
            style.next.unwrap_or(StyleId(index as u32)).0 as i32,
        );
        if index > 0 {
            ctrl_i(
                out,
                "sbasedon",
                style.base.unwrap_or(StyleId::BODY).0 as i32,
            );
        }
        if let Some(level) = style.outline_level {
            ctrl_i(out, "outlinelevel", i32::from(level.saturating_sub(1)));
        }
        let (char, para) = if index == 0 {
            (
                resolved_char_format(&ctx.styles.defaults).overlay(&style.char),
                resolved_para_format(&ctx.styles.para_defaults).overlay(&style.para),
            )
        } else {
            (style.char.clone(), style.para)
        };
        write_char_format(out, &char, ctx);
        write_para_format(out, &para, ctx.styles.defaults.size_pt);
        write_font_name(out, &style_name(StyleId(index as u32), style.name.as_str()));
        out.extend_from_slice(b";}\n");
    }
    for raw in ctx.stylesheet_extras {
        out.extend_from_slice(raw);
        out.push(b'\n');
    }
    out.extend_from_slice(b"}\n");
}

fn style_name(id: StyleId, fallback: &str) -> String {
    match id {
        StyleId::BODY => "Normal".to_owned(),
        StyleId::TITLE => "Title".to_owned(),
        StyleId::HEADING_1 => "heading 1".to_owned(),
        StyleId::HEADING_2 => "heading 2".to_owned(),
        StyleId::HEADING_3 => "heading 3".to_owned(),
        StyleId::HEADING_4 => "heading 4".to_owned(),
        StyleId::QUOTE => "Quote".to_owned(),
        _ => fallback.replace(';', " "),
    }
}

fn write_list_tables(out: &mut Vec<u8>, ctx: &Context<'_>) {
    if ctx.lists.entries.is_empty() {
        return;
    }
    out.extend_from_slice(b"{\\*\\listtable\n");
    for entry in &ctx.lists.entries {
        out.extend_from_slice(b"{\\list\\listtemplateid");
        out.extend_from_slice(entry.list_id.to_string().as_bytes());
        out.push(b' ');
        for level in 0..9u8 {
            write_list_level(out, entry.style, level);
        }
        out.extend_from_slice(b"\\listid");
        out.extend_from_slice(entry.list_id.to_string().as_bytes());
        out.extend_from_slice(b"}\n");
    }
    out.extend_from_slice(b"}\n");
    out.extend_from_slice(b"{\\*\\listoverridetable\n");
    for entry in &ctx.lists.entries {
        out.extend_from_slice(b"{\\listoverride\\listid");
        out.extend_from_slice(entry.list_id.to_string().as_bytes());
        out.extend_from_slice(b"\\listoverridecount0\\ls");
        out.extend_from_slice(entry.ls.to_string().as_bytes());
        out.extend_from_slice(b"}\n");
    }
    out.extend_from_slice(b"}\n");
}

fn write_list_level(out: &mut Vec<u8>, style: ListStyle, level: u8) {
    let (nfc, template) = level_format(style, level);
    let indent = 720 * (i32::from(level) + 1);
    out.extend_from_slice(b"{\\listlevel\\levelnfc");
    out.extend_from_slice(nfc.to_string().as_bytes());
    out.extend_from_slice(b"\\levelnfcn");
    out.extend_from_slice(nfc.to_string().as_bytes());
    out.extend_from_slice(
        b"\\leveljc0\\leveljcn0\\levelfollow0\\levelstartat1\\levelspace0\\levelindent0\n",
    );
    out.extend_from_slice(b"{\\leveltext");
    out.extend_from_slice(template);
    out.extend_from_slice(b"}\n");
    if style == ListStyle::Bullet {
        out.extend_from_slice(b"{\\levelnumbers;}\n");
    } else {
        out.extend_from_slice(b"{\\levelnumbers\\'01;}\n");
    }
    out.extend_from_slice(
        format!("\\fi-360\\li{indent}\\lin{indent}\\jclisttab\\tx{indent}}}\n").as_bytes(),
    );
}

/// `(levelnfc, raw leveltext destination body)`. The first byte of the level text is its
/// length; `\'00` is the level-number placeholder.
fn level_format(style: ListStyle, level: u8) -> (i32, &'static [u8]) {
    const BULLET: &[u8] = b"\\'01\\'95;";
    const DECIMAL: &[u8] = b"\\'02\\'00.;";
    const PAREN: &[u8] = b"\\'03(\\'00);";
    match style {
        ListStyle::Bullet => (23, BULLET),
        ListStyle::Numbered => match level % 3 {
            0 => (0, DECIMAL),
            1 => (4, DECIMAL),
            _ => (2, DECIMAL),
        },
        ListStyle::Lettered => match level % 3 {
            0 => (3, DECIMAL),
            1 => (4, DECIMAL),
            _ => (2, DECIMAL),
        },
        ListStyle::Harvard => match level % 7 {
            0 => (1, DECIMAL),
            1 => (3, DECIMAL),
            2 => (0, DECIMAL),
            3 => (4, DECIMAL),
            4 => (0, PAREN),
            5 => (4, PAREN),
            _ => (2, DECIMAL),
        },
    }
}

// ---------------------------------------------------------------------- body

fn write_body(out: &mut Vec<u8>, document: &Document, ctx: &Context<'_>) {
    let mut section_index = 0usize;
    let mut top_level_paragraph = 0usize;
    let first_section = document
        .sections
        .first()
        .cloned()
        .unwrap_or_else(Section::default);
    write_section_props(out, &first_section, ctx);
    for block in document.body().iter() {
        match block {
            Block::Paragraph(paragraph) => {
                let starts_section = paragraph.props.section_start
                    && top_level_paragraph > 0
                    && section_index + 1 < document.sections.len();
                if starts_section {
                    write_section_furniture(
                        out,
                        &document.sections[section_index],
                        section_index,
                        ctx,
                    );
                    ctrl(out, "sect");
                    section_index += 1;
                    write_section_props(
                        out,
                        document
                            .sections
                            .get(section_index)
                            .unwrap_or(&first_section),
                        ctx,
                    );
                }
                write_paragraph(out, paragraph, false, ctx);
                top_level_paragraph += 1;
            }
            Block::Table(table) => write_table(out, table, ctx),
        }
    }
    let last = document
        .sections
        .get(section_index)
        .cloned()
        .unwrap_or_else(Section::default);
    write_section_furniture(out, &last, section_index, ctx);
}

fn write_section_props(out: &mut Vec<u8>, section: &Section, _ctx: &Context<'_>) {
    ctrl(out, "sectd");
    let width = section.page.width_pt();
    let height = section.page.height_pt();
    ctrl_i(out, "paperw", twips_i(width));
    ctrl_i(out, "paperh", twips_i(height));
    ctrl_i(out, "margl", twips_i(section.page.margins.left));
    ctrl_i(out, "margr", twips_i(section.page.margins.right));
    ctrl_i(out, "margt", twips_i(section.page.margins.top));
    ctrl_i(out, "margb", twips_i(section.page.margins.bottom));
    ctrl_i(out, "headery", twips_i(section.header_distance_pt));
    ctrl_i(out, "footery", twips_i(section.footer_distance_pt));
    if section.page.orientation == Orientation::Landscape {
        ctrl(out, "landscape");
    } else {
        ctrl(out, "portrait");
    }
    if section.columns > 1 {
        ctrl_i(out, "cols", i32::from(section.columns));
        ctrl_i(out, "colsx", twips_i(section.column_gutter_pt));
    }
    if section.headers.different_first || section.footers.different_first {
        ctrl(out, "titlepg");
    }
    if let Some(start) = section.first_page_number {
        ctrl_i(out, "pgnstart", start as i32);
    }
}

fn write_section_furniture(
    out: &mut Vec<u8>,
    section: &Section,
    section_index: usize,
    ctx: &Context<'_>,
) {
    let headers = &section.headers;
    if let Some(header) = &headers.default {
        write_header_group(
            out,
            "header",
            header,
            HfKind::Header,
            HfVariant::Default,
            section_index,
            ctx,
        );
    }
    if headers.different_first {
        if let Some(header) = headers.first.as_ref().or(headers.default.as_ref()) {
            write_header_group(
                out,
                "headerf",
                header,
                HfKind::Header,
                HfVariant::First,
                section_index,
                ctx,
            );
        }
    }
    if headers.different_even_odd {
        if let Some(header) = headers.even.as_ref().or(headers.default.as_ref()) {
            write_header_group(
                out,
                "headerl",
                header,
                HfKind::Header,
                HfVariant::Even,
                section_index,
                ctx,
            );
        }
    }
    let footers = &section.footers;
    if let Some(footer) = &footers.default {
        write_header_group(
            out,
            "footer",
            footer,
            HfKind::Footer,
            HfVariant::Default,
            section_index,
            ctx,
        );
    }
    if footers.different_first {
        if let Some(footer) = footers.first.as_ref().or(footers.default.as_ref()) {
            write_header_group(
                out,
                "footerf",
                footer,
                HfKind::Footer,
                HfVariant::First,
                section_index,
                ctx,
            );
        }
    }
    if footers.different_even_odd {
        if let Some(footer) = footers.even.as_ref().or(footers.default.as_ref()) {
            write_header_group(
                out,
                "footerl",
                footer,
                HfKind::Footer,
                HfVariant::Even,
                section_index,
                ctx,
            );
        }
    }
}

fn write_header_group(
    out: &mut Vec<u8>,
    selector: &str,
    header: &HeaderFooter,
    kind: HfKind,
    variant: HfVariant,
    section_index: usize,
    ctx: &Context<'_>,
) {
    out.push(b'{');
    ctrl(out, selector);
    ctrl(out, "pard");
    ctrl(out, "plain");
    write_align(out, header.align);
    write_header_text(out, &header.text, ctx);
    ctrl(out, "par");
    if let Some(snippets) = ctx.header_snippets.get(&(section_index, kind, variant)) {
        for raw in snippets {
            out.extend_from_slice(raw);
        }
    }
    out.push(b'}');
}

fn write_header_text(out: &mut Vec<u8>, text: &str, _ctx: &Context<'_>) {
    let mut rest = text;
    loop {
        let page = rest.find("{page}");
        let pages = rest.find("{pages}");
        let at = match (page, pages) {
            (Some(page), Some(pages)) => Some((page.min(pages), page <= pages)),
            (Some(page), None) => Some((page, true)),
            (None, Some(pages)) => Some((pages, false)),
            (None, None) => None,
        };
        let Some((at, is_page)) = at else {
            write_text(out, rest);
            return;
        };
        write_text(out, &rest[..at]);
        if is_page {
            write_field(out, "PAGE", "1");
            rest = &rest[at + "{page}".len()..];
        } else {
            write_field(out, "NUMPAGES", "1");
            rest = &rest[at + "{pages}".len()..];
        }
    }
}

fn write_field(out: &mut Vec<u8>, instruction: &str, cached: &str) {
    out.extend_from_slice(b"{\\field{\\*\\fldinst ");
    out.extend_from_slice(instruction.as_bytes());
    out.extend_from_slice(b"}{\\fldrslt ");
    out.extend_from_slice(cached.as_bytes());
    out.extend_from_slice(b"}}");
}

fn write_paragraph(out: &mut Vec<u8>, paragraph: &Paragraph, in_table: bool, ctx: &Context<'_>) {
    out.push(b'{');
    ctrl(out, "pard");
    ctrl(out, "plain");
    ctrl_i(out, "s", paragraph.props.style.0 as i32);
    if in_table {
        ctrl(out, "intbl");
    }
    write_para_format(out, &paragraph.props.format, ctx.styles.defaults.size_pt);
    if paragraph.props.page_break_before {
        ctrl(out, "pagebb");
    }
    if let Some(item) = paragraph.props.list {
        if let Some(ls) = ctx.lists.ls(item.style, item.group) {
            ctrl_i(out, "ls", i32::from(ls));
            ctrl_i(out, "ilvl", i32::from(item.level));
        }
    }
    if let Some(level) = paragraph.props.outline_level {
        ctrl_i(out, "outlinelevel", i32::from(level.saturating_sub(1)));
    }
    let base = ctx.styles.resolve_char(paragraph.props.style);
    write_char_format(out, &resolved_char_format(&base), ctx);
    write_paragraph_content(out, paragraph, &base, ctx);
    ctrl(out, "par");
    out.push(b'}');
}

fn write_paragraph_content(
    out: &mut Vec<u8>,
    paragraph: &Paragraph,
    base: &ResolvedChar,
    ctx: &Context<'_>,
) {
    let text = paragraph.text();
    let snippets = ctx.snippets.get(&paragraph.id);
    let mut boundaries: Vec<usize> = Vec::with_capacity(paragraph.runs().len() * 2 + 4);
    boundaries.push(0);
    boundaries.push(text.len());
    for run in paragraph.runs() {
        boundaries.push(run.range.start.min(text.len()));
        boundaries.push(run.range.end.min(text.len()));
    }
    let mut snippets_at: Vec<(usize, &[u8])> = Vec::new();
    if let Some(snippets) = snippets {
        for snippet in snippets {
            let offset = floor_char_boundary(text, snippet.offset.min(text.len()));
            boundaries.push(offset);
            snippets_at.push((offset, snippet.raw.as_slice()));
        }
    }
    // A note is written as its own destination group, so the paragraph text is cut at the
    // reference offset and the group is emitted there. Offsets are byte offsets in the
    // paragraph text; one that is past the end (or inside a character) is snapped to the
    // nearest position before it, which for a past-the-end offset is the paragraph end.
    let mut notes_at: Vec<(usize, &Note)> = Vec::with_capacity(paragraph.notes().len());
    for note in paragraph.notes() {
        let offset = floor_char_boundary(text, note.offset.min(text.len()));
        boundaries.push(offset);
        notes_at.push((offset, note));
    }
    let mut bookmarks_at: Vec<(usize, &str)> = Vec::with_capacity(paragraph.bookmarks().len());
    for bookmark in paragraph.bookmarks() {
        let offset = floor_char_boundary(text, bookmark.offset.min(text.len()));
        boundaries.push(offset);
        bookmarks_at.push((offset, bookmark.name.as_str()));
    }
    let mut images_at: Vec<(usize, &InlineImage)> = Vec::with_capacity(paragraph.images().len());
    for image in paragraph.images() {
        let offset = floor_char_boundary(text, image.offset.min(text.len()));
        boundaries.push(offset);
        images_at.push((offset, image));
    }
    // A cross-reference is a field whose cached result is the text in its range, so the
    // range ends are boundaries too and the field group wraps the pieces between them.
    let mut refs_at: Vec<(usize, usize, &CrossRef)> =
        Vec::with_capacity(paragraph.cross_refs().len());
    for cross_ref in paragraph.cross_refs() {
        let start = floor_char_boundary(text, cross_ref.range.start.min(text.len()));
        let end = floor_char_boundary(text, cross_ref.range.end.min(text.len()));
        if start < end {
            boundaries.push(start);
            boundaries.push(end);
            refs_at.push((start, end, cross_ref));
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    for (index, &start) in boundaries.iter().enumerate() {
        for _ in refs_at.iter().filter(|(_, end, _)| *end == start) {
            out.extend_from_slice(b"}}");
        }
        for (_, raw) in snippets_at.iter().filter(|(offset, _)| *offset == start) {
            out.extend_from_slice(raw);
        }
        for (_, note) in notes_at.iter().filter(|(offset, _)| *offset == start) {
            write_note(out, note);
        }
        for (_, name) in bookmarks_at.iter().filter(|(offset, _)| *offset == start) {
            write_bookmark(out, name);
        }
        for (_, image) in images_at.iter().filter(|(offset, _)| *offset == start) {
            write_picture(out, image);
        }
        for (_, _, cross_ref) in refs_at.iter().filter(|(begin, _, _)| *begin == start) {
            write_cross_ref_open(out, cross_ref);
        }
        let end = boundaries.get(index + 1).copied().unwrap_or(text.len());
        if start >= end {
            continue;
        }
        let format = paragraph.runs().iter().find_map(|run| {
            if run.range.start <= start && start < run.range.end {
                Some(&run.format)
            } else {
                None
            }
        });
        write_text_piece(out, &text[start..end], format, base, ctx);
    }
}

/// Opens a `REF` or `PAGEREF` field and its `\fldrslt` group; the caller writes the cached
/// text and closes both groups with `}}`.
fn write_cross_ref_open(out: &mut Vec<u8>, cross_ref: &CrossRef) {
    let word = match cross_ref.kind {
        CrossRefKind::Text => "REF",
        CrossRefKind::Page => "PAGEREF",
    };
    let target = if cross_ref.target.is_empty()
        || cross_ref
            .target
            .contains(|c: char| c.is_whitespace() || c == '"')
    {
        format!("\"{}\"", cross_ref.target.replace('"', "'"))
    } else {
        cross_ref.target.clone()
    };
    out.extend_from_slice(br"{\field{\*\fldinst ");
    // `\h` makes the field a link in Word; the backslash is escaped like any text.
    write_text(out, &format!("{word} {target} \\h "));
    out.extend_from_slice(br"}{\fldrslt ");
}

/// Writes one picture as a bare `{\pict ...}` group at its anchor. Word, WordPad, LibreOffice
/// and Pages all read this oldest form; `{\*\shppict}` only adds a shape wrapper that older
/// readers need a `\nonshppict` fallback for, which Libertum has no second encoding to put
/// in. The size is written as pixels (`\picw`/`\pich`, for the bytes) and as the displayed
/// size in twips (`\picwgoal`/`\pichgoal`); the scale is 100 %. Alt text and name travel in
/// a `\*\picprop` shape-property group that readers without it skip. The payload is hex, 64
/// bytes per line.
fn write_picture(out: &mut Vec<u8>, image: &InlineImage) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let media = &image.media;
    out.extend_from_slice(br"{\pict");
    out.extend_from_slice(match media.format() {
        ImageFormat::Png => br"\pngblip".as_slice(),
        ImageFormat::Jpeg => br"\jpegblip".as_slice(),
    });
    ctrl_i(out, "picscalex", 100);
    ctrl_i(out, "picscaley", 100);
    ctrl_i(out, "picw", media.width_px().min(i32::MAX as u32) as i32);
    ctrl_i(out, "pich", media.height_px().min(i32::MAX as u32) as i32);
    ctrl_i(out, "picwgoal", twips_i(image.width_pt));
    ctrl_i(out, "pichgoal", twips_i(image.height_pt));
    if !image.alt.is_empty() || !image.name.is_empty() {
        out.extend_from_slice(br"{\*\picprop");
        for (key, value) in [("wzName", &image.name), ("wzDescription", &image.alt)] {
            if !value.is_empty() {
                out.extend_from_slice(br"{\sp{\sn ");
                out.extend_from_slice(key.as_bytes());
                out.extend_from_slice(br"}{\sv ");
                write_text(out, value);
                out.extend_from_slice(b"}}");
            }
        }
        out.push(b'}');
    }
    out.push(b'\n');
    let bytes = media.bytes();
    out.reserve(bytes.len() * 2 + bytes.len() / 32 + 4);
    for line in bytes.chunks(64) {
        for &byte in line {
            out.push(HEX[usize::from(byte >> 4)]);
            out.push(HEX[usize::from(byte & 15)]);
        }
        out.push(b'\n');
    }
    out.push(b'}');
}

/// Writes a bookmark as the adjacent `{\*\bkmkstart Name}` and `{\*\bkmkend Name}` groups
/// that Word uses. Libertum stores a point, so the end group follows the start immediately.
fn write_bookmark(out: &mut Vec<u8>, name: &str) {
    out.extend_from_slice(br"{\*\bkmkstart ");
    write_text(out, name);
    out.push(b'}');
    out.extend_from_slice(br"{\*\bkmkend ");
    write_text(out, name);
    out.push(b'}');
}

/// Writes one note as a `{\footnote …}` or `{\endnote …}` destination group. The group marks
/// where the reference sits. The note body is plain text, matching the model.
fn write_note(out: &mut Vec<u8>, note: &Note) {
    out.push(b'{');
    // Word's RTF has no `\endnote` destination: it writes every note as `\footnote` and adds
    // `\ftnalt` for an endnote. That is also what our reader recognises.
    ctrl(out, "footnote");
    if note.kind == NoteKind::Endnote {
        ctrl(out, "ftnalt");
    }
    ctrl(out, "pard");
    ctrl(out, "plain");
    // `\chftn` is the reference mark a reader such as Word expects at the start of the note's
    // first paragraph. Libertum regenerates numbering, so the mark is not stored in the model.
    ctrl(out, "chftn");
    write_text(out, &note.text);
    out.push(b'}');
}

fn write_text_piece(
    out: &mut Vec<u8>,
    text: &str,
    format: Option<&CharFormat>,
    base: &ResolvedChar,
    ctx: &Context<'_>,
) {
    let controls = format.map(|format| run_controls(format, base, ctx));
    match controls {
        Some(controls) if !controls.is_empty() => {
            out.push(b'{');
            out.extend_from_slice(&controls);
            write_text(out, text);
            out.push(b'}');
        }
        _ => write_text(out, text),
    }
}

// ---------------------------------------------------------------------- tables in the body

fn write_table(out: &mut Vec<u8>, table: &Table, ctx: &Context<'_>) {
    let columns = table.widths_pt.len().max(1);
    if let Some(style) = &table.style {
        // An ignorable destination before the first row: Word and other readers skip it, and
        // Libertum's reader maps it back to the built-in table style.
        out.extend_from_slice(br"{\*\libertumtblstyle ");
        write_text(out, &style.name);
        out.push(b'}');
    }
    for (row_index, row) in table.rows.iter().enumerate() {
        ctrl(out, "trowd");
        if row_index < usize::from(table.header_rows) {
            ctrl(out, "trhdr");
        }
        if row.keep_together {
            ctrl(out, "trkeep");
        }
        if row.min_height_pt > 0.0 {
            ctrl_i(out, "trrh", twips_i(row.min_height_pt));
        }
        write_table_borders(out, &table.borders, ctx);
        // Right edge of every grid column, in twips.
        let mut edge = 0.0f32;
        let edges: Vec<i32> = (0..columns)
            .map(|column| {
                edge += table
                    .widths_pt
                    .get(column)
                    .copied()
                    .unwrap_or(MIN_TABLE_COLUMN_PT);
                twips_i(edge)
            })
            .collect();
        let edge_of = |column: usize| edges.get(column).or(edges.last()).copied().unwrap_or(0);
        // Cell definitions. In RTF each cell's properties (merge, shading) come *before* its
        // `\cellx` edge, and every definition precedes the first cell's content; Word ignores
        // them anywhere else.
        let mut column = 0usize;
        for cell in &row.cells {
            let span = usize::from(cell.col_span.max(1));
            let vertical_first = row_index + 1 < table.rows.len()
                && table
                    .cell_at_column(row_index + 1, column)
                    .is_some_and(|(_, below)| below.merged_into_above);
            if cell.merged_into_above {
                ctrl(out, "clvmrg");
            } else if vertical_first {
                ctrl(out, "clvmgf");
            }
            // The resolved fill is written, so a reader without Libertum's table styles still
            // shows the banding; the style name travels in `\*\libertumtblstyle` instead.
            if let Some(fill) = table.resolved_fill(row_index, cell) {
                ctrl_i(out, "clcbpat", ctx.colours.index(fill) as i32);
            }
            ctrl(out, "clvertalt");
            if span > 1 {
                ctrl(out, "clmgf");
            }
            ctrl_i(out, "cellx", edge_of(column));
            // A horizontally merged cell is stored as one model cell covering several grid
            // columns. RTF needs a cell for each covered column: the first carries the text,
            // the rest are empty continuations.
            for covered in 1..span {
                ctrl(out, "clmrg");
                ctrl(out, "clvertalt");
                ctrl_i(out, "cellx", edge_of(column + covered));
            }
            column += span;
        }
        // Grid columns beyond the row's cells keep their edges so the widths survive.
        for trailing in column..columns {
            ctrl_i(out, "cellx", edge_of(trailing));
        }
        for cell in &row.cells {
            for paragraph in &cell.paragraphs {
                write_paragraph(out, paragraph, true, ctx);
            }
            ctrl(out, "cell");
            for _ in 1..usize::from(cell.col_span.max(1)) {
                out.extend_from_slice(b"{\\pard\\plain\\intbl");
                ctrl(out, "par");
                out.extend_from_slice(b"}");
                ctrl(out, "cell");
            }
        }
        ctrl(out, "row");
    }
    ctrl(out, "pard");
}

fn write_table_borders(out: &mut Vec<u8>, borders: &TableBorders, ctx: &Context<'_>) {
    if !borders.is_visible() {
        return;
    }
    let width = twips_i(borders.width_pt).max(1);
    let colour = ctx.colours.index(borders.colour) as i32;
    let mut sides: Vec<&str> = Vec::new();
    if borders.outer {
        sides.extend(["trbrdrt", "trbrdrl", "trbrdrb", "trbrdrr"]);
    }
    if borders.inner {
        sides.extend(["trbrdrh", "trbrdrv"]);
    }
    for side in sides {
        ctrl(out, side);
        ctrl(out, "brdrs");
        ctrl_i(out, "brdrw", width);
        ctrl_i(out, "brdrcf", colour);
    }
}

// ---------------------------------------------------------------------- formatting

fn resolved_char_format(resolved: &ResolvedChar) -> CharFormat {
    CharFormat {
        family: Some(resolved.family.clone()),
        size_pt: Some(resolved.size_pt),
        weight: Some(resolved.weight),
        italic: Some(resolved.italic),
        underline: Some(resolved.underline),
        strikethrough: Some(resolved.strikethrough),
        colour: Some(resolved.colour),
        highlight: resolved.highlight.map(Some),
        vertical: Some(resolved.vertical),
        letter_spacing_pt: Some(resolved.letter_spacing_pt),
        ligatures: Some(resolved.ligatures),
    }
}

fn resolved_para_format(resolved: &ResolvedPara) -> ParaFormat {
    ParaFormat {
        align: Some(resolved.align),
        indent_left_pt: Some(resolved.indent_left_pt),
        indent_right_pt: Some(resolved.indent_right_pt),
        indent_first_pt: Some(resolved.indent_first_pt),
        space_before_pt: Some(resolved.space_before_pt),
        space_after_pt: Some(resolved.space_after_pt),
        line_spacing: Some(resolved.line_spacing),
        keep_with_next: Some(resolved.keep_with_next),
        keep_together: Some(resolved.keep_together),
        widow_control: Some(resolved.widow_control),
        hyphenate: Some(resolved.hyphenate),
        borders: None,
        tab_stops: resolved.tab_stops,
    }
}

fn write_char_format(out: &mut Vec<u8>, format: &CharFormat, ctx: &Context<'_>) {
    if let Some(family) = &format.family {
        if !family.is_empty() {
            ctrl_i(out, "f", ctx.fonts.index(family) as i32);
        }
    }
    if let Some(size) = format.size_pt {
        ctrl_i(out, "fs", (size * 2.0).round().clamp(1.0, 32767.0) as i32);
    }
    if let Some(weight) = format.weight {
        ctrl_flag(out, "b", weight >= 600);
    }
    if let Some(italic) = format.italic {
        ctrl_flag(out, "i", italic);
    }
    if let Some(underline) = format.underline {
        if underline {
            ctrl(out, "ul");
        } else {
            ctrl(out, "ulnone");
        }
    }
    if let Some(strike) = format.strikethrough {
        ctrl_flag(out, "strike", strike);
    }
    if let Some(vertical) = format.vertical {
        match vertical {
            VerticalAlign::Baseline => ctrl(out, "nosupersub"),
            VerticalAlign::Superscript => ctrl(out, "super"),
            VerticalAlign::Subscript => ctrl(out, "sub"),
        }
    }
    if let Some(colour) = format.colour {
        ctrl_i(out, "cf", ctx.colours.index(colour) as i32);
    }
    if let Some(highlight) = format.highlight {
        match highlight {
            Some(colour) => ctrl_i(out, "highlight", ctx.colours.index(colour) as i32),
            None => ctrl_i(out, "highlight", 0),
        }
    }
    if let Some(spacing) = format.letter_spacing_pt {
        ctrl_i(out, "expnd", (spacing * 20.0).round() as i32);
    }
}

fn write_para_format(out: &mut Vec<u8>, format: &ParaFormat, default_size_pt: f32) {
    if let Some(align) = format.align {
        write_align(out, align);
    }
    if let Some(indent) = format.indent_left_pt {
        ctrl_i(out, "li", twips_i(indent));
    }
    if let Some(indent) = format.indent_right_pt {
        ctrl_i(out, "ri", twips_i(indent));
    }
    if let Some(indent) = format.indent_first_pt {
        ctrl_i(out, "fi", twips_i(indent));
    }
    if let Some(space) = format.space_before_pt {
        ctrl_i(out, "sb", twips_i(space));
    }
    if let Some(space) = format.space_after_pt {
        ctrl_i(out, "sa", twips_i(space));
    }
    if let Some(spacing) = format.line_spacing {
        match spacing {
            LineSpacing::Multiple(multiple) => {
                ctrl_i(out, "sl", (multiple * 240.0).round().max(1.0) as i32);
                ctrl_i(out, "slmult", 1);
            }
            LineSpacing::Exactly(points) => {
                ctrl_i(out, "sl", twips_i(points).max(1));
                ctrl_i(out, "slmult", 0);
            }
            LineSpacing::AtLeast(points) => {
                ctrl_i(out, "sl", -twips_i(points).max(1));
                ctrl_i(out, "slmult", 0);
            }
            LineSpacing::Leading(extra) => {
                let height = (default_size_pt * 1.2 + extra).max(1.0);
                ctrl_i(out, "sl", twips_i(height).max(1));
                ctrl_i(out, "slmult", 0);
            }
        }
    }
    if let Some(keep) = format.keep_together {
        ctrl_flag(out, "keep", keep);
    }
    if let Some(keep) = format.keep_with_next {
        ctrl_flag(out, "keepn", keep);
    }
    if let Some(widow) = format.widow_control {
        if widow {
            ctrl(out, "widctlpar");
        } else {
            ctrl(out, "nowidctlpar");
        }
    }
    if let Some(hyphenate) = format.hyphenate {
        ctrl_i(out, "hyphpar", i32::from(hyphenate));
    }
    if let Some(stops) = format.tab_stops {
        for stop in stops.iter() {
            match stop.kind {
                TabStopKind::Left => ctrl(out, "tql"),
                TabStopKind::Centre => ctrl(out, "tqc"),
                TabStopKind::Right => ctrl(out, "tqr"),
                TabStopKind::Decimal => ctrl(out, "tqd"),
            }
            ctrl_i(out, "tx", twips_i(stop.position_pt));
        }
    }
}

fn write_align(out: &mut Vec<u8>, align: Align) {
    match align {
        Align::Start => ctrl(out, "ql"),
        Align::Centre => ctrl(out, "qc"),
        Align::End => ctrl(out, "qr"),
        Align::Justify => ctrl(out, "qj"),
    }
}

fn run_controls(format: &CharFormat, base: &ResolvedChar, ctx: &Context<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(family) = &format.family {
        if !family.is_empty() && **family != *base.family {
            ctrl_i(&mut out, "f", ctx.fonts.index(family) as i32);
        }
    }
    if let Some(size) = format.size_pt {
        if (size - base.size_pt).abs() > 0.001 {
            ctrl_i(
                &mut out,
                "fs",
                (size * 2.0).round().clamp(1.0, 32767.0) as i32,
            );
        }
    }
    if let Some(weight) = format.weight {
        let bold = weight >= 600;
        if bold != (base.weight >= 600) {
            ctrl_flag(&mut out, "b", bold);
        }
    }
    if let Some(italic) = format.italic {
        if italic != base.italic {
            ctrl_flag(&mut out, "i", italic);
        }
    }
    if let Some(underline) = format.underline {
        if underline != base.underline {
            if underline {
                ctrl(&mut out, "ul");
            } else {
                ctrl(&mut out, "ulnone");
            }
        }
    }
    if let Some(strike) = format.strikethrough {
        if strike != base.strikethrough {
            ctrl_flag(&mut out, "strike", strike);
        }
    }
    if let Some(vertical) = format.vertical {
        if vertical != base.vertical {
            match vertical {
                VerticalAlign::Baseline => ctrl(&mut out, "nosupersub"),
                VerticalAlign::Superscript => ctrl(&mut out, "super"),
                VerticalAlign::Subscript => ctrl(&mut out, "sub"),
            }
        }
    }
    if let Some(colour) = format.colour {
        if colour != base.colour {
            ctrl_i(&mut out, "cf", ctx.colours.index(colour) as i32);
        }
    }
    if let Some(highlight) = format.highlight {
        match highlight {
            Some(colour) if base.highlight != Some(colour) => {
                ctrl_i(&mut out, "highlight", ctx.colours.index(colour) as i32);
            }
            None if base.highlight.is_some() => ctrl_i(&mut out, "highlight", 0),
            _ => {}
        }
    }
    if let Some(spacing) = format.letter_spacing_pt {
        if (spacing - base.letter_spacing_pt).abs() > 0.001 {
            ctrl_i(&mut out, "expnd", (spacing * 20.0).round() as i32);
        }
    }
    out
}

// ---------------------------------------------------------------------- text and tokens

fn ctrl(out: &mut Vec<u8>, name: &str) {
    out.push(b'\\');
    out.extend_from_slice(name.as_bytes());
    out.push(b' ');
}

fn ctrl_i(out: &mut Vec<u8>, name: &str, value: i32) {
    out.push(b'\\');
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(value.to_string().as_bytes());
    out.push(b' ');
}

fn ctrl_flag(out: &mut Vec<u8>, name: &str, on: bool) {
    if on {
        ctrl(out, name);
    } else {
        out.push(b'\\');
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b"0 ");
    }
}

fn twips_i(points: f32) -> i32 {
    if points.is_finite() {
        (points * 20.0).round() as i32
    } else {
        0
    }
}

fn write_text(out: &mut Vec<u8>, text: &str) {
    for character in text.chars() {
        match character {
            '\\' | '{' | '}' => {
                out.push(b'\\');
                out.push(character as u8);
            }
            '\t' => out.extend_from_slice(b"\\tab "),
            '\n' | '\r' => out.extend_from_slice(b"\\line "),
            character if character.is_ascii() && (character as u32) >= 0x20 => {
                out.push(character as u8);
            }
            character => write_unicode_char(out, character),
        }
    }
}

/// Writes one non-ASCII character as one or two `\uN?` escapes (signed UTF-16 code units).
fn write_unicode_char(out: &mut Vec<u8>, character: char) {
    let mut units = [0u16; 2];
    for unit in character.encode_utf16(&mut units) {
        let value = i32::from(*unit);
        let signed = if value >= 0x8000 {
            value - 0x1_0000
        } else {
            value
        };
        out.extend_from_slice(b"\\u");
        out.extend_from_slice(signed.to_string().as_bytes());
        out.push(b'?');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(text: &str) -> String {
        let mut out = Vec::new();
        write_text(&mut out, text);
        String::from_utf8(out).expect("ASCII RTF")
    }

    #[test]
    fn escapes_braces_and_backslashes() {
        assert_eq!(written(r"a{b}c\d"), r"a\{b\}c\\d");
    }

    #[test]
    fn tabs_become_control_words() {
        assert_eq!(written("a\tb"), "a\\tab b");
    }

    #[test]
    fn non_ascii_is_written_as_signed_unicode_units() {
        assert_eq!(written("é"), "\\u233?");
        assert_eq!(written("•"), "\\u8226?");
    }

    #[test]
    fn a_surrogate_pair_uses_two_unicode_units() {
        assert_eq!(written("😀"), "\\u-10179?\\u-8704?");
    }

    #[test]
    fn run_controls_only_write_what_differs_from_the_style() {
        let styles = StyleSheet::with_defaults("Georgia", 12.0);
        let ctx = Context {
            styles: &styles,
            fonts: FontTable::collect(&Document::new()),
            colours: ColourTable::collect(&Document::new()),
            lists: ListPlan::collect(&Document::new()),
            snippets: &HashMap::new(),
            header_snippets: &HashMap::new(),
            stylesheet_extras: &[],
        };
        let base = styles.resolve_char(StyleId::BODY);
        let controls = run_controls(&CharFormat::bold(), &base, &ctx);
        assert_eq!(String::from_utf8(controls).expect("ASCII"), "\\b ");
        let controls = run_controls(
            &CharFormat {
                size_pt: Some(18.0),
                ..CharFormat::default()
            },
            &base,
            &ctx,
        );
        assert_eq!(String::from_utf8(controls).expect("ASCII"), "\\fs36 ");
    }
}
