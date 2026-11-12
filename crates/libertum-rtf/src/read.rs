//! Reading an RTF stream into the model (PLAN §6.5).
//!
//! The reader walks the byte tokens once. It never builds a tree for the whole stream: a
//! paragraph is accumulated as it arrives, then added to the block list (or the current table
//! cell) and forgotten. Groups save and restore the formatting state, as RTF specifies.
//!
//! Unknown groups and control words are copied as raw bytes. A fragment met inside body text
//! is attached to the paragraph being built at the byte offset where it appeared; a fragment
//! met at top level goes to the document preamble; a fragment inside a header or footer is
//! attached to that furniture variant.

use std::collections::HashMap;
use std::sync::Arc;

use libertum_model::{
    Align, Block, BlockId, Bookmark, CharFormat, CrossRef, CrossRefKind, Document, HeaderFooter,
    HeadersFooters, HfKind, HfVariant, InlineImage, LineSpacing, ListItem, ListStyle,
    MAX_LIST_LEVEL, MIN_TABLE_COLUMN_PT, Media, Note, NoteKind, Orientation, PaperSize, ParaFormat,
    ParaProps, Paragraph, Run, Section, StyleId, TabStop, TabStopKind, Table, TableBorders,
    TableCell, TableRow, TableStyle,
};

use crate::encoding::TextDecoder;
use crate::token::{Token, Tokenizer};
use crate::{HeaderKey, RtfError, Snippet};

/// Deepest group nesting accepted. RTF has no limit; this keeps a malicious or broken file
/// from exhausting the stack and is far deeper than any real document.
const MAX_GROUP_DEPTH: u32 = 128;

pub(crate) struct Parsed {
    pub(crate) document: Document,
    pub(crate) preamble: Vec<Vec<u8>>,
    pub(crate) snippets: HashMap<BlockId, Vec<Snippet>>,
    pub(crate) header_snippets: HashMap<HeaderKey, Vec<Vec<u8>>>,
    pub(crate) stylesheet_extras: Vec<Vec<u8>>,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn read(bytes: &[u8]) -> Result<Parsed, RtfError> {
    let mut reader = Reader::new(bytes);
    reader.run()?;
    reader.finish()
}

/// Where text currently goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sink {
    Body,
    Header,
    Instruction,
    /// Inside a `\footnote`/`\endnote` destination: text goes to the note being built.
    Note,
    Skip,
}

/// A note destination being accumulated as plain text.
struct NoteBuilder {
    kind: NoteKind,
    /// Byte offset in the paragraph text where the reference was met.
    offset: usize,
    text: String,
    /// Whether the source applied formatting to the note; that is flattened away.
    styled: bool,
}

/// The paragraph properties the reader tracks. They persist across `\par`, as RTF paragraph
/// formatting does; `\pard` resets them.
#[derive(Clone, Debug)]
struct ParaState {
    format: ParaFormat,
    style: StyleId,
    outline_level: Option<u8>,
    list: Option<ListItem>,
}

impl Default for ParaState {
    fn default() -> Self {
        Self {
            format: ParaFormat::default(),
            style: StyleId::BODY,
            outline_level: None,
            list: None,
        }
    }
}

/// One paragraph being accumulated.
struct PendingParagraph {
    id: BlockId,
    text: String,
    runs: Vec<Run>,
    props: ParaProps,
    snippets: Vec<Snippet>,
    notes: Vec<Note>,
    bookmarks: Vec<Bookmark>,
    images: Vec<InlineImage>,
    cross_refs: Vec<CrossRef>,
}

impl PendingParagraph {
    fn new(id: BlockId, props: ParaProps) -> Self {
        Self {
            id,
            text: String::new(),
            runs: Vec::new(),
            props,
            snippets: Vec::new(),
            notes: Vec::new(),
            bookmarks: Vec::new(),
            images: Vec::new(),
            cross_refs: Vec::new(),
        }
    }

    fn append(&mut self, text: &str, format: &CharFormat) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.runs.push(Run {
            range: start..self.text.len(),
            format: format.clone(),
        });
    }

    fn add_snippet(&mut self, raw: Vec<u8>) {
        self.snippets.push(Snippet {
            offset: self.text.len(),
            raw,
        });
    }

    fn into_paragraph(mut self) -> Paragraph {
        let notes = std::mem::take(&mut self.notes);
        let bookmarks = std::mem::take(&mut self.bookmarks);
        let images = std::mem::take(&mut self.images);
        let cross_refs = std::mem::take(&mut self.cross_refs);
        let mut paragraph =
            Paragraph::with_runs(self.id, self.text, self.runs).with_props(self.props);
        if !notes.is_empty() {
            paragraph.set_notes(notes);
        }
        if !bookmarks.is_empty() {
            paragraph.set_bookmarks(bookmarks);
        }
        if !images.is_empty() {
            paragraph.set_images(images);
        }
        if !cross_refs.is_empty() {
            paragraph.set_cross_refs(cross_refs);
        }
        paragraph
    }
}

/// A header or footer being accumulated as plain text (the model's furniture is one paragraph).
struct HeaderBuilder {
    text: String,
    align: Align,
    paragraphs: usize,
    target: (HfKind, HfVariant),
}

impl HeaderBuilder {
    fn new(target: (HfKind, HfVariant)) -> Self {
        Self {
            text: String::new(),
            align: Align::Start,
            paragraphs: 0,
            target,
        }
    }
}

/// A style definition from `\stylesheet`.
#[derive(Clone, Debug, Default)]
struct StyleDef {
    index: i32,
    name: String,
    based_on: Option<i32>,
    next: Option<i32>,
    char: CharFormat,
    para: ParaFormat,
    outline: Option<u8>,
}

/// One list level from `\listtable`.
#[derive(Clone, Copy, Debug, Default)]
struct LevelDef {
    nfc: i32,
}

#[derive(Clone, Copy, Debug)]
struct ListDef {
    style: ListStyle,
}

/// Table cell merge flags from `\clmgf`/`\clmrg`/`\clvmgf`/`\clvmrg`.
#[derive(Clone, Copy, Debug, Default)]
struct MergeFlags {
    h_first: bool,
    h_continue: bool,
    v_first: bool,
    v_continue: bool,
    fill: Option<[u8; 4]>,
}

impl MergeFlags {
    fn is_empty(&self) -> bool {
        !(self.h_first || self.h_continue || self.v_first || self.v_continue || self.fill.is_some())
    }
}

/// Table border state from `\trbrdr*`, `\brdrw` and `\brdrcf`.
#[derive(Clone, Copy, Debug, Default)]
struct BorderFlags {
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
    inner_h: bool,
    inner_v: bool,
    width_twips: Option<i32>,
    colour: Option<[u8; 4]>,
}

struct TableBuilder {
    rows: Vec<TableRow>,
    widths: Vec<f32>,
    cellx: Vec<f32>,
    tr_left: f32,
    row: Vec<TableCell>,
    cell_paragraphs: Vec<Paragraph>,
    row_started: bool,
    cell_open: bool,
    row_header: bool,
    /// `\trkeep` seen in the current row.
    row_keep: bool,
    still_header: bool,
    header_rows: u8,
    row_min_height_pt: f32,
    /// Properties set since the last `\cellx` or `\cell`.
    merge: MergeFlags,
    /// The cell definitions of the current row: the properties that preceded each `\cellx`.
    defs: Vec<MergeFlags>,
    /// `\cell` marks seen in the current row.
    cells_done: usize,
    merge_open: bool,
    borders: BorderFlags,
    /// Name from a `{\*\libertumtblstyle ...}` group that preceded the first row.
    style_name: Option<String>,
}

impl TableBuilder {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            widths: Vec::new(),
            cellx: Vec::new(),
            tr_left: 0.0,
            row: Vec::new(),
            cell_paragraphs: Vec::new(),
            row_started: false,
            cell_open: false,
            row_header: false,
            row_keep: false,
            still_header: true,
            header_rows: 0,
            row_min_height_pt: 0.0,
            merge: MergeFlags::default(),
            defs: Vec::new(),
            cells_done: 0,
            merge_open: false,
            borders: BorderFlags::default(),
            style_name: None,
        }
    }

    fn start_row(&mut self) {
        self.cellx.clear();
        self.tr_left = 0.0;
        self.row.clear();
        self.cell_paragraphs.clear();
        self.row_started = true;
        self.cell_open = false;
        self.row_header = false;
        self.row_keep = false;
        self.row_min_height_pt = 0.0;
        self.merge = MergeFlags::default();
        self.defs.clear();
        self.cells_done = 0;
        self.merge_open = false;
    }

    /// The properties of the next cell. RTF puts them before the cell's `\cellx` (recorded in
    /// `defs`); files written by earlier Libertum versions put them just before the cell's
    /// content instead, which is `merge`.
    fn next_cell_flags(&mut self) -> MergeFlags {
        let defined = self.defs.get(self.cells_done).copied().unwrap_or_default();
        self.cells_done += 1;
        let pending = std::mem::take(&mut self.merge);
        if defined.is_empty() { pending } else { defined }
    }
}

pub(crate) struct Reader<'a> {
    tokenizer: Tokenizer<'a>,
    decoder: TextDecoder,
    uc: u32,
    fallback: u32,
    fonts: HashMap<i32, String>,
    deff: i32,
    colours: Vec<[u8; 4]>,
    styles: Vec<StyleDef>,
    stylesheet_extras: Vec<Vec<u8>>,
    style_ids: HashMap<i32, StyleId>,
    lists: HashMap<i32, ListDef>,
    overrides: HashMap<i32, i32>,
    blocks: Vec<Block>,
    sections: Vec<Section>,
    section: Section,
    paper_w: Option<f32>,
    paper_h: Option<f32>,
    landscape: Option<bool>,
    pending_section_start: bool,
    pending_page_break: bool,
    section_touched: bool,
    para: ParaState,
    list_level: u8,
    char_state: CharFormat,
    sl_twips: Option<i32>,
    sl_mult: i32,
    tab_kind: TabStopKind,
    current: Option<PendingParagraph>,
    table: Option<TableBuilder>,
    sink: Sink,
    note: Option<NoteBuilder>,
    header: Option<HeaderBuilder>,
    field_instruction: String,
    /// A table style name read ahead of the table it belongs to.
    pending_table_style: Option<String>,
    /// Pictures read so far (to tell whether a `\shppict` group held one).
    pictures_read: usize,
    /// A `\shppict` group just produced a picture, so a directly following `\nonshppict`
    /// fallback is skipped.
    shppict_done: bool,
    preamble: Vec<Vec<u8>>,
    snippets: HashMap<BlockId, Vec<Snippet>>,
    header_snippets: HashMap<HeaderKey, Vec<Vec<u8>>>,
    warnings: Vec<String>,
    next_id: u64,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        let (decoder, fallback) = TextDecoder::new(1252);
        let mut warnings = Vec::new();
        if let Some(code_page) = fallback {
            warnings.push(format!(
                "the code page is not known; CP{code_page} was used instead"
            ));
        }
        Self {
            tokenizer: Tokenizer::new(bytes),
            decoder,
            uc: 1,
            fallback: 0,
            fonts: HashMap::new(),
            deff: 0,
            colours: Vec::new(),
            styles: Vec::new(),
            stylesheet_extras: Vec::new(),
            style_ids: HashMap::new(),
            lists: HashMap::new(),
            overrides: HashMap::new(),
            blocks: Vec::new(),
            sections: Vec::new(),
            section: Section::default(),
            paper_w: None,
            paper_h: None,
            landscape: None,
            pending_section_start: false,
            pending_page_break: false,
            section_touched: false,
            para: ParaState::default(),
            list_level: 0,
            char_state: CharFormat::default(),
            sl_twips: None,
            sl_mult: 0,
            tab_kind: TabStopKind::Left,
            current: None,
            table: None,
            sink: Sink::Body,
            note: None,
            header: None,
            field_instruction: String::new(),
            pending_table_style: None,
            pictures_read: 0,
            shppict_done: false,
            preamble: Vec::new(),
            snippets: HashMap::new(),
            header_snippets: HashMap::new(),
            warnings,
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    fn run(&mut self) -> Result<(), RtfError> {
        let start = self.tokenizer.position();
        match self.tokenizer.next_token()? {
            Some(Token::GroupStart) => {
                self.parse_contents(0)?;
                self.tokenizer.set_position(self.tokenizer.input().len());
                Ok(())
            }
            Some(token) => {
                self.handle_token(token, start, 0)?;
                self.parse_until_eof(0)
            }
            None => Ok(()),
        }
    }

    fn parse_until_eof(&mut self, depth: u32) -> Result<(), RtfError> {
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Ok(()),
                Some(Token::GroupEnd) => {
                    return Err(RtfError::parse(start, "unexpected closing brace"));
                }
                Some(Token::GroupStart) => self.parse_nested_group(start, depth + 1)?,
                Some(token) => self.handle_token(token, start, depth)?,
            }
        }
    }

    /// Parses inside a group until its closing brace. `depth` is the nesting depth of the
    /// group being parsed (0 for the root).
    fn parse_contents(&mut self, depth: u32) -> Result<(), RtfError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(RtfError::parse(
                self.tokenizer.position(),
                "groups are nested too deeply",
            ));
        }
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => {
                    return Err(RtfError::parse(
                        self.tokenizer.position(),
                        "unexpected end of the stream inside a group",
                    ));
                }
                Some(Token::GroupEnd) => return Ok(()),
                Some(Token::GroupStart) => self.parse_nested_group(start, depth + 1)?,
                Some(token) => self.handle_token(token, start, depth)?,
            }
        }
    }

    fn parse_nested_group(&mut self, _group_start: usize, depth: u32) -> Result<(), RtfError> {
        // The caller's start may point at whitespace before the brace (the tokeniser skips
        // CR/LF between tokens). The actual group starts one byte behind the cursor.
        let group_start = self.tokenizer.position().saturating_sub(1);
        if depth > MAX_GROUP_DEPTH {
            return Err(RtfError::parse(group_start, "groups are nested too deeply"));
        }
        let first_start = self.tokenizer.position();
        let Some(first) = self.tokenizer.next_token()? else {
            return Err(RtfError::parse(group_start, "unterminated group"));
        };
        match first {
            Token::GroupEnd => Ok(()),
            Token::ControlSymbol(b'*') => {
                let after = self.tokenizer.next_token()?;
                match after {
                    Some(Token::ControlWord { name, .. }) if is_destination(name) => {
                        self.destination(name, group_start, depth)
                    }
                    _ => self.preserve_raw_group(group_start),
                }
            }
            Token::ControlWord { name, .. } if is_destination(name) => {
                self.destination(name, group_start, depth)
            }
            token => {
                // A formatting group such as `{\b bold}` or `{\{`.
                let saved_char = self.char_state.clone();
                let saved_para = self.para.clone();
                self.handle_token(token, first_start, depth)?;
                self.parse_contents(depth)?;
                self.char_state = saved_char;
                self.para = saved_para;
                Ok(())
            }
        }
    }

    fn destination(&mut self, name: &str, group_start: usize, depth: u32) -> Result<(), RtfError> {
        if name != "nonshppict" {
            self.shppict_done = false;
        }
        match name {
            "fonttbl" => self.parse_font_table(depth + 1),
            "colortbl" => self.parse_colour_table(depth + 1),
            "stylesheet" => self.parse_style_sheet(depth + 1),
            "listtable" => self.parse_list_table(depth + 1),
            "listoverridetable" => self.parse_list_override_table(depth + 1),
            "header" | "headerr" | "headerl" | "headerf" | "footer" | "footerr" | "footerl"
            | "footerf" => {
                let (kind, variant, right) = header_selector(name);
                self.parse_header_group(kind, variant, right, depth + 1)
            }
            "field" => self.parse_field_group(group_start, depth + 1),
            "pict" => self.parse_picture(group_start, depth + 1),
            "shppict" => {
                let before = self.pictures_read;
                self.parse_contents(depth + 1)?;
                self.shppict_done = self.pictures_read > before;
                Ok(())
            }
            "nonshppict" => {
                // The fallback of a `\shppict` group: when that group gave a picture, this
                // one shows the same image in another encoding and is skipped.
                if self.shppict_done {
                    self.shppict_done = false;
                    self.tokenizer.take_raw_group(group_start)?;
                    Ok(())
                } else {
                    self.parse_contents(depth + 1)
                }
            }
            "libertumtblstyle" => self.parse_table_style_group(),
            "shp" | "shpinst" | "shprslt" | "object" | "objdata" => {
                Err(RtfError::unsupported("a drawing object or shape"))
            }
            "footnote" => self.parse_note_group(NoteKind::Footnote, depth + 1),
            "endnote" => self.parse_note_group(NoteKind::Endnote, depth + 1),
            "bkmkstart" => self.parse_bookmark_group(depth + 1),
            "bkmkend" => {
                self.tokenizer.take_raw_group(group_start)?;
                Ok(())
            }
            "annotation" | "atnid" | "atnauthor" | "atnref" | "atndate" | "comment" => {
                Err(RtfError::unsupported("an annotation or comment"))
            }
            _ => self.preserve_raw_group(group_start),
        }
    }

    // ------------------------------------------------------------------ fonts

    fn parse_font_table(&mut self, depth: u32) -> Result<(), RtfError> {
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated font table")),
                Some(Token::GroupEnd) => return Ok(()),
                Some(Token::GroupStart) => self.parse_font_entry(depth)?,
                Some(_) => {}
            }
        }
    }

    fn parse_font_entry(&mut self, _depth: u32) -> Result<(), RtfError> {
        let mut index = None;
        let mut name = String::new();
        let mut done = false;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated font entry")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::ControlWord { name: word, param }) => {
                    if word == "f" {
                        index = param;
                    }
                }
                Some(Token::HexByte(byte)) => {
                    if !done {
                        append_metadata(&mut name, &[byte], self.decoder.codepage(), &mut done);
                    }
                }
                Some(Token::Text(bytes)) => {
                    if !done {
                        append_metadata(&mut name, bytes, self.decoder.codepage(), &mut done);
                    }
                }
                Some(Token::ControlSymbol(_)) => {}
            }
        }
        if let Some(index) = index {
            let name = name.trim().to_owned();
            if !name.is_empty() {
                self.fonts.insert(index, name);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ colours

    fn parse_colour_table(&mut self, _depth: u32) -> Result<(), RtfError> {
        let mut red = 0u8;
        let mut green = 0u8;
        let mut blue = 0u8;
        let mut have = false;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated colour table")),
                Some(Token::GroupEnd) => {
                    if have {
                        self.colours.push([red, green, blue, 255]);
                    }
                    return Ok(());
                }
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::ControlWord { name, param }) => {
                    let value = param.unwrap_or(0).clamp(0, 255) as u8;
                    match name {
                        "red" => {
                            red = value;
                            have = true;
                        }
                        "green" => {
                            green = value;
                            have = true;
                        }
                        "blue" => {
                            blue = value;
                            have = true;
                        }
                        _ => {}
                    }
                }
                Some(Token::Text(bytes)) => {
                    for &byte in bytes {
                        if byte == b';' {
                            self.colours.push([red, green, blue, 255]);
                            red = 0;
                            green = 0;
                            blue = 0;
                            have = false;
                        }
                    }
                }
                Some(Token::HexByte(_)) | Some(Token::ControlSymbol(_)) => {}
            }
        }
    }

    // ------------------------------------------------------------------ styles

    fn parse_style_sheet(&mut self, depth: u32) -> Result<(), RtfError> {
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated style sheet")),
                Some(Token::GroupEnd) => {
                    self.assign_style_ids();
                    return Ok(());
                }
                Some(Token::GroupStart) => {
                    self.parse_style_entry(start, depth)?;
                }
                Some(_) => {}
            }
        }
    }

    /// Maps RTF `\s` indices to model style ids. Built-in names map to the model's fixed ids;
    /// other styles are appended in encounter order after them.
    fn assign_style_ids(&mut self) {
        let mut ids: HashMap<i32, StyleId> = HashMap::new();
        ids.insert(0, StyleId::BODY);
        // Custom styles start after the built-ins; Heading 4 is the last of them.
        let mut next_custom = StyleId::HEADING_4.0 + 1;
        for style in &self.styles {
            if style.index == 0 || is_normal_name(&style.name) {
                ids.insert(style.index, StyleId::BODY);
            } else if let Some(id) = builtin_style(&style.name) {
                ids.insert(style.index, id);
            } else if let std::collections::hash_map::Entry::Vacant(entry) = ids.entry(style.index)
            {
                entry.insert(StyleId(next_custom));
                next_custom += 1;
            }
        }
        self.style_ids = ids;
    }

    fn parse_style_entry(&mut self, group_start: usize, depth: u32) -> Result<(), RtfError> {
        let Some(first) = self.tokenizer.next_token()? else {
            return Err(RtfError::parse(group_start, "unterminated style entry"));
        };
        // Character and table styles (`{\*\cs15 ...;}`, `{\*\ts11 ...;}`) are not modelled;
        // keep the raw group so a save can put it back inside the generated style sheet.
        if first == Token::ControlSymbol(b'*') {
            let saved = self.tokenizer.take_raw_group(group_start)?.to_vec();
            self.stylesheet_extras.push(saved);
            return Ok(());
        }
        let mut def = StyleDef::default();
        let mut sl_twips = None;
        let mut sl_mult = 0;
        let mut tab_kind = TabStopKind::Left;
        let mut name_done = false;
        let mut first = Some(first);
        loop {
            let start = self.tokenizer.position();
            let token = match first.take() {
                Some(token) => token,
                None => match self.tokenizer.next_token()? {
                    Some(token) => token,
                    None => return Err(RtfError::parse(start, "unterminated style entry")),
                },
            };
            match token {
                Token::GroupEnd => break,
                Token::GroupStart => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Token::ControlWord { name, param } => match name {
                    "s" => def.index = param.unwrap_or(0),
                    "sbasedon" => def.based_on = param,
                    "snext" => def.next = param,
                    "outlinelevel" => {
                        def.outline = param.map(|level| (level.clamp(0, 8) as u8) + 1);
                    }
                    "plain" => def.char = CharFormat::default(),
                    "pard" => def.para = ParaFormat::default(),
                    "hyphpar" => def.para.hyphenate = Some(param != Some(0)),
                    _ => {
                        if !self.apply_char_control(name, param, &mut def.char) {
                            let _ = Self::apply_para_control(
                                name,
                                param,
                                &mut def.para,
                                &mut sl_twips,
                                &mut sl_mult,
                                &mut tab_kind,
                            );
                        }
                    }
                },
                Token::HexByte(byte) => {
                    if !name_done {
                        append_metadata(
                            &mut def.name,
                            &[byte],
                            self.decoder.codepage(),
                            &mut name_done,
                        );
                    }
                }
                Token::Text(bytes) => {
                    if !name_done {
                        append_metadata(
                            &mut def.name,
                            bytes,
                            self.decoder.codepage(),
                            &mut name_done,
                        );
                    }
                }
                Token::ControlSymbol(_) => {}
            }
        }
        let _ = depth;
        if def.index == 0 && def.name.trim().is_empty() {
            def.name = "Normal".to_owned();
        }
        if let Some(index) = self
            .styles
            .iter()
            .position(|style| style.index == def.index)
        {
            self.styles[index] = def;
        } else {
            self.styles.push(def);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ lists

    fn parse_list_table(&mut self, depth: u32) -> Result<(), RtfError> {
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated list table")),
                Some(Token::GroupEnd) => return Ok(()),
                Some(Token::GroupStart) => self.parse_list_entry(depth)?,
                Some(_) => {}
            }
        }
    }

    fn parse_list_entry(&mut self, depth: u32) -> Result<(), RtfError> {
        let mut list_id = None;
        let mut levels: Vec<LevelDef> = Vec::new();
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated list")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => {
                    let first_start = self.tokenizer.position();
                    let Some(first) = self.tokenizer.next_token()? else {
                        return Err(RtfError::parse(first_start, "unterminated list"));
                    };
                    match first {
                        Token::ControlWord {
                            name: "listlevel", ..
                        } => {
                            levels.push(self.parse_list_level(depth)?);
                        }
                        _ => {
                            self.tokenizer.take_raw_group(start)?;
                        }
                    }
                }
                Some(Token::ControlWord { name, param }) => {
                    if name == "listid" {
                        list_id = param;
                    }
                }
                Some(_) => {}
            }
        }
        if let Some(list_id) = list_id {
            let nfc = levels.first().map_or(0, |level| level.nfc);
            let style = if nfc == 23 {
                ListStyle::Bullet
            } else if nfc == 3 || nfc == 4 {
                ListStyle::Lettered
            } else if nfc == 1 || nfc == 2 {
                ListStyle::Harvard
            } else {
                ListStyle::Numbered
            };
            self.lists.insert(list_id, ListDef { style });
        }
        Ok(())
    }

    fn parse_list_level(&mut self, depth: u32) -> Result<LevelDef, RtfError> {
        let mut level = LevelDef::default();
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated list level")),
                Some(Token::GroupEnd) => return Ok(level),
                Some(Token::GroupStart) => {
                    let first_start = self.tokenizer.position();
                    let Some(first) = self.tokenizer.next_token()? else {
                        return Err(RtfError::parse(first_start, "unterminated list level"));
                    };
                    match first {
                        Token::ControlWord {
                            name: "leveltext", ..
                        } => {
                            let _ = self.parse_level_text(depth)?;
                        }
                        _ => {
                            self.tokenizer.take_raw_group(start)?;
                        }
                    }
                }
                Some(Token::ControlWord { name, param }) => {
                    if name == "levelnfc" {
                        level.nfc = param.unwrap_or(0);
                    }
                }
                Some(_) => {}
            }
        }
    }

    fn parse_level_text(&mut self, _depth: u32) -> Result<String, RtfError> {
        let mut text = String::new();
        let mut seen_length = false;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated level text")),
                Some(Token::GroupEnd) => return Ok(text),
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::HexByte(byte)) => {
                    if !seen_length {
                        seen_length = true;
                    } else if byte != 0 {
                        text.push(char::from(byte));
                    } else {
                        text.push('\u{0}');
                    }
                }
                Some(Token::Text(bytes)) => {
                    if !seen_length {
                        seen_length = true;
                    }
                    text.push_str(&metadata_text(bytes, self.decoder.codepage()));
                }
                Some(Token::ControlSymbol(_)) => {}
                Some(Token::ControlWord { name, param }) => {
                    if name == "u" {
                        if let Some(value) = param {
                            let unit = if value < 0 {
                                (value + 65536) as u16
                            } else {
                                value.min(65535) as u16
                            };
                            if let Some(character) = char::from_u32(u32::from(unit)) {
                                text.push(character);
                            }
                        }
                    }
                }
            }
        }
    }

    fn parse_list_override_table(&mut self, depth: u32) -> Result<(), RtfError> {
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated list override table")),
                Some(Token::GroupEnd) => return Ok(()),
                Some(Token::GroupStart) => self.parse_list_override(depth)?,
                Some(_) => {}
            }
        }
    }

    fn parse_list_override(&mut self, _depth: u32) -> Result<(), RtfError> {
        let mut list_id = None;
        let mut ls = None;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated list override")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::ControlWord { name, param }) => match name {
                    "listid" => list_id = param,
                    "ls" => ls = param,
                    _ => {}
                },
                Some(_) => {}
            }
        }
        if let (Some(list_id), Some(ls)) = (list_id, ls) {
            self.overrides.insert(ls, list_id);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ fields

    fn parse_field_group(&mut self, group_start: usize, depth: u32) -> Result<(), RtfError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(RtfError::parse(group_start, "groups are nested too deeply"));
        }
        self.field_instruction.clear();
        let mut instruction_seen = false;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(group_start, "unterminated field")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => {
                    let Some(first) = self.tokenizer.next_token()? else {
                        return Err(RtfError::parse(start, "unterminated field"));
                    };
                    match first {
                        Token::ControlSymbol(b'*') => {
                            let Some(after) = self.tokenizer.next_token()? else {
                                return Err(RtfError::parse(start, "unterminated field"));
                            };
                            match after {
                                Token::ControlWord {
                                    name: "fldinst", ..
                                } => {
                                    self.parse_instruction(depth + 1)?;
                                    instruction_seen = true;
                                }
                                Token::ControlWord {
                                    name: "fldrslt", ..
                                } => {
                                    self.parse_field_result(start, depth + 1)?;
                                }
                                _ => {
                                    self.tokenizer.take_raw_group(start)?;
                                }
                            }
                        }
                        Token::ControlWord {
                            name: "fldinst", ..
                        } => {
                            self.parse_instruction(depth + 1)?;
                            instruction_seen = true;
                        }
                        Token::ControlWord {
                            name: "fldrslt", ..
                        } => {
                            self.parse_field_result(start, depth + 1)?;
                        }
                        _ => {
                            self.tokenizer.take_raw_group(start)?;
                        }
                    }
                }
                Some(_) => {}
            }
        }
        if !instruction_seen {
            self.warnings
                .push("a field without an instruction became its cached text".to_owned());
        }
        Ok(())
    }

    fn parse_instruction(&mut self, _depth: u32) -> Result<(), RtfError> {
        // Bytes still to skip after a `\uN` (its ANSI fallback).
        let mut fallback = 0usize;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated field instruction")),
                Some(Token::GroupEnd) => return Ok(()),
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::Text(bytes)) => {
                    let skip = fallback.min(bytes.len());
                    fallback -= skip;
                    if skip < bytes.len() {
                        self.field_instruction
                            .push_str(&metadata_text(&bytes[skip..], self.decoder.codepage()));
                    }
                }
                Some(Token::HexByte(byte)) => {
                    if fallback > 0 {
                        fallback -= 1;
                    } else {
                        self.field_instruction
                            .push_str(&metadata_text(&[byte], self.decoder.codepage()));
                    }
                }
                Some(Token::ControlWord { name, param }) => {
                    if name == "u" {
                        if let Some(value) = param {
                            let unit = if value < 0 {
                                (value + 65536) as u16
                            } else {
                                value.min(65535) as u16
                            };
                            if let Some(character) = char::from_u32(u32::from(unit)) {
                                self.field_instruction.push(character);
                            }
                            fallback = self.uc as usize;
                        }
                    } else {
                        fallback = 0;
                    }
                }
                // An escaped backslash starts a field switch (`\\h`); other symbols carry
                // no instruction text.
                Some(Token::ControlSymbol(b'\\')) => self.field_instruction.push('\\'),
                Some(Token::ControlSymbol(_)) => {}
            }
        }
    }

    fn parse_field_result(&mut self, group_start: usize, depth: u32) -> Result<(), RtfError> {
        let kind = field_kind(&self.field_instruction);
        match self.sink {
            Sink::Instruction | Sink::Skip => {
                self.tokenizer.take_raw_group(group_start)?;
                Ok(())
            }
            Sink::Header => match kind {
                FieldKind::Page => {
                    self.tokenizer.take_raw_group(group_start)?;
                    self.append_text("{page}");
                    Ok(())
                }
                FieldKind::Pages => {
                    self.tokenizer.take_raw_group(group_start)?;
                    self.append_text("{pages}");
                    Ok(())
                }
                FieldKind::Other | FieldKind::Ref | FieldKind::PageRef => {
                    self.parse_contents(depth)
                }
            },
            // A field inside a note becomes its cached result as plain text; a note has no
            // field node to keep the instruction in.
            Sink::Note => self.parse_contents(depth),
            Sink::Body => match kind {
                FieldKind::Page => {
                    self.warnings.push(
                        "a PAGE field in body text became its cached number as plain text"
                            .to_owned(),
                    );
                    self.parse_contents(depth)
                }
                FieldKind::Pages => {
                    self.warnings.push(
                        "a NUMPAGES field in body text became its cached number as plain text"
                            .to_owned(),
                    );
                    self.parse_contents(depth)
                }
                FieldKind::Ref | FieldKind::PageRef => self.parse_cross_ref_result(kind, depth),
                FieldKind::Other => {
                    let instruction = self.field_instruction.trim().to_owned();
                    Err(RtfError::unsupported(format!(
                        "a body field ({instruction}); only PAGE, NUMPAGES, REF and PAGEREF are supported"
                    )))
                }
            },
        }
    }

    /// The cached result of a body `REF` or `PAGEREF` field. The text becomes ordinary
    /// paragraph text and its byte range becomes a [`CrossRef`] to the named bookmark. A result
    /// that is empty, or that spans a paragraph break, has no single range and is kept as plain
    /// text with a warning.
    fn parse_cross_ref_result(&mut self, kind: FieldKind, depth: u32) -> Result<(), RtfError> {
        let target = cross_ref_target(&self.field_instruction);
        let instruction = self.field_instruction.trim().to_owned();
        self.flush_decoder_text();
        self.ensure_paragraph();
        let (id, start) = match &self.current {
            Some(current) => (current.id, current.text.len()),
            None => return self.parse_contents(depth),
        };
        let saved_char = self.char_state.clone();
        let saved_para = self.para.clone();
        let parsed = self.parse_contents(depth);
        self.char_state = saved_char;
        self.para = saved_para;
        parsed?;
        self.flush_decoder_text();
        let model_kind = if kind == FieldKind::PageRef {
            CrossRefKind::Page
        } else {
            CrossRefKind::Text
        };
        let mut warning = None;
        match (&mut self.current, target) {
            (Some(current), Some(target)) if current.id == id && current.text.len() > start => {
                if target.starts_with('_') {
                    warning = Some(format!(
                        "the cross-reference {instruction} points at a hidden bookmark, which is not imported"
                    ));
                }
                current.cross_refs.push(CrossRef {
                    range: start..current.text.len(),
                    target,
                    kind: model_kind,
                });
            }
            _ => {
                warning = Some(format!(
                    "the field {instruction} has no bookmark name, no result text or a result that spans paragraphs; it became plain text"
                ));
            }
        }
        if let Some(warning) = warning {
            self.warnings.push(warning);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ pictures

    /// Reads a `{\pict ...}` group into an inline image at the current paragraph offset.
    ///
    /// Only `\pngblip` and `\jpegblip` pictures are modelled. Metafiles and bitmaps, cropped
    /// pictures and damaged data are refused with an error, never dropped. The payload is hex
    /// text (decoded in one linear pass, without a per-byte string) or `\binN` raw bytes.
    fn parse_picture(&mut self, group_start: usize, depth: u32) -> Result<(), RtfError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(RtfError::parse(group_start, "groups are nested too deeply"));
        }
        match self.sink {
            Sink::Skip | Sink::Instruction => {
                self.tokenizer.take_raw_group(group_start)?;
                return Ok(());
            }
            Sink::Header => {
                return Err(RtfError::unsupported("a picture in a header or footer"));
            }
            Sink::Note => return Err(RtfError::unsupported("a picture in a note")),
            Sink::Body => {}
        }
        let mut is_blip = false;
        let mut goal_w: Option<i32> = None;
        let mut goal_h: Option<i32> = None;
        let mut scale_x = 100i32;
        let mut scale_y = 100i32;
        let mut cropped = false;
        let mut data: Vec<u8> = Vec::new();
        let mut high: Option<u8> = None;
        let mut alt = String::new();
        let mut name = String::new();
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(group_start, "unterminated picture")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => {
                    let first = self.tokenizer.next_token()?;
                    let second = if first == Some(Token::ControlSymbol(b'*')) {
                        self.tokenizer.next_token()?
                    } else {
                        None
                    };
                    if matches!(
                        second,
                        Some(Token::ControlWord {
                            name: "picprop",
                            ..
                        })
                    ) {
                        self.parse_picture_properties(&mut alt, &mut name)?;
                    } else {
                        // Picture metadata such as `\*\blipuid` is regenerated on save.
                        self.tokenizer.take_raw_group(start)?;
                    }
                }
                Some(Token::ControlWord { name: word, param }) => match word {
                    "pngblip" | "jpegblip" => is_blip = true,
                    "wmetafile" | "emfblip" | "pmmetafile" | "macpict" | "dibitmap" | "wbitmap" => {
                        return Err(RtfError::unsupported(
                            "a metafile or bitmap picture (only PNG and JPEG pictures are supported)",
                        ));
                    }
                    "picwgoal" => goal_w = param,
                    "pichgoal" => goal_h = param,
                    "picscalex" => scale_x = param.filter(|value| *value > 0).unwrap_or(100),
                    "picscaley" => scale_y = param.filter(|value| *value > 0).unwrap_or(100),
                    "piccropl" | "piccropr" | "piccropt" | "piccropb" => {
                        if param.unwrap_or(0) != 0 {
                            cropped = true;
                        }
                    }
                    "bin" => {
                        let length = usize::try_from(param.unwrap_or(0)).unwrap_or(0);
                        data.extend_from_slice(self.tokenizer.take_binary(length)?);
                    }
                    _ => {}
                },
                Some(Token::Text(bytes)) => {
                    if let Err(at) = decode_hex_into(bytes, &mut data, &mut high) {
                        return Err(RtfError::parse(
                            start + at,
                            "a picture holds a character that is not a hex digit",
                        ));
                    }
                }
                Some(Token::HexByte(_)) | Some(Token::ControlSymbol(_)) => {}
            }
        }
        if !is_blip {
            return Err(RtfError::unsupported(
                "a picture that is not a PNG or JPEG (no pngblip or jpegblip)",
            ));
        }
        if cropped {
            return Err(RtfError::unsupported(
                "a cropped picture (the model has no crop)",
            ));
        }
        if high.is_some() {
            return Err(RtfError::parse(
                group_start,
                "a picture holds an odd number of hex digits",
            ));
        }
        let media = Media::new(data).map_err(|error| {
            RtfError::parse(
                group_start,
                format!("the picture data is not usable: {error}"),
            )
        })?;
        let (natural_w, natural_h) = media.natural_size_pt();
        let width_pt = match goal_w {
            Some(goal) if goal > 0 => goal as f32 / 20.0,
            _ => natural_w,
        } * scale_x as f32
            / 100.0;
        let height_pt = match goal_h {
            Some(goal) if goal > 0 => goal as f32 / 20.0,
            _ => natural_h,
        } * scale_y as f32
            / 100.0;
        self.flush_decoder_text();
        self.ensure_paragraph();
        if let Some(current) = &mut self.current {
            let mut image = InlineImage {
                offset: current.text.len(),
                media: Arc::new(media),
                width_pt,
                height_pt,
                alt,
                name,
            };
            image.clamp_size();
            current.images.push(image);
            self.pictures_read += 1;
        }
        Ok(())
    }

    /// `{\*\picprop ...}` inside a picture: shape properties, of which the name and the
    /// description (alt text) are kept. Everything else is ignored.
    fn parse_picture_properties(
        &mut self,
        alt: &mut String,
        name: &mut String,
    ) -> Result<(), RtfError> {
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated picture properties")),
                Some(Token::GroupEnd) => return Ok(()),
                Some(Token::GroupStart) => {
                    let first = self.tokenizer.next_token()?;
                    if matches!(first, Some(Token::ControlWord { name: "sp", .. })) {
                        self.parse_shape_property(alt, name)?;
                    } else {
                        self.tokenizer.take_raw_group(start)?;
                    }
                }
                Some(_) => {}
            }
        }
    }

    /// One `{\sp{\sn key}{\sv value}}` pair (the `\sp` word is already read).
    fn parse_shape_property(
        &mut self,
        alt: &mut String,
        name: &mut String,
    ) -> Result<(), RtfError> {
        let mut key = String::new();
        let mut value = String::new();
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated shape property")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => match self.tokenizer.next_token()? {
                    Some(Token::ControlWord { name: "sn", .. }) => key = self.read_group_text()?,
                    Some(Token::ControlWord { name: "sv", .. }) => {
                        value = self.read_group_text()?;
                    }
                    _ => {
                        self.tokenizer.take_raw_group(start)?;
                    }
                },
                Some(_) => {}
            }
        }
        match key.as_str() {
            "wzDescription" => *alt = value,
            "wzName" => *name = value,
            _ => {}
        }
        Ok(())
    }

    /// The plain text of the group whose start has been read, up to and including its closing
    /// brace. Nested groups are skipped.
    fn read_group_text(&mut self) -> Result<String, RtfError> {
        let mut text = String::new();
        let mut fallback = 0usize;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated group")),
                Some(Token::GroupEnd) => return Ok(text),
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::Text(bytes)) => {
                    let skip = fallback.min(bytes.len());
                    fallback -= skip;
                    if skip < bytes.len() {
                        text.push_str(&metadata_text(&bytes[skip..], self.decoder.codepage()));
                    }
                }
                Some(Token::HexByte(byte)) => {
                    if fallback > 0 {
                        fallback -= 1;
                    } else {
                        text.push_str(&metadata_text(&[byte], self.decoder.codepage()));
                    }
                }
                Some(Token::ControlSymbol(symbol @ (b'\\' | b'{' | b'}'))) => {
                    text.push(char::from(symbol));
                }
                Some(Token::ControlSymbol(_)) => {}
                Some(Token::ControlWord { name: "u", param }) => {
                    if let Some(value) = param {
                        let unit = if value < 0 {
                            (value + 65536) as u16
                        } else {
                            value.min(65535) as u16
                        };
                        if let Some(character) = char::from_u32(u32::from(unit)) {
                            text.push(character);
                        }
                        fallback = self.uc as usize;
                    }
                }
                Some(Token::ControlWord { name: "tab", .. }) => text.push('\t'),
                Some(Token::ControlWord { .. }) => {}
            }
        }
    }

    /// `{\*\libertumtblstyle Name}`: the table style of the table whose first row follows.
    fn parse_table_style_group(&mut self) -> Result<(), RtfError> {
        let name = self.read_group_text()?;
        if self.sink == Sink::Body {
            let name = name.trim();
            if !name.is_empty() {
                self.pending_table_style = Some(name.to_owned());
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ notes

    /// Parses the body of a `\footnote` or `\endnote` destination as plain text and attaches
    /// it to the paragraph being built, anchored where the reference appeared (the current
    /// paragraph text length). Formatting inside the note is flattened; a note in a header or
    /// footer has nowhere to go in the model and is dropped with a warning.
    /// Parses `{\*\bkmkstart Name}`. The group holds the anchor's name; the bookmark is
    /// recorded at the current paragraph offset. `bkmkend` groups are consumed separately.
    fn parse_bookmark_group(&mut self, depth: u32) -> Result<(), RtfError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(RtfError::parse(
                self.tokenizer.position(),
                "groups are nested too deeply",
            ));
        }
        let mut name = String::new();
        let mut fallback = 0usize;
        loop {
            let start = self.tokenizer.position();
            match self.tokenizer.next_token()? {
                None => return Err(RtfError::parse(start, "unterminated bookmark")),
                Some(Token::GroupEnd) => break,
                Some(Token::GroupStart) => {
                    self.tokenizer.take_raw_group(start)?;
                }
                Some(Token::Text(bytes)) => {
                    if fallback > 0 {
                        fallback -= 1;
                    } else {
                        name.push_str(&String::from_utf8_lossy(bytes));
                    }
                }
                Some(Token::HexByte(byte)) => {
                    if fallback > 0 {
                        fallback -= 1;
                    } else {
                        name.push(byte as char);
                    }
                }
                Some(Token::ControlWord { name: "u", param }) => {
                    if let Some(value) = param {
                        let unit = if value < 0 {
                            (value + 65536) as u16
                        } else {
                            value.min(65535) as u16
                        };
                        if let Some(character) = char::from_u32(u32::from(unit)) {
                            name.push(character);
                        }
                        fallback = 1;
                    }
                }
                Some(Token::ControlWord { .. }) | Some(Token::ControlSymbol(_)) => {}
            }
        }
        let name = name.trim();
        if name.is_empty() || name.starts_with('_') {
            return Ok(());
        }
        self.ensure_paragraph();
        if let Some(current) = &mut self.current {
            current.bookmarks.push(Bookmark {
                name: name.to_owned(),
                offset: current.text.len(),
            });
        }
        Ok(())
    }

    fn parse_note_group(&mut self, kind: NoteKind, depth: u32) -> Result<(), RtfError> {
        if depth > MAX_GROUP_DEPTH {
            return Err(RtfError::parse(
                self.tokenizer.position(),
                "groups are nested too deeply",
            ));
        }
        if self.note.is_some() {
            // A note inside a note: flatten its text into the enclosing note.
            return self.parse_contents(depth);
        }
        let saved_sink = self.sink;
        let anchored = saved_sink == Sink::Body;
        let offset = if anchored {
            self.ensure_paragraph();
            self.current
                .as_ref()
                .map_or(0, |current| current.text.len())
        } else {
            0
        };
        self.note = Some(NoteBuilder {
            kind,
            offset,
            text: String::new(),
            styled: false,
        });
        self.sink = Sink::Note;
        let parsed = self.parse_contents(depth);
        self.flush_decoder_text();
        let mut built = self.note.take();
        self.sink = saved_sink;
        parsed?;
        let Some(builder) = built.as_mut() else {
            return Ok(());
        };
        // A destination usually ends with the paragraph break that closes the note's last
        // paragraph; it is structure, not content.
        let text = builder.text.trim_end_matches('\n').to_owned();
        if builder.styled {
            self.warnings
                .push("note formatting was flattened to plain text".to_owned());
        }
        match saved_sink {
            Sink::Body => {
                if let Some(current) = &mut self.current {
                    current.notes.push(Note {
                        kind: builder.kind,
                        offset: builder.offset,
                        text,
                    });
                }
            }
            Sink::Header => {
                self.warnings
                    .push("a note in a header or footer was dropped".to_owned());
            }
            Sink::Instruction | Sink::Note | Sink::Skip => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------------ headers

    fn parse_header_group(
        &mut self,
        kind: HfKind,
        variant: HfVariant,
        right: bool,
        depth: u32,
    ) -> Result<(), RtfError> {
        let saved_sink = self.sink;
        let saved_char = self.char_state.clone();
        let saved_para = self.para.clone();
        let saved_header = self.header.take();
        self.sink = Sink::Header;
        self.char_state = CharFormat::default();
        self.para = ParaState::default();
        self.header = Some(HeaderBuilder::new((kind, variant)));
        let parsed = self.parse_contents(depth);
        let mut builder = self.header.take();
        if let Some(builder) = &mut builder {
            if builder.paragraphs == 0 {
                builder.align = self.para.format.align.unwrap_or(Align::Start);
            }
        }
        self.sink = saved_sink;
        self.char_state = saved_char;
        self.para = saved_para;
        self.header = saved_header;
        parsed?;
        let Some(mut builder) = builder else {
            return Ok(());
        };
        self.section_touched = true;
        builder.text = builder.text.trim_end().to_owned();
        let header = HeaderFooter {
            text: builder.text,
            align: builder.align,
        };
        let set = if kind == HfKind::Header {
            &mut self.section.headers
        } else {
            &mut self.section.footers
        };
        if right {
            if set.default.is_none() {
                set.default = Some(header);
            }
        } else {
            match variant {
                HfVariant::Default => set.default = Some(header),
                HfVariant::First => {
                    set.first = Some(header);
                    set.different_first = true;
                }
                HfVariant::Even => {
                    set.even = Some(header);
                    set.different_even_odd = true;
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ token handling

    fn handle_token(
        &mut self,
        token: Token<'a>,
        token_start: usize,
        depth: u32,
    ) -> Result<(), RtfError> {
        self.shppict_done = false;
        match token {
            Token::GroupStart => self.parse_nested_group(token_start, depth + 1),
            Token::GroupEnd => Ok(()),
            Token::Text(bytes) => {
                if self.sink == Sink::Skip {
                    return Ok(());
                }
                let mut start = 0usize;
                if self.fallback > 0 {
                    let skip = (self.fallback as usize).min(bytes.len());
                    start = skip;
                    self.fallback -= skip as u32;
                }
                if start >= bytes.len() {
                    return Ok(());
                }
                if self.sink == Sink::Instruction {
                    let text = metadata_text(&bytes[start..], self.decoder.codepage());
                    self.field_instruction.push_str(&text);
                    return Ok(());
                }
                self.flush_pending_unit();
                let text = self.decoder.decode(&bytes[start..]);
                if !text.is_empty() {
                    self.append_text(&text);
                }
                Ok(())
            }
            Token::HexByte(byte) => {
                if self.sink == Sink::Skip {
                    return Ok(());
                }
                if self.fallback > 0 {
                    self.fallback -= 1;
                    return Ok(());
                }
                self.flush_pending_unit();
                let text = self.decoder.decode(&[byte]);
                if !text.is_empty() {
                    self.append_text(&text);
                }
                Ok(())
            }
            Token::ControlSymbol(symbol) => {
                if self.sink == Sink::Skip || self.sink == Sink::Instruction {
                    return Ok(());
                }
                match symbol {
                    b'\\' => self.append_text("\\"),
                    b'{' => self.append_text("{"),
                    b'}' => self.append_text("}"),
                    b'~' => self.append_text("\u{00A0}"),
                    b'_' => self.append_text("\u{2011}"),
                    b'-' | b'*' | b'\n' | b'\r' => {}
                    _ => self.preserve_raw(token_start, token_start + 2),
                }
                Ok(())
            }
            Token::ControlWord { name, param } => {
                self.handle_control_word(name, param, token_start, depth)
            }
        }
    }

    fn handle_control_word(
        &mut self,
        name: &str,
        param: Option<i32>,
        token_start: usize,
        depth: u32,
    ) -> Result<(), RtfError> {
        if self.sink == Sink::Skip {
            return Ok(());
        }
        if self.sink == Sink::Instruction {
            if name == "u" {
                if let Some(value) = param {
                    let unit = if value < 0 {
                        (value + 65536) as u16
                    } else {
                        value.min(65535) as u16
                    };
                    if let Some(character) = char::from_u32(u32::from(unit)) {
                        self.field_instruction.push(character);
                    }
                }
            }
            return Ok(());
        }
        if self.sink == Sink::Note {
            return self.handle_note_control(name, param);
        }
        if self.sink == Sink::Header {
            return self.handle_header_control(name, param, token_start);
        }
        if name != "u" {
            self.fallback = 0;
        }
        let _ = depth;
        match name {
            "rtf" | "ansi" | "mac" | "pc" | "pca" | "deflang" | "deflangfe" | "adeflang"
            | "stshfdbch" | "stshfloch" | "stshfhich" | "stshfbi" | "cpg" | "deftab"
            | "widowctrl" | "hyphauto" | "hyphcaps" | "hyphcol" | "hyphnxlmon" | "ftnbj"
            | "aenddoc" | "aendnotes" | "aftnbj" | "aftnsep" | "aftnsepc" | "viewkind"
            | "viewscale" | "viewzk" | "viewbksp" | "fromtext" | "fromhtml" | "fbidis"
            | "atnid" | "atnauthor" | "atndate" | "atnref" | "atntime" | "atrfend"
            | "atrfstart" => return Ok(()),
            "deff" => {
                self.deff = param.unwrap_or(0);
                return Ok(());
            }
            "ansicpg" => {
                let code_page = param.unwrap_or(1252).clamp(0, 65535) as u16;
                if let Some(fallback) = self.decoder.set_codepage(code_page) {
                    self.warnings.push(format!(
                        "code page {code_page} is not known; CP{fallback} was used instead"
                    ));
                }
                return Ok(());
            }
            "uc" => {
                self.uc = param.unwrap_or(1).clamp(0, 32) as u32;
                return Ok(());
            }
            "u" => {
                self.flush_decoder_bytes();
                let value = param.unwrap_or(0);
                let unit = if value < 0 {
                    (value + 65536) as u16
                } else {
                    value.min(65535) as u16
                };
                let text = self.decoder.push_unit(unit);
                if !text.is_empty() {
                    self.append_text(&text);
                }
                self.fallback = self.uc;
                return Ok(());
            }
            "par" => {
                self.flush_decoder_text();
                if self.current.is_none() {
                    self.ensure_paragraph();
                }
                self.finish_paragraph();
                return Ok(());
            }
            "page" => {
                self.flush_decoder_text();
                self.finish_paragraph();
                self.pending_page_break = true;
                return Ok(());
            }
            "line" => {
                self.append_text(" ");
                return Ok(());
            }
            "tab" => {
                self.append_text("\t");
                return Ok(());
            }
            "sect" => {
                self.flush_decoder_text();
                self.finish_paragraph();
                self.finalise_section();
                self.pending_section_start = true;
                return Ok(());
            }
            "sectd" => {
                self.reset_section();
                return Ok(());
            }
            "pard" => {
                self.para = ParaState::default();
                self.sl_twips = None;
                self.sl_mult = 0;
                self.tab_kind = TabStopKind::Left;
                self.list_level = 0;
                if let Some(current) = &mut self.current {
                    current.props.format = ParaFormat::default();
                    current.props.style = StyleId::BODY;
                    current.props.list = None;
                    current.props.outline_level = None;
                }
                return Ok(());
            }
            "plain" => {
                self.char_state = CharFormat::default();
                return Ok(());
            }
            "pagebb" => {
                self.pending_page_break = true;
                if let Some(current) = &mut self.current {
                    current.props.page_break_before = true;
                }
                return Ok(());
            }
            "header" | "headerr" | "headerl" | "headerf" | "footer" | "footerr" | "footerl"
            | "footerf" | "field" => return Ok(()),
            "footnote" | "endnote" => {
                // A note is a destination group; a bare control word has no group to consume,
                // so drop it rather than treating its text as body text.
                self.warnings
                    .push("a note control word outside its group was ignored".to_owned());
                return Ok(());
            }
            "pict" => return Err(RtfError::unsupported("a picture")),
            "shp" | "shpinst" | "object" => {
                return Err(RtfError::unsupported("a drawing object or shape"));
            }
            "annotation" => return Err(RtfError::unsupported("an annotation or comment")),
            "nesttableprops" | "nestrow" => {
                return Err(RtfError::unsupported("a nested table"));
            }
            "itap" => {
                if param.unwrap_or(0) > 1 {
                    return Err(RtfError::unsupported("a nested table"));
                }
                return Ok(());
            }
            "trowd" => {
                self.start_table_row()?;
                return Ok(());
            }
            "cellx" => {
                if let Some(table) = &mut self.table {
                    table.cellx.push(pt_of(param) / 20.0);
                    let definition = std::mem::take(&mut table.merge);
                    table.defs.push(definition);
                }
                return Ok(());
            }
            "trleft" => {
                if let Some(table) = &mut self.table {
                    table.tr_left = pt_of(param) / 20.0;
                }
                return Ok(());
            }
            "trrh" => {
                if let Some(table) = &mut self.table {
                    table.row_min_height_pt = (pt_of(param) / 20.0).max(0.0);
                }
                return Ok(());
            }
            "trkeep" => {
                if let Some(table) = &mut self.table {
                    table.row_keep = true;
                }
                return Ok(());
            }
            "trhdr" => {
                if let Some(table) = &mut self.table {
                    table.row_header = true;
                }
                return Ok(());
            }
            "cell" => {
                self.finish_cell();
                return Ok(());
            }
            "row" => {
                self.finish_row()?;
                return Ok(());
            }
            "clmgf" => {
                if let Some(table) = &mut self.table {
                    table.merge.h_first = true;
                }
                return Ok(());
            }
            "clmrg" => {
                if let Some(table) = &mut self.table {
                    table.merge.h_continue = true;
                }
                return Ok(());
            }
            "clvmgf" => {
                if let Some(table) = &mut self.table {
                    table.merge.v_first = true;
                }
                return Ok(());
            }
            "clvmrg" => {
                if let Some(table) = &mut self.table {
                    table.merge.v_continue = true;
                }
                return Ok(());
            }
            "clcbpat" => {
                let fill = self.colour_of(param);
                if let Some(table) = &mut self.table {
                    table.merge.fill = fill;
                }
                return Ok(());
            }
            "trbrdrt" => return self.border_side(BorderSide::Top),
            "trbrdrb" => return self.border_side(BorderSide::Bottom),
            "trbrdrl" => return self.border_side(BorderSide::Left),
            "trbrdrr" => return self.border_side(BorderSide::Right),
            "trbrdrh" => return self.border_side(BorderSide::InnerH),
            "trbrdrv" => return self.border_side(BorderSide::InnerV),
            "brdrw" => {
                if let Some(table) = &mut self.table {
                    table.borders.width_twips = param;
                }
                return Ok(());
            }
            "brdrcf" => {
                let colour = self.colour_of(param);
                if let Some(table) = &mut self.table {
                    table.borders.colour = colour;
                }
                return Ok(());
            }
            "paperw" => {
                self.paper_w = Some(pt_of(param) / 20.0);
                self.apply_paper();
                return Ok(());
            }
            "paperh" => {
                self.paper_h = Some(pt_of(param) / 20.0);
                self.apply_paper();
                return Ok(());
            }
            "landscape" => {
                self.landscape = Some(true);
                self.apply_paper();
                return Ok(());
            }
            "portrait" => {
                self.landscape = Some(false);
                self.apply_paper();
                return Ok(());
            }
            "margl" => {
                self.section.page.margins.left = pt_of(param) / 20.0;
                return Ok(());
            }
            "margr" => {
                self.section.page.margins.right = pt_of(param) / 20.0;
                return Ok(());
            }
            "margt" => {
                self.section.page.margins.top = pt_of(param) / 20.0;
                return Ok(());
            }
            "margb" => {
                self.section.page.margins.bottom = pt_of(param) / 20.0;
                return Ok(());
            }
            "cols" => {
                self.section
                    .set_columns(param.unwrap_or(1).clamp(1, 255) as u8);
                return Ok(());
            }
            "colsx" => {
                self.section.set_column_gutter(pt_of(param) / 20.0);
                return Ok(());
            }
            "headery" => {
                self.section.header_distance_pt = (pt_of(param) / 20.0).max(0.0);
                return Ok(());
            }
            "footery" => {
                self.section.footer_distance_pt = (pt_of(param) / 20.0).max(0.0);
                return Ok(());
            }
            "titlepg" => {
                // The first-page switch is applied to the header or footer that actually
                // defines a first-page variant; setting both here would lose the model's
                // independent `different_first` flags.
                return Ok(());
            }
            "pgnstart" | "pgnstarts" => {
                self.section.first_page_number = Some(param.unwrap_or(0).max(0) as u32);
                return Ok(());
            }
            "sbknone" | "sbkcol" | "sbkpage" | "sbkeven" | "sbkodd" | "linex" | "endnhere"
            | "pgnrestart" | "pgndec" | "pgnucrm" | "pgnlcrm" | "pgnurom" | "pgnlrom"
            | "pgncont" => return Ok(()),
            "s" => {
                let index = param.unwrap_or(0);
                let style = self.style_ids.get(&index).copied().unwrap_or(StyleId::BODY);
                self.para.style = style;
                if let Some(current) = &mut self.current {
                    current.props.style = style;
                }
                return Ok(());
            }
            "outlinelevel" => {
                let level = param.map(|value| (value.clamp(0, 8) as u8) + 1);
                self.para.outline_level = level;
                if let Some(current) = &mut self.current {
                    current.props.outline_level = level;
                }
                return Ok(());
            }
            "ls" => {
                let ls = param.unwrap_or(0);
                if ls > 0 {
                    let list_id = self.overrides.get(&ls).copied().unwrap_or(ls);
                    let style = self
                        .lists
                        .get(&list_id)
                        .map_or(ListStyle::Numbered, |list| list.style);
                    let group = u64::from(ls as u32) + 1;
                    self.para.list = Some(ListItem {
                        style,
                        level: self.list_level,
                        group,
                    });
                    self.clear_list_indents();
                } else {
                    self.para.list = None;
                }
                if let Some(current) = &mut self.current {
                    current.props.list = self.para.list;
                }
                return Ok(());
            }
            "ilvl" => {
                self.list_level = param.unwrap_or(0).clamp(0, i32::from(MAX_LIST_LEVEL)) as u8;
                if let Some(list) = &mut self.para.list {
                    list.level = self.list_level;
                }
                if let Some(current) = &mut self.current {
                    current.props.list = self.para.list;
                }
                return Ok(());
            }
            "cs" | "deleted" | "revised" | "revauth" | "revdttm" => {
                self.preserve_raw(token_start, self.tokenizer.position());
                return Ok(());
            }
            _ => {}
        }
        let mut char_state = self.char_state.clone();
        if self.apply_char_control(name, param, &mut char_state) {
            self.char_state = char_state;
            return Ok(());
        }
        let mut format = self.para.format;
        if Self::apply_para_control(
            name,
            param,
            &mut format,
            &mut self.sl_twips,
            &mut self.sl_mult,
            &mut self.tab_kind,
        ) {
            self.para.format = format;
            if let Some(current) = &mut self.current {
                current.props.format = format;
                if current.props.list.is_some() {
                    clear_indents(&mut current.props.format);
                }
            }
            return Ok(());
        }
        if is_ignored_control(name) {
            return Ok(());
        }
        self.preserve_raw(token_start, self.tokenizer.position());
        Ok(())
    }

    /// A control word inside `\header`/`\footer`. Furniture is plain text, so character
    /// formatting is tracked only to keep `\u` and alignment right.
    fn handle_header_control(
        &mut self,
        name: &str,
        param: Option<i32>,
        token_start: usize,
    ) -> Result<(), RtfError> {
        if name != "u" {
            self.fallback = 0;
        }
        match name {
            "par" => {
                self.flush_decoder_text();
                if let Some(header) = &mut self.header {
                    if header.paragraphs == 0 {
                        header.align = self.para.format.align.unwrap_or(Align::Start);
                    }
                    if !header.text.is_empty() && !header.text.ends_with(' ') {
                        header.text.push(' ');
                    }
                    header.paragraphs += 1;
                }
                return Ok(());
            }
            "tab" => {
                self.append_text("\t");
                return Ok(());
            }
            "line" => {
                self.append_text(" ");
                return Ok(());
            }
            "u" => {
                self.flush_decoder_bytes();
                let value = param.unwrap_or(0);
                let unit = if value < 0 {
                    (value + 65536) as u16
                } else {
                    value.min(65535) as u16
                };
                let text = self.decoder.push_unit(unit);
                if !text.is_empty() {
                    self.append_text(&text);
                }
                self.fallback = self.uc;
                return Ok(());
            }
            "ansicpg" => {
                let code_page = param.unwrap_or(1252).clamp(0, 65535) as u16;
                if let Some(fallback) = self.decoder.set_codepage(code_page) {
                    self.warnings.push(format!(
                        "code page {code_page} is not known; CP{fallback} was used instead"
                    ));
                }
                return Ok(());
            }
            "uc" => {
                self.uc = param.unwrap_or(1).clamp(0, 32) as u32;
                return Ok(());
            }
            "plain" => {
                self.char_state = CharFormat::default();
                return Ok(());
            }
            "pard" => {
                self.para = ParaState::default();
                self.sl_twips = None;
                self.sl_mult = 0;
                self.tab_kind = TabStopKind::Left;
                return Ok(());
            }
            "field" | "header" | "footer" | "headerf" | "footerf" | "headerl" | "footerl"
            | "headerr" | "footerr" | "sect" | "sectd" => return Ok(()),
            "pict" | "shppict" | "nonshppict" => {
                return Err(RtfError::unsupported("a picture in a header or footer"));
            }
            "footnote" | "endnote" => {
                // Normally a note is a destination group and is dropped there; this covers the
                // malformed bare form inside furniture.
                self.warnings
                    .push("a note in a header or footer was dropped".to_owned());
                return Ok(());
            }
            "shp" | "shpinst" | "object" => {
                return Err(RtfError::unsupported("a drawing object or shape"));
            }
            _ => {}
        }
        let mut char_state = self.char_state.clone();
        if self.apply_char_control(name, param, &mut char_state) {
            self.char_state = char_state;
            return Ok(());
        }
        let mut format = self.para.format;
        if Self::apply_para_control(
            name,
            param,
            &mut format,
            &mut self.sl_twips,
            &mut self.sl_mult,
            &mut self.tab_kind,
        ) {
            self.para.format = format;
            return Ok(());
        }
        if !is_ignored_control(name) {
            self.preserve_raw(token_start, self.tokenizer.position());
        }
        Ok(())
    }

    /// A control word inside a `\footnote`/`\endnote` destination. The model stores a note as
    /// plain text, so formatting controls are noticed (for a warning) and dropped; line and
    /// paragraph breaks become newlines.
    fn handle_note_control(&mut self, name: &str, param: Option<i32>) -> Result<(), RtfError> {
        if name != "u" {
            self.fallback = 0;
        }
        match name {
            "par" | "line" => {
                self.note_break();
                return Ok(());
            }
            // Word writes an endnote as `\footnote\ftnalt`; the RTF specification has no
            // separate `\endnote` destination, but some writers emit one and this reader
            // accepts both.
            "ftnalt" => {
                if let Some(note) = &mut self.note {
                    note.kind = NoteKind::Endnote;
                }
                return Ok(());
            }
            "tab" => {
                self.append_text("\t");
                return Ok(());
            }
            "u" => {
                self.flush_decoder_bytes();
                let value = param.unwrap_or(0);
                let unit = if value < 0 {
                    (value + 65536) as u16
                } else {
                    value.min(65535) as u16
                };
                let text = self.decoder.push_unit(unit);
                if !text.is_empty() {
                    self.append_text(&text);
                }
                self.fallback = self.uc;
                return Ok(());
            }
            "uc" => {
                self.uc = param.unwrap_or(1).clamp(0, 32) as u32;
                return Ok(());
            }
            "ansicpg" => {
                let code_page = param.unwrap_or(1252).clamp(0, 65535) as u16;
                if let Some(fallback) = self.decoder.set_codepage(code_page) {
                    self.warnings.push(format!(
                        "code page {code_page} is not known; CP{fallback} was used instead"
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
        let mut char_state = self.char_state.clone();
        if self.apply_char_control(name, param, &mut char_state) {
            if let Some(note) = &mut self.note {
                note.styled = true;
            }
            return Ok(());
        }
        let mut format = self.para.format;
        let mut sl_twips = None;
        let mut sl_mult = 0;
        let mut tab_kind = self.tab_kind;
        if Self::apply_para_control(
            name,
            param,
            &mut format,
            &mut sl_twips,
            &mut sl_mult,
            &mut tab_kind,
        ) {
            if let Some(note) = &mut self.note {
                note.styled = true;
            }
            return Ok(());
        }
        // Reference marks (`\chftn`), field controls, revision marks and unknown control words
        // carry no note text; they are dropped rather than preserved as raw snippets.
        Ok(())
    }

    fn note_break(&mut self) {
        self.flush_decoder_text();
        if let Some(note) = &mut self.note {
            if !note.text.is_empty() && !note.text.ends_with('\n') {
                note.text.push('\n');
            }
        }
    }

    fn border_side(&mut self, side: BorderSide) -> Result<(), RtfError> {
        if let Some(table) = &mut self.table {
            match side {
                BorderSide::Top => table.borders.top = true,
                BorderSide::Bottom => table.borders.bottom = true,
                BorderSide::Left => table.borders.left = true,
                BorderSide::Right => table.borders.right = true,
                BorderSide::InnerH => table.borders.inner_h = true,
                BorderSide::InnerV => table.borders.inner_v = true,
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ char/para controls

    fn apply_char_control(&self, name: &str, param: Option<i32>, format: &mut CharFormat) -> bool {
        match name {
            "b" => {
                format.weight = Some(if param == Some(0) { 400 } else { 700 });
            }
            "i" => format.italic = Some(param != Some(0)),
            "ul" | "uld" | "uldash" | "uldashd" | "uldashdd" | "uldb" | "ulhwave" | "ulldash"
            | "ulth" | "ulthd" | "ulthdash" | "ulthdashd" | "ulthdashdd" | "ulthldash"
            | "ululdbwave" | "ulw" | "ulwave" => {
                format.underline = Some(param != Some(0));
            }
            "ulnone" => format.underline = Some(false),
            "strike" | "striked" => format.strikethrough = Some(param != Some(0)),
            "super" => format.vertical = Some(libertum_model::VerticalAlign::Superscript),
            "sub" => format.vertical = Some(libertum_model::VerticalAlign::Subscript),
            "nosupersub" => format.vertical = Some(libertum_model::VerticalAlign::Baseline),
            "fs" => {
                if let Some(value) = param {
                    format.size_pt = Some((value as f32 / 2.0).max(0.5));
                }
            }
            "f" => {
                if let Some(index) = param {
                    if let Some(family) = self.fonts.get(&index) {
                        format.family = Some(family.clone().into());
                    }
                }
            }
            "cf" | "cb" | "highlight" => {
                let colour = self.colour_of(param);
                if name == "cf" {
                    format.colour = colour;
                } else {
                    format.highlight = Some(colour);
                }
            }
            "expnd" | "expndtw" => {
                if let Some(value) = param {
                    format.letter_spacing_pt = Some(value as f32 / 20.0);
                }
            }
            "caps" | "scaps" | "outl" | "shad" | "embo" | "impr" | "v" | "up" | "dn"
            | "charscale" | "chshdng" | "chcbpat" | "chcfpat" | "lang" | "langfe" | "langnp"
            | "noproof" | "fcs" | "af" | "loch" | "hich" | "dbch" | "ltrch" | "rtlch" | "fbias"
            | "fcharset" | "cchs" => {}
            _ => return false,
        }
        true
    }

    fn apply_para_control(
        name: &str,
        param: Option<i32>,
        format: &mut ParaFormat,
        sl_twips: &mut Option<i32>,
        sl_mult: &mut i32,
        tab_kind: &mut TabStopKind,
    ) -> bool {
        match name {
            "ql" => format.align = Some(Align::Start),
            "qc" => format.align = Some(Align::Centre),
            "qr" => format.align = Some(Align::End),
            "qj" | "qd" => format.align = Some(Align::Justify),
            "li" | "lin" => format.indent_left_pt = Some(pt_of(param) / 20.0),
            "ri" | "rin" => format.indent_right_pt = Some(pt_of(param) / 20.0),
            "fi" | "fin" => format.indent_first_pt = Some(pt_of(param) / 20.0),
            "sb" => format.space_before_pt = Some(pt_of(param) / 20.0),
            "sa" => format.space_after_pt = Some(pt_of(param) / 20.0),
            "sl" => {
                *sl_twips = param;
                set_line_spacing(format, *sl_twips, *sl_mult);
            }
            "slmult" => {
                *sl_mult = param.unwrap_or(0);
                if let Some(twips) = *sl_twips {
                    set_line_spacing(format, Some(twips), *sl_mult);
                }
            }
            "keep" => format.keep_together = Some(param != Some(0)),
            "keepn" => format.keep_with_next = Some(param != Some(0)),
            "widctlpar" => format.widow_control = Some(true),
            "nowidctlpar" => format.widow_control = Some(false),
            "hyphpar" => format.hyphenate = Some(param != Some(0)),
            "tql" => *tab_kind = TabStopKind::Left,
            "tqc" => *tab_kind = TabStopKind::Centre,
            "tqr" => *tab_kind = TabStopKind::Right,
            "tqd" => *tab_kind = TabStopKind::Decimal,
            "tx" => {
                let mut stops = format.tab_stops.unwrap_or_default();
                let _ = stops.push(TabStop {
                    position_pt: pt_of(param) / 20.0,
                    kind: *tab_kind,
                });
                format.tab_stops = Some(stops);
            }
            "intbl" | "itap" | "aspalpha" | "aspnum" | "faauto" | "adjustright" | "indentfirst"
            | "listtext" | "pn" | "pnlvl" | "pnf" | "pnindent" | "pnstart" | "pnsp" | "pntxta"
            | "pntxtb" | "jclisttab" => {}
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------ paragraph building

    fn append_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        match self.sink {
            Sink::Instruction => {
                self.field_instruction.push_str(text);
                return;
            }
            Sink::Skip => return,
            Sink::Note => {
                if let Some(note) = &mut self.note {
                    note.text.push_str(text);
                }
                return;
            }
            Sink::Header => {
                if let Some(header) = &mut self.header {
                    header.text.push_str(text);
                }
                return;
            }
            Sink::Body => {}
        }
        self.ensure_paragraph();
        if let Some(current) = &mut self.current {
            current.append(text, &self.char_state);
        }
    }

    fn ensure_paragraph(&mut self) {
        if self.current.is_some() {
            return;
        }
        if self.table.as_ref().is_some_and(|table| !table.row_started) {
            self.flush_table();
        }
        let list = self.para.list;
        let mut format = self.para.format;
        if list.is_some() {
            clear_indents(&mut format);
        }
        let props = ParaProps {
            style: self.para.style,
            format,
            page_break_before: self.pending_page_break,
            section_start: self.pending_section_start,
            outline_level: self.para.outline_level,
            toc_target: None,
            list,
        };
        self.pending_page_break = false;
        self.pending_section_start = false;
        let id = self.alloc_id();
        if let Some(table) = &mut self.table {
            if table.row_started {
                table.cell_open = true;
            }
        }
        self.current = Some(PendingParagraph::new(id, props));
    }

    fn finish_paragraph(&mut self) {
        self.flush_decoder_text();
        let Some(mut pending) = self.current.take() else {
            return;
        };
        let id = pending.id;
        let snippets = std::mem::take(&mut pending.snippets);
        let paragraph = pending.into_paragraph();
        if !snippets.is_empty() {
            self.snippets.entry(id).or_default().extend(snippets);
        }
        if let Some(table) = &mut self.table {
            table.cell_paragraphs.push(paragraph);
        } else {
            self.blocks.push(Block::Paragraph(paragraph));
            self.section_touched = true;
        }
    }

    fn clear_list_indents(&mut self) {
        clear_indents(&mut self.para.format);
        if let Some(current) = &mut self.current {
            clear_indents(&mut current.props.format);
            current.props.list = self.para.list;
        }
    }

    // ------------------------------------------------------------------ tables

    fn start_table_row(&mut self) -> Result<(), RtfError> {
        if self.current.is_some() {
            self.finish_paragraph();
        }
        if self.table.as_ref().is_some_and(|table| table.row_started) {
            return Err(RtfError::unsupported("a nested table"));
        }
        let style_name = self.pending_table_style.take();
        if self.table.is_none() {
            let mut builder = TableBuilder::new();
            builder.style_name = style_name;
            self.table = Some(builder);
        }
        if let Some(table) = &mut self.table {
            table.start_row();
        }
        self.para = ParaState::default();
        Ok(())
    }

    fn finish_cell(&mut self) {
        if self.current.is_some() {
            self.finish_paragraph();
        }
        let needs_paragraph = self
            .table
            .as_ref()
            .is_some_and(|table| table.row_started && table.cell_paragraphs.is_empty());
        let empty_id = if needs_paragraph {
            Some(self.alloc_id())
        } else {
            None
        };
        let Some(table) = &mut self.table else {
            return;
        };
        if !table.row_started {
            return;
        }
        let mut paragraphs = std::mem::take(&mut table.cell_paragraphs);
        if paragraphs.is_empty() {
            if let Some(id) = empty_id {
                paragraphs.push(Paragraph::new(id, ""));
            }
        }
        let flags = table.next_cell_flags();
        if flags.h_continue && table.merge_open {
            if let Some(last) = table.row.last_mut() {
                last.col_span = last.col_span.saturating_add(1);
            }
        } else {
            let cell = TableCell {
                col_span: 1,
                merged_into_above: flags.v_continue,
                paragraphs,
                fill: flags.fill,
            };
            table.row.push(cell);
            table.merge_open = flags.h_first;
        }
        table.cell_open = false;
        self.para = ParaState::default();
    }

    fn finish_row(&mut self) -> Result<(), RtfError> {
        let in_progress = self.table.as_ref().is_some_and(|table| table.row_started);
        if !in_progress {
            return Ok(());
        }
        if self.current.is_some() {
            self.finish_paragraph();
        }
        let opens_cell = self
            .table
            .as_ref()
            .is_some_and(|table| table.cell_open || !table.cell_paragraphs.is_empty());
        let empty_id = if opens_cell
            && self
                .table
                .as_ref()
                .is_some_and(|table| table.cell_paragraphs.is_empty())
        {
            Some(self.alloc_id())
        } else {
            None
        };
        let Some(table) = &mut self.table else {
            return Ok(());
        };
        if !table.row_started {
            return Ok(());
        }
        if opens_cell {
            let mut paragraphs = std::mem::take(&mut table.cell_paragraphs);
            if paragraphs.is_empty() {
                if let Some(id) = empty_id {
                    paragraphs.push(Paragraph::new(id, ""));
                }
            }
            let flags = table.next_cell_flags();
            table.row.push(TableCell {
                col_span: 1,
                merged_into_above: flags.v_continue,
                paragraphs,
                fill: flags.fill,
            });
        }
        table.merge = MergeFlags::default();
        table.cell_open = false;
        if !table.row.is_empty() {
            let cells = std::mem::take(&mut table.row);
            let span: usize = cells
                .iter()
                .map(|cell| usize::from(cell.col_span.max(1)))
                .sum();
            if table.cellx.len() > table.widths.len() {
                table.widths = widths_from_cellx(&table.cellx, table.tr_left);
            }
            if table.widths.len() < span {
                let total: f32 = table.widths.iter().sum::<f32>().max(72.0 * span as f32);
                let each = (total / span as f32).max(MIN_TABLE_COLUMN_PT);
                table.widths.resize(span, each);
            }
            let header = table.row_header;
            table.rows.push(TableRow {
                cells,
                min_height_pt: table.row_min_height_pt,
                keep_together: table.row_keep,
            });
            if header && table.still_header {
                table.header_rows = table.header_rows.saturating_add(1);
            } else if !header {
                table.still_header = false;
            }
        }
        table.row_started = false;
        table.cellx.clear();
        table.tr_left = 0.0;
        self.para = ParaState::default();
        Ok(())
    }

    fn flush_table(&mut self) {
        if self.current.is_some() {
            self.finish_paragraph();
        }
        let Some(mut table) = self.table.take() else {
            return;
        };
        if table.rows.is_empty() {
            return;
        }
        let columns = table
            .rows
            .iter()
            .map(TableRow::span)
            .max()
            .unwrap_or(1)
            .max(1);
        if table.widths.len() < columns {
            let total = table.widths.iter().sum::<f32>().max(72.0 * columns as f32);
            let each = (total / columns as f32).max(MIN_TABLE_COLUMN_PT);
            table.widths.resize(columns, each);
        }
        let borders = table_borders(&table.borders);
        let id = self.alloc_id();
        let mut built = Table {
            style: None,
            id,
            widths_pt: table.widths,
            rows: table.rows,
            header_rows: table.header_rows,
            borders,
        };
        built.normalise();
        if let Some(name) = table.style_name.as_deref() {
            self.apply_table_style(&mut built, name);
        }
        self.blocks.push(Block::Table(built));
        self.section_touched = true;
    }

    /// Sets a built-in table style named by `{\*\libertumtblstyle}` and removes the cell
    /// fills the style itself resolves to (the writer put them in each cell for readers that
    /// have no table styles). A cell whose own fill equals the style's comes back as `None`.
    fn apply_table_style(&mut self, table: &mut Table, name: &str) {
        let Some((style, _borders)) = TableStyle::builtin(name) else {
            self.warnings.push(format!(
                "the table style {name} is not a built-in one; its fills stay on the cells"
            ));
            return;
        };
        table.style = Some(style);
        let fills: Vec<Option<[u8; 4]>> = (0..table.rows.len())
            .map(|row| table.style_fill(row))
            .collect();
        for (row, fill) in table.rows.iter_mut().zip(fills) {
            if fill.is_none() {
                continue;
            }
            for cell in &mut row.cells {
                if cell.fill == fill {
                    cell.fill = None;
                }
            }
        }
    }

    // ------------------------------------------------------------------ sections

    fn apply_paper(&mut self) {
        let (Some(width), Some(height)) = (self.paper_w, self.paper_h) else {
            return;
        };
        let landscape = self.landscape.unwrap_or(width > height);
        self.section.page.orientation = if landscape {
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
            self.warnings.push(format!(
                "page size {width:.0}x{height:.0} pt is not a standard paper; using {best:?}"
            ));
        }
        self.section.page.paper = best;
    }

    fn finalise_section(&mut self) {
        self.apply_paper();
        self.sections.push(self.section.clone());
        let mut next = self.section.clone();
        next.headers = HeadersFooters::default();
        next.footers = HeadersFooters::default();
        next.page_numbers = None;
        next.first_page_number = None;
        next.hide_first_page = false;
        self.section = next;
        self.section_touched = false;
    }

    fn reset_section(&mut self) {
        self.section = Section::default();
        self.paper_w = None;
        self.paper_h = None;
        self.landscape = None;
        self.section_touched = true;
    }

    // ------------------------------------------------------------------ preservation

    fn preserve_raw_group(&mut self, group_start: usize) -> Result<(), RtfError> {
        self.shppict_done = false;
        let raw = self.tokenizer.take_raw_group(group_start)?.to_vec();
        self.attach_raw(raw);
        Ok(())
    }

    fn preserve_raw(&mut self, start: usize, end: usize) {
        let input = self.tokenizer.input();
        let start = start.min(end).min(input.len());
        let end = end.min(input.len());
        let raw = input[start..end].to_vec();
        self.attach_raw(raw);
    }

    fn attach_raw(&mut self, raw: Vec<u8>) {
        match self.sink {
            Sink::Instruction | Sink::Skip => {}
            // A note holds plain text only; a raw group inside one has nowhere to be written
            // back, so it is dropped (the note's text is kept).
            Sink::Note => {}
            Sink::Header => {
                if let Some(header) = &self.header {
                    self.header_snippets
                        .entry(header_key(self.sections.len(), header.target))
                        .or_default()
                        .push(raw);
                }
            }
            Sink::Body => {
                if self.current.is_some() || self.table.is_some() {
                    self.ensure_paragraph();
                    if let Some(current) = &mut self.current {
                        current.add_snippet(raw);
                    } else {
                        self.preamble.push(raw);
                    }
                } else {
                    self.preamble.push(raw);
                }
            }
        }
    }

    // ------------------------------------------------------------------ finishing

    fn flush_pending_unit(&mut self) {
        if let Some(character) = self.decoder.take_pending() {
            self.append_text(&character.to_string());
        }
    }

    fn flush_decoder_bytes(&mut self) {
        let text = self.decoder.flush_bytes();
        if !text.is_empty() {
            self.append_text(&text);
        }
    }

    fn flush_decoder_text(&mut self) {
        let text = self.decoder.finish();
        if !text.is_empty() {
            self.append_text(&text);
        }
    }

    fn colour_of(&self, index: Option<i32>) -> Option<[u8; 4]> {
        let index = index?;
        if index <= 0 {
            return None;
        }
        self.colours.get(index as usize).copied()
    }

    fn finish(mut self) -> Result<Parsed, RtfError> {
        self.flush_decoder_text();
        self.finish_paragraph();
        self.flush_table();
        self.apply_paper();
        if self.sections.is_empty() || self.section_touched {
            self.sections.push(self.section.clone());
        }
        if self.sections.is_empty() {
            self.sections.push(Section::default());
        }
        let styles = self.build_styles();
        normalise_blocks(&mut self.blocks, &styles);
        let mut document = Document::from_blocks(self.blocks);
        document.styles = styles;
        document.sections = self.sections;
        Ok(Parsed {
            document,
            preamble: self.preamble,
            snippets: self.snippets,
            header_snippets: self.header_snippets,
            stylesheet_extras: self.stylesheet_extras,
            warnings: self.warnings,
        })
    }

    fn build_styles(&mut self) -> libertum_model::StyleSheet {
        if self.style_ids.is_empty() {
            self.assign_style_ids();
        }
        let normal = self
            .styles
            .iter()
            .find(|style| is_normal_name(&style.name) || style.index == 0)
            .cloned();
        let family = normal
            .as_ref()
            .and_then(|style| style.char.family.as_ref().map(|f| f.to_string()))
            .or_else(|| self.fonts.get(&self.deff).cloned())
            .unwrap_or_else(|| "Cambria".to_owned());
        let size = normal
            .as_ref()
            .and_then(|style| style.char.size_pt)
            .unwrap_or(12.0);
        let mut sheet = libertum_model::StyleSheet::with_defaults(&family, size);
        if let Some(normal) = &normal {
            sheet.defaults = sheet.defaults.apply(&normal.char);
            sheet.para_defaults = sheet.para_defaults.apply(&normal.para);
        }
        let highest = self
            .styles
            .iter()
            .filter_map(|style| self.style_ids.get(&style.index))
            .map(|id| id.0 as usize)
            .max()
            .unwrap_or(5);
        while sheet.paragraph.len() <= highest {
            sheet.paragraph.push(libertum_model::ParagraphStyle {
                name: format!("Style {}", sheet.paragraph.len()),
                base: None,
                char: CharFormat::default(),
                para: ParaFormat::default(),
                next: None,
                outline_level: None,
            });
        }
        for def in &self.styles {
            let Some(&model_id) = self.style_ids.get(&def.index) else {
                continue;
            };
            if model_id == StyleId::BODY && (def.index == 0 || is_normal_name(&def.name)) {
                continue;
            }
            let Some(slot) = sheet.paragraph.get_mut(model_id.0 as usize) else {
                continue;
            };
            if builtin_style(&def.name).is_some() {
                slot.char = def.char.clone();
                slot.para = def.para;
                if let Some(base) = def
                    .based_on
                    .and_then(|index| self.style_ids.get(&index).copied())
                {
                    if base != model_id {
                        slot.base = Some(base);
                    }
                }
                if let Some(next) = def
                    .next
                    .and_then(|index| self.style_ids.get(&index).copied())
                {
                    slot.next = if next == model_id { None } else { Some(next) };
                }
                if def.outline.is_some() {
                    slot.outline_level = def.outline;
                }
            } else {
                let base = def
                    .based_on
                    .and_then(|index| self.style_ids.get(&index).copied());
                let next = def
                    .next
                    .and_then(|index| self.style_ids.get(&index).copied())
                    .filter(|next| *next != model_id);
                *slot = libertum_model::ParagraphStyle {
                    name: if def.name.trim().is_empty() {
                        format!("Style {}", model_id.0)
                    } else {
                        def.name.trim().to_owned()
                    },
                    base,
                    char: def.char.clone(),
                    para: def.para,
                    next,
                    outline_level: def.outline,
                };
            }
        }
        sheet
    }
}

// ---------------------------------------------------------------------- helpers

#[derive(Clone, Copy)]
enum BorderSide {
    Top,
    Bottom,
    Left,
    Right,
    InnerH,
    InnerV,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    Page,
    Pages,
    Ref,
    PageRef,
    Other,
}

fn field_kind(instruction: &str) -> FieldKind {
    let first = instruction
        .trim()
        .split(|character: char| character.is_whitespace())
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .to_ascii_uppercase();
    match first.as_str() {
        "PAGE" => FieldKind::Page,
        "NUMPAGES" => FieldKind::Pages,
        "REF" => FieldKind::Ref,
        "PAGEREF" => FieldKind::PageRef,
        _ => FieldKind::Other,
    }
}

/// The bookmark a `REF` or `PAGEREF` instruction names: the first word after the field type,
/// optionally in double quotes. Switches (`\h`, `\p`) are ignored, and an instruction
/// without a name gives `None`.
fn cross_ref_target(instruction: &str) -> Option<String> {
    let rest = instruction.trim_start();
    let rest = rest[rest.find(char::is_whitespace)?..].trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let name = &quoted[..quoted.find('"')?];
        return (!name.trim().is_empty()).then(|| name.trim().to_owned());
    }
    let name = rest.split_whitespace().next()?;
    (!name.starts_with('\\')).then(|| name.to_owned())
}

/// Hex digit value for each byte, or 0xFF for a byte that is not a hex digit.
const HEX_VALUES: [u8; 256] = {
    let mut table = [0xFFu8; 256];
    let mut index = 0usize;
    while index < 10 {
        table[b'0' as usize + index] = index as u8;
        index += 1;
    }
    index = 0;
    while index < 6 {
        table[b'a' as usize + index] = 10 + index as u8;
        table[b'A' as usize + index] = 10 + index as u8;
        index += 1;
    }
    table
};

/// Appends the bytes spelled by hex digits in `text` to `out`, in one pass. `high` carries a
/// half-read byte from one chunk to the next (the tokeniser splits text at line breaks).
/// White space between digits is allowed; another byte returns its index as the error.
fn decode_hex_into(text: &[u8], out: &mut Vec<u8>, high: &mut Option<u8>) -> Result<(), usize> {
    out.reserve(text.len() / 2);
    for (index, &byte) in text.iter().enumerate() {
        if byte == b' ' || byte == b'\t' {
            continue;
        }
        let value = HEX_VALUES[usize::from(byte)];
        if value == 0xFF {
            return Err(index);
        }
        match high.take() {
            Some(upper) => out.push((upper << 4) | value),
            None => *high = Some(value),
        }
    }
    Ok(())
}

fn header_selector(name: &str) -> (HfKind, HfVariant, bool) {
    match name {
        "header" => (HfKind::Header, HfVariant::Default, false),
        "headerr" => (HfKind::Header, HfVariant::Default, true),
        "headerl" => (HfKind::Header, HfVariant::Even, false),
        "headerf" => (HfKind::Header, HfVariant::First, false),
        "footer" => (HfKind::Footer, HfVariant::Default, false),
        "footerr" => (HfKind::Footer, HfVariant::Default, true),
        "footerl" => (HfKind::Footer, HfVariant::Even, false),
        "footerf" => (HfKind::Footer, HfVariant::First, false),
        _ => (HfKind::Header, HfVariant::Default, false),
    }
}

fn header_key(section: usize, target: (HfKind, HfVariant)) -> HeaderKey {
    (section, target.0, target.1)
}

fn pt_of(param: Option<i32>) -> f32 {
    param.unwrap_or(0) as f32
}

fn widths_from_cellx(cellx: &[f32], tr_left: f32) -> Vec<f32> {
    let mut widths = Vec::with_capacity(cellx.len());
    let mut previous = tr_left;
    for &edge in cellx {
        widths.push((edge - previous).max(MIN_TABLE_COLUMN_PT));
        previous = edge;
    }
    widths
}

fn table_borders(flags: &BorderFlags) -> TableBorders {
    let outer = flags.top || flags.bottom || flags.left || flags.right;
    let inner = flags.inner_h || flags.inner_v;
    if !outer && !inner {
        return TableBorders::none();
    }
    TableBorders {
        width_pt: flags
            .width_twips
            .map_or(0.75, |twips| (twips as f32 / 20.0).max(0.25)),
        colour: flags.colour.unwrap_or([0, 0, 0, 255]),
        outer,
        inner,
    }
}

fn set_line_spacing(format: &mut ParaFormat, twips: Option<i32>, multiple: i32) {
    let Some(twips) = twips else {
        return;
    };
    format.line_spacing = Some(if multiple != 0 {
        LineSpacing::Multiple((twips.abs() as f32 / 240.0).max(0.1))
    } else if twips < 0 {
        LineSpacing::AtLeast((-twips) as f32 / 20.0)
    } else {
        LineSpacing::Exactly(twips as f32 / 20.0)
    });
}

fn clear_indents(format: &mut ParaFormat) {
    format.indent_left_pt = None;
    format.indent_first_pt = None;
}

fn is_normal_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "normal" | "body" | "standard"
    )
}

fn builtin_style(name: &str) -> Option<StyleId> {
    match name.trim().to_ascii_lowercase().as_str() {
        "normal" | "body" | "standard" => Some(StyleId::BODY),
        "title" => Some(StyleId::TITLE),
        "heading 1" | "heading1" => Some(StyleId::HEADING_1),
        "heading 2" | "heading2" => Some(StyleId::HEADING_2),
        "heading 3" | "heading3" => Some(StyleId::HEADING_3),
        "heading 4" | "heading4" => Some(StyleId::HEADING_4),
        "quote" | "quotation" => Some(StyleId::QUOTE),
        _ => None,
    }
}

fn metadata_text(bytes: &[u8], codepage: u16) -> String {
    let (mut decoder, _) = TextDecoder::new(codepage);
    let mut text = decoder.decode(bytes);
    text.push_str(&decoder.finish());
    text
}

fn append_metadata(name: &mut String, bytes: &[u8], codepage: u16, done: &mut bool) {
    if *done {
        return;
    }
    let text = metadata_text(bytes, codepage);
    if let Some(at) = text.find(';') {
        name.push_str(&text[..at]);
        *done = true;
    } else {
        name.push_str(&text);
    }
}

fn is_destination(name: &str) -> bool {
    matches!(
        name,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "listtable"
            | "listoverridetable"
            | "header"
            | "headerr"
            | "headerl"
            | "headerf"
            | "footer"
            | "footerr"
            | "footerl"
            | "footerf"
            | "field"
            | "pict"
            | "shppict"
            | "nonshppict"
            | "shp"
            | "shpinst"
            | "shprslt"
            | "object"
            | "objdata"
            | "footnote"
            | "endnote"
            | "annotation"
            | "atnid"
            | "atnauthor"
            | "atnref"
            | "atndate"
            | "comment"
            | "info"
            | "generator"
            | "company"
            | "doccomm"
            | "operator"
            | "title"
            | "subject"
            | "author"
            | "keywords"
            | "version"
            | "vern"
            | "edmins"
            | "nofpages"
            | "nofwords"
            | "nofchars"
            | "nofcharsws"
            | "themedata"
            | "colorschememapping"
            | "latentstyles"
            | "datastore"
            | "mmathPr"
            | "rsidtbl"
            | "filetbl"
            | "revtbl"
            | "xmlnstbl"
            | "upr"
            | "defchp"
            | "defpap"
            | "bkmkstart"
            | "bkmkend"
            | "fldinst"
            | "fldrslt"
            | "listtext"
            | "pntext"
            | "listname"
            | "leveltext"
            | "levelnumbers"
            | "lfolevel"
            | "listoverride"
            | "list"
            | "listlevel"
            | "pn"
            | "pntxta"
            | "pntxtb"
            | "cs"
            | "ts"
            | "falt"
            | "panose"
            | "fontemb"
            | "fontfile"
            | "file"
            | "template"
            | "private"
            | "libertumtblstyle"
    )
}

fn is_ignored_control(name: &str) -> bool {
    const IGNORED: &[&str] = &[
        "ansicpg",
        "deff",
        "uc",
        "rtf",
        "ansi",
        "mac",
        "pc",
        "pca",
        "deflang",
        "deflangfe",
        "adeflang",
        "deftab",
        "widowctrl",
        "hyphauto",
        "hyphcaps",
        "hyphcol",
        "hyphnxlmon",
        "ftnbj",
        "aenddoc",
        "aendnotes",
        "aftnbj",
        "aftnsep",
        "aftnsepc",
        "viewkind",
        "viewscale",
        "viewzk",
        "viewbksp",
        "fromtext",
        "fromhtml",
        "fbidis",
        "sectd",
        "pard",
        "plain",
        "par",
        "page",
        "line",
        "tab",
        "sect",
        "pgnstart",
        "pgnstarts",
        "titlepg",
        "paperw",
        "paperh",
        "margl",
        "margr",
        "margt",
        "margb",
        "landscape",
        "portrait",
        "cols",
        "colsx",
        "headery",
        "footery",
        "trowd",
        "cellx",
        "cell",
        "row",
        "trhdr",
        "trleft",
        "trrh",
        "trgaph",
        "intbl",
        "itap",
        "clmgf",
        "clmrg",
        "clvmgf",
        "clvmrg",
        "clcbpat",
        "clvertalt",
        "clvertalc",
        "clvertalb",
        "brdrs",
        "brdrth",
        "brdrdb",
        "brdrdot",
        "brdrdash",
        "brdrnil",
        "brdrnone",
        "brdrw",
        "brdrcf",
        "tql",
        "tqc",
        "tqr",
        "tqd",
        "tx",
        "ql",
        "qc",
        "qr",
        "qj",
        "qd",
        "li",
        "ri",
        "fi",
        "lin",
        "rin",
        "fin",
        "sb",
        "sa",
        "sl",
        "slmult",
        "keep",
        "keepn",
        "widctlpar",
        "nowidctlpar",
        "hyphpar",
        "b",
        "i",
        "ul",
        "ulnone",
        "strike",
        "super",
        "sub",
        "nosupersub",
        "fs",
        "f",
        "cf",
        "highlight",
        "cb",
        "expnd",
        "expndtw",
        "s",
        "cs",
        "ls",
        "ilvl",
        "outlinelevel",
        "pagebb",
        "snext",
        "sbasedon",
        "listtext",
        "pntext",
        "sautoupd",
        "qformat",
        "additive",
        "rtlpar",
        "ltrpar",
        "rtlch",
        "ltrch",
        "lang",
        "langfe",
        "langnp",
        "noproof",
        "fcs",
        "af",
        "loch",
        "hich",
        "dbch",
        "fbias",
        "fcharset",
        "cchs",
        "caps",
        "scaps",
        "outl",
        "shad",
        "embo",
        "impr",
        "v",
        "up",
        "dn",
        "chshdng",
        "chcbpat",
        "chcfpat",
        "chdate",
        "chtime",
        "chpgn",
        "chftn",
        "chftnsep",
        "chftnsepc",
        "chatn",
        "chdpl",
        "chdpa",
        "chdpat",
        "sectPr",
        "deleted",
        "revised",
        "revauth",
        "revdttm",
        "sbknone",
        "sbkcol",
        "sbkpage",
        "sbkeven",
        "sbkodd",
        "linex",
        "endnhere",
        "pgnrestart",
        "pgndec",
        "pgnucrm",
        "pgnlcrm",
        "pgnurom",
        "pgnlrom",
        "pgncont",
        "aspalpha",
        "aspnum",
        "faauto",
        "adjustright",
        "indentfirst",
        "jclisttab",
        "pn",
        "pnlvl",
        "pnf",
        "pnindent",
        "pnstart",
        "pnsp",
        "pntxta",
        "pntxtb",
        "listoverridecount",
        "listtemplateid",
        "listsimple",
        "listrestarthdn",
        "levelstartat",
        "levelfollow",
        "leveljc",
        "leveljcn",
        "levelold",
        "levelprev",
        "levelprevspace",
        "levelindent",
        "levelspace",
        "levellegal",
        "levelnorestart",
        "levelnfcn",
        "levelnfc",
        "levelnumbers",
        "leveltext",
        "listid",
        "listoverride",
        "lfolevel",
        "listoverridestart",
        "listoverrideformat",
        "listname",
        "listlevel",
        "list",
        "trbrdrt",
        "trbrdrb",
        "trbrdrl",
        "trbrdrr",
        "trbrdrh",
        "trbrdrv",
        "trpadd",
        "trpaddl",
        "trpaddr",
        "trpaddt",
        "trpaddb",
        "trpaddfl",
        "trpaddfr",
        "trpaddft",
        "trpaddfb",
        "trftsWidth",
        "trwWidth",
        "clftsWidth",
        "clwWidth",
        "clpadl",
        "clpadr",
        "clpadt",
        "clpadb",
        "clpadfl",
        "clpadfr",
        "clpadft",
        "clpadfb",
        "clbrdrt",
        "clbrdrb",
        "clbrdrl",
        "clbrdrr",
        "trqc",
        "trql",
        "trqr",
        "atnid",
        "atnauthor",
        "atndate",
        "atnref",
        "atntime",
        "atrfend",
        "atrfstart",
        "rsid",
        "rsidroot",
        "rsidr",
        "rsidrpr",
        "rsidp",
        "rsidrdefault",
        "rsidtr",
    ];
    if IGNORED.contains(&name) {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower.starts_with("rsid") || lower.starts_with("lang")
}

fn normalise_blocks(blocks: &mut [Block], styles: &libertum_model::StyleSheet) {
    for block in blocks.iter_mut() {
        match block {
            Block::Paragraph(paragraph) => normalise_paragraph_runs(paragraph, styles),
            Block::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        for paragraph in &mut cell.paragraphs {
                            normalise_paragraph_runs(paragraph, styles);
                        }
                    }
                }
            }
        }
    }
}

fn normalise_paragraph_runs(paragraph: &mut Paragraph, styles: &libertum_model::StyleSheet) {
    let style = styles.resolve_char(paragraph.props.style);
    let runs: Vec<Run> = paragraph
        .runs()
        .iter()
        .map(|run| Run {
            range: run.range.clone(),
            format: subtract_style(&run.format, &style),
        })
        .collect();
    paragraph.set_runs(runs);
}

fn subtract_style(format: &CharFormat, style: &libertum_model::ResolvedChar) -> CharFormat {
    CharFormat {
        family: format
            .family
            .clone()
            .filter(|family| **family != *style.family),
        size_pt: format.size_pt.filter(|size| *size != style.size_pt),
        weight: format.weight.filter(|weight| *weight != style.weight),
        italic: format.italic.filter(|value| *value != style.italic),
        underline: format.underline.filter(|value| *value != style.underline),
        strikethrough: format
            .strikethrough
            .filter(|value| *value != style.strikethrough),
        colour: format.colour.filter(|value| *value != style.colour),
        highlight: match format.highlight {
            Some(Some(value)) if style.highlight == Some(value) => None,
            Some(None) if style.highlight.is_none() => None,
            other => other,
        },
        vertical: format.vertical.filter(|value| *value != style.vertical),
        letter_spacing_pt: format
            .letter_spacing_pt
            .filter(|value| *value != style.letter_spacing_pt),
        ligatures: format.ligatures.filter(|value| *value != style.ligatures),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rtf;

    #[test]
    fn paragraphs_and_runs_map() {
        let rtf = Rtf::parse(
            br"{\rtf1\ansi\deff0{\fonttbl{\f0\fcharset0 Georgia;}}\fs24\pard Hello {\b bold} world\par Second\par}",
        )
        .expect("parse");
        let doc = rtf.document();
        assert_eq!(doc.len(), 2);
        let first = doc.paragraph_at(0).expect("paragraph");
        assert_eq!(first.text(), "Hello bold world");
        assert_eq!(first.runs().len(), 1);
        assert_eq!(first.runs()[0].range, 6..10);
        assert_eq!(first.runs()[0].format.weight, Some(700));
        assert_eq!(doc.styles.defaults.family.as_ref(), "Georgia");
        assert_eq!(doc.styles.defaults.size_pt, 12.0);
    }

    #[test]
    fn cp1252_and_unicode_are_decoded() {
        let rtf =
            Rtf::parse(br"{\rtf1\ansi\ansicpg1252 caf\'e9 \u8226? and \u-10179?\u-8704? done\par}")
                .expect("parse");
        let text = rtf.document().paragraph_at(0).expect("paragraph").text();
        assert_eq!(text, "caf\u{e9} \u{2022} and \u{1F600} done");
    }

    #[test]
    fn page_setup_and_sections_map() {
        let rtf = Rtf::parse(
            br"{\rtf1\ansi\deff0\paperw11906\paperh16838\margl1440\margr720\margt1080\margb900\cols2\colsx720\headery720\footery1440 One\par\sect\paperw16838\paperh11906\landscape\margl720\margr720\margt720\margb720 Two\par}",
        )
        .expect("parse");
        let doc = rtf.document();
        assert_eq!(doc.sections.len(), 2);
        assert_eq!(doc.sections[0].page.paper, PaperSize::A4);
        assert_eq!(doc.sections[0].columns, 2);
        assert!((doc.sections[0].column_gutter_pt - 36.0).abs() < 0.01);
        assert!((doc.sections[0].page.margins.left - 72.0).abs() < 0.01);
        assert_eq!(doc.sections[1].page.orientation, Orientation::Landscape);
        assert!(doc.paragraph_at(1).expect("paragraph").props.section_start);
    }

    #[test]
    fn unknown_groups_are_preserved() {
        let rtf = Rtf::parse(br"{\rtf1\ansi {\*\unknownlib keep me} Text\par}").expect("parse");
        let saved = String::from_utf8_lossy(&rtf.to_bytes().expect("write")).into_owned();
        assert!(saved.contains(r"{\*\unknownlib keep me}"), "{saved}");
    }

    #[test]
    fn a_header_page_field_becomes_a_token() {
        let rtf = Rtf::parse(
            br"{\rtf1\ansi {\footer \pard\plain\ql Page {\field{\*\fldinst PAGE}{\fldrslt 1}} of {\field{\*\fldinst NUMPAGES}{\fldrslt 1}}\par}}",
        )
        .expect("parse");
        let footer = rtf.document().sections[0]
            .footers
            .default
            .as_ref()
            .expect("footer");
        assert_eq!(footer.text, "Page {page} of {pages}");
    }

    #[test]
    fn line_tab_and_page_controls_map() {
        let rtf = Rtf::parse(br"{\rtf1\ansi One\line A\tab B\page Two\par}").expect("parse");
        let document = rtf.document();
        assert_eq!(document.len(), 2);
        assert_eq!(
            document.paragraph_at(0).expect("paragraph").text(),
            "One A\tB"
        );
        let second = document.paragraph_at(1).expect("paragraph");
        assert_eq!(second.text(), "Two");
        assert!(second.props.page_break_before);
    }

    #[test]
    fn a_table_reads_its_column_widths() {
        let rtf = Rtf::parse(
            br"{\rtf1\ansi\trowd\cellx2400\cellx6000 \pard\intbl A\cell B\cell\row\pard}",
        )
        .expect("parse");
        let table = rtf
            .document()
            .body()
            .iter()
            .find_map(|block| block.as_table())
            .expect("table");
        assert_eq!(table.widths_pt, vec![120.0, 180.0]);
    }

    #[test]
    fn a_nested_table_is_refused() {
        let error = Rtf::parse(br"{\rtf1\ansi\trowd\cellx1000\trowd\cellx500 x\cell\row}")
            .expect_err("nested table");
        assert!(matches!(error, RtfError::Unsupported { .. }), "{error:?}");
    }
}
