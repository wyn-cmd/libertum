//! HTML to the document model.
//!
//! The tree from [`Dom`] is walked once. Block elements (`p`, `div`, headings, list items,
//! block quotes) close the paragraph being filled and open a new one; inline elements change
//! the character formatting of the text inside them. Whitespace is collapsed as a browser
//! does, except inside `pre`. Images come from `data:` URIs or local files; a remote image is
//! never downloaded.

use std::collections::HashMap;
use std::sync::Arc;

use libertum_model::{
    Align, BOLD, Block, BlockId, CharFormat, Document, InlineImage, ListItem, ListStyle,
    MAX_LIST_LEVEL, Note, NoteKind, Orientation, PageSetup, PaperSize, ParaProps, Paragraph, Run,
    StyleId, StyleSheet, Table, TableBorders, TableCell, TableRow, VerticalAlign,
};

use crate::css;
use crate::dom::{Dom, NodeId, NodeKind};
use crate::style::{self, Baseline};

/// Fetches an image's bytes by the `src` it was written with (relative to the file being
/// read). Never used for remote URLs.
pub type Resolve<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, String>;

/// Largest image read, bytes.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// What reading produced.
pub struct ReadResult {
    pub document: Document,
    pub warnings: Vec<String>,
    pub ao3: bool,
}

/// Reads a parsed tree. `ao3_rules` applies the AO3 download rules when the tree looks like
/// one.
pub fn read(dom: &Dom, resolve: Option<Resolve<'_>>, ao3_rules: bool) -> ReadResult {
    let defaults = document_defaults(dom);
    let baseline = Baseline::new(&defaults.family, defaults.size_pt, defaults.colour);
    let mut reader = Reader::new(dom, resolve, baseline);
    reader.collect_footnotes();

    let ao3 = ao3_rules && crate::ao3::is_download(dom);
    if ao3 {
        crate::ao3::read(&mut reader);
    } else {
        let body = dom.body();
        let ctx = Ctx::default();
        reader.walk_children(body, &ctx);
        reader.flush();
    }
    reader.finish(defaults, ao3)
}

/// What a `<style>` block or the `body` element says about the whole document.
pub struct Defaults {
    pub family: String,
    pub size_pt: f32,
    pub colour: [u8; 4],
    pub page_colour: Option<[u8; 4]>,
    pub page: Option<PageSetup>,
    /// The `lang` (or `xml:lang`) of the `html` element, else of `body`, as a normalised tag.
    pub language: Option<String>,
    pub language2: Option<String>,
}

fn document_defaults(dom: &Dom) -> Defaults {
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut page_decls: Vec<(String, String)> = Vec::new();
    for sheet in dom.find_all(Dom::ROOT, "style") {
        let text = dom.text(sheet);
        decls.extend(css::rules_for(&text, "html"));
        decls.extend(css::rules_for(&text, "body"));
        page_decls.extend(css::rules_for(&text, "@page"));
    }
    let body = dom.body();
    for element in [
        dom.find(Dom::ROOT, &|d, id| d.name(id) == "html"),
        Some(body),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(style) = dom.attr(element, "style") {
            decls.extend(css::declarations(style));
        }
        if let Some(bg) = dom.attr(element, "bgcolor") {
            decls.push(("background-color".into(), bg.to_owned()));
        }
    }
    let base = StyleSheet::default();
    let mut out = Defaults {
        family: base.defaults.family.to_string(),
        size_pt: base.defaults.size_pt,
        colour: base.defaults.colour,
        page_colour: None,
        page: None,
        language2: dom
            .find(Dom::ROOT, &|d, id| {
                d.name(id) == "meta" && d.attr(id, "name") == Some("libertum-language-2")
            })
            .and_then(|id| dom.attr(id, "content"))
            .and_then(libertum_model::language_tag),
        language: [
            dom.find(Dom::ROOT, &|d, id| d.name(id) == "html"),
            Some(body),
        ]
        .into_iter()
        .flatten()
        .find_map(|element| {
            dom.attr(element, "lang")
                .or_else(|| dom.attr(element, "xml:lang"))
                .and_then(libertum_model::language_tag)
        }),
    };
    for (key, value) in &decls {
        match key.as_str() {
            "background" | "background-color" => {
                // White paper is the default: no colour to store.
                out.page_colour = css::background_colour(value)
                    .filter(|c| c[3] > 0)
                    .map(|[r, g, b, _]| [r, g, b, 255])
                    .filter(|c| *c != [255, 255, 255, 255]);
            }
            "font-family" => {
                if let Some(f) = css::font_family(value) {
                    out.family = f;
                }
            }
            "font-size" => {
                if let Some(s) = css::font_size(value, 12.0) {
                    out.size_pt = s;
                }
            }
            "color" => {
                if let Some(c) = css::colour(value) {
                    out.colour = c;
                }
            }
            _ => {}
        }
    }
    if !page_decls.is_empty() {
        let mut page = PageSetup::default();
        let mut any = false;
        for (key, value) in &page_decls {
            match key.as_str() {
                "size" => {
                    if let Some((w, h)) = css::page_size(value) {
                        let (short, long) = (w.min(h), w.max(h));
                        if let Some(paper) = PaperSize::ALL.into_iter().find(|p| {
                            let (pw, ph) = p.points();
                            (pw - short).abs() < 3.0 && (ph - long).abs() < 3.0
                        }) {
                            page.paper = paper;
                            page.orientation = if w > h {
                                Orientation::Landscape
                            } else {
                                Orientation::Portrait
                            };
                            any = true;
                        }
                    }
                }
                "margin" => {
                    if let Some([t, r, b, l]) = css::box_lengths(value, 12.0) {
                        page.margins.top = t;
                        page.margins.right = r;
                        page.margins.bottom = b;
                        page.margins.left = l;
                        any = true;
                    }
                }
                _ => {}
            }
        }
        if any {
            out.page = Some(page);
        }
    }
    out
}

/// The formatting context while walking: what an enclosing element imposes.
#[derive(Clone, Default)]
pub struct Ctx {
    /// Character formatting from enclosing inline (and block) elements.
    pub char: CharFormat,
    /// Paragraph style for new paragraphs (`None` is Body).
    pub style: Option<StyleId>,
    pub outline_level: Option<u8>,
    /// Inherited alignment (`text-align` inherits in CSS).
    pub align: Option<Align>,
    pub quote_depth: u8,
    /// Open lists, innermost last: their style and the top-level list's group.
    pub lists: Vec<ListStyle>,
    pub list_group: u64,
    pub in_list_item: bool,
    pub pre: bool,
}

/// A paragraph being filled.
struct Pending {
    text: String,
    runs: Vec<Run>,
    images: Vec<InlineImage>,
    bookmarks: Vec<libertum_model::Bookmark>,
    /// Ruby: the base's byte range and its reading.
    ruby: Vec<libertum_model::Ruby>,
    /// Footnote references: offset and the id of the note's text.
    note_refs: Vec<(usize, String)>,
    props: ParaProps,
    /// Collapsed whitespace waiting for the next text, so trailing space is dropped.
    space: Option<CharFormat>,
    /// Made by an element that always produces a paragraph (`p`, a heading, `li`), so it is
    /// kept even when empty.
    explicit: bool,
}

pub struct Reader<'a> {
    pub dom: &'a Dom,
    resolve: Option<Resolve<'a>>,
    pub baseline: Baseline,
    blocks: Vec<Block>,
    next_id: u64,
    next_group: u64,
    warnings: Vec<String>,
    pending: Option<Pending>,
    /// Footnote text by element id, gathered before the walk.
    notes: HashMap<String, (NoteKind, String)>,
    /// Elements not to walk (footnote lists, collected separately).
    skip: Vec<NodeId>,
}

impl<'a> Reader<'a> {
    fn new(dom: &'a Dom, resolve: Option<Resolve<'a>>, baseline: Baseline) -> Self {
        Self {
            dom,
            resolve,
            baseline,
            blocks: Vec::new(),
            next_id: 1,
            next_group: 1,
            warnings: Vec::new(),
            pending: None,
            notes: HashMap::new(),
            skip: Vec::new(),
        }
    }

    fn alloc(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn warn(&mut self, message: impl Into<String>) {
        if self.warnings.len() < 100 {
            self.warnings.push(message.into());
        }
    }

    /// Footnote lists (`section.footnotes`, `div.footnotes`, `aside.footnotes`, as written
    /// by Libertum and by Pandoc): each `li` with an id is a note's text.
    fn collect_footnotes(&mut self) {
        let dom = self.dom;
        let mut stack = vec![Dom::ROOT];
        while let Some(at) = stack.pop() {
            if dom.is_element(at) && dom.has_class(at, "footnotes") {
                self.skip.push(at);
                for li in dom.find_all(at, "li") {
                    let Some(id) = dom.attr(li, "id") else {
                        continue;
                    };
                    let kind = if dom.attr(li, "data-kind") == Some("endnote") {
                        NoteKind::Endnote
                    } else {
                        NoteKind::Footnote
                    };
                    let text = note_text(dom, li);
                    self.notes.insert(id.to_owned(), (kind, text));
                }
                continue;
            }
            stack.extend(dom.children(at).iter().rev());
        }
    }

    pub fn walk_children(&mut self, id: NodeId, ctx: &Ctx) {
        for &child in self.dom.children(id) {
            self.walk(child, ctx);
        }
    }

    pub fn walk(&mut self, id: NodeId, ctx: &Ctx) {
        if self.skip.contains(&id) {
            return;
        }
        let dom = self.dom;
        match &dom.nodes[id].kind {
            NodeKind::Text(text) => {
                let text = text.clone();
                self.text(&text, ctx);
            }
            NodeKind::Element { .. } => self.element(id, ctx),
            NodeKind::Document | NodeKind::Other => {}
        }
    }

    fn element(&mut self, id: NodeId, ctx: &Ctx) {
        let dom = self.dom;
        let name = dom.name(id).to_owned();
        if dom.has_class(id, "landmark") {
            // AO3's "Chapter Text" and similar screen-reader headings are page furniture.
            return;
        }
        match name.as_str() {
            "head" | "script" | "style" | "title" | "template" | "noscript" | "meta" | "link"
            | "iframe" | "object" | "embed" | "svg" | "math" | "select" | "button" | "input"
            | "textarea" | "canvas" | "audio" | "video" | "source" | "track" | "map" => {}
            "br" => self.line_break(ctx),
            "img" => self.image(id, ctx),
            "hr" => {
                self.flush();
                let props = ParaProps {
                    style: self.baseline.rule,
                    ..ParaProps::default()
                };
                let paragraph = Paragraph::new(self.alloc(), "").with_props(props);
                self.blocks.push(Block::Paragraph(paragraph));
            }
            "table" => {
                self.flush();
                let table = self.table(id, ctx);
                self.blocks.push(Block::Table(table));
            }
            "ul" | "ol" | "menu" | "dir" => {
                self.flush();
                let mut inner = ctx.clone();
                inner.char = self.block_char_from(id, &ctx.char);
                if inner.lists.is_empty() {
                    inner.list_group = self.next_group;
                    self.next_group += 1;
                }
                inner.lists.push(list_style(dom, id, &name));
                inner.in_list_item = false;
                self.walk_children(id, &inner);
                self.flush();
            }
            "li" => {
                self.flush();
                let mut inner = self.block_ctx(id, ctx);
                if inner.lists.is_empty() {
                    // A stray item outside any list: a bulleted list of its own.
                    inner.lists.push(ListStyle::Bullet);
                    inner.list_group = self.next_group;
                    self.next_group += 1;
                }
                inner.in_list_item = true;
                self.open(id, &inner, true);
                self.walk_children(id, &inner);
                self.flush();
            }
            "blockquote" => {
                self.flush();
                let mut inner = self.block_ctx(id, ctx);
                inner.quote_depth = inner.quote_depth.saturating_add(1);
                self.walk_children(id, &inner);
                self.flush();
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = name.as_bytes()[1] - b'0';
                let mut inner = self.block_ctx(id, ctx);
                inner.style = Some(style::heading_style(level));
                inner.outline_level = Some(level);
                self.open(id, &inner, true);
                self.walk_children(id, &inner);
                self.flush();
            }
            "pre" | "listing" | "xmp" | "plaintext" => {
                self.flush();
                let mut inner = self.block_ctx(id, ctx);
                inner.pre = true;
                if inner.char.family.is_none() {
                    inner.char.family = Some(Arc::from(style::MONOSPACE));
                }
                self.open(id, &inner, true);
                self.walk_children(id, &inner);
                self.flush();
            }
            "p" | "dt" | "dd" | "figcaption" | "caption" | "address" | "summary" => {
                self.flush();
                let inner = self.block_ctx(id, ctx);
                self.open(id, &inner, true);
                self.walk_children(id, &inner);
                self.flush();
            }
            "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside"
            | "figure" | "center" | "dl" | "details" | "fieldset" | "form" | "hgroup" | "body"
            | "html" | "legend" | "tbody" | "thead" | "tfoot" | "tr" | "td" | "th" => {
                self.flush();
                let mut inner = self.block_ctx(id, ctx);
                if name == "center" {
                    inner.align = Some(Align::Centre);
                }
                self.walk_children(id, &inner);
                self.flush();
            }
            _ => self.inline(id, &name, ctx),
        }
    }

    fn inline(&mut self, id: NodeId, name: &str, ctx: &Ctx) {
        let dom = self.dom;
        // A footnote reference: remember where it sits; its number is not text.
        if dom.has_class(id, "footnote-ref") || dom.has_class(id, "footnote-reference") {
            let target = dom
                .find(id, &|d, n| d.name(n) == "a" && d.attr(n, "href").is_some())
                .and_then(|a| dom.attr(a, "href"))
                .and_then(|href| href.strip_prefix('#'))
                .map(str::to_owned);
            if let Some(target) = target
                && self.notes.contains_key(&target)
            {
                self.ensure(ctx);
                self.settle_space();
                if let Some(p) = &mut self.pending {
                    p.note_refs.push((p.text.len(), target));
                }
                return;
            }
        }
        if name == "a" && dom.children(id).is_empty() {
            // An empty named anchor is a bookmark.
            if let Some(anchor) = dom.attr(id, "id").or_else(|| dom.attr(id, "name")) {
                self.ensure(ctx);
                self.settle_space();
                if let Some(p) = &mut self.pending {
                    p.bookmarks.push(libertum_model::Bookmark {
                        name: anchor.to_owned(),
                        offset: p.text.len(),
                    });
                }
                return;
            }
        }
        if name == "ruby" {
            // The base is the element's own text, the reading is in `rt` (`rp` is the
            // fallback parentheses for browsers without ruby).
            let mut inner = ctx.clone();
            inner.char = self.char_from(id, &inner.char);
            self.ensure(ctx);
            self.settle_space();
            let start = self.pending.as_ref().map_or(0, |p| p.text.len());
            let mut reading = String::new();
            for &child in dom.children(id) {
                match dom.name(child) {
                    "rt" => reading.push_str(&dom.text(child)),
                    "rp" | "rtc" => {}
                    _ => self.walk(child, &inner),
                }
            }
            let reading = reading.trim().to_owned();
            if let Some(p) = &mut self.pending
                && p.text.len() > start
                && !reading.is_empty()
            {
                p.ruby
                    .push(libertum_model::Ruby::new(start..p.text.len(), reading));
            }
            return;
        }
        let mut inner = ctx.clone();
        let format = &mut inner.char;
        match name {
            "b" | "strong" => format.weight = Some(BOLD),
            "i" | "em" | "cite" | "dfn" | "var" | "address" => format.italic = Some(true),
            "u" | "ins" => format.underline = Some(true),
            "s" | "strike" | "del" => format.strikethrough = Some(true),
            "sup" => format.vertical = Some(VerticalAlign::Superscript),
            "sub" => format.vertical = Some(VerticalAlign::Subscript),
            "code" | "kbd" | "samp" | "tt" => {
                format.family = Some(Arc::from(style::MONOSPACE));
            }
            "mark" => format.highlight = Some(Some([255, 255, 0, 255])),
            "font" => {
                if let Some(face) = dom.attr(id, "face").and_then(css::font_family) {
                    format.family = Some(Arc::from(face));
                }
                if let Some(c) = dom.attr(id, "color").and_then(css::colour) {
                    format.colour = Some(c);
                }
            }
            _ => {}
        }
        inner.char = self.char_from(id, &inner.char);
        if name == "q" {
            self.text("\u{201C}", &inner);
            self.walk_children(id, &inner);
            self.text("\u{201D}", &inner);
            return;
        }
        self.walk_children(id, &inner);
    }

    /// The context for a block element's content: its own CSS, `align` and the Libertum
    /// style name it may carry.
    fn block_ctx(&mut self, id: NodeId, ctx: &Ctx) -> Ctx {
        let dom = self.dom;
        let mut inner = ctx.clone();
        inner.char = self.block_char_from(id, &ctx.char);
        if let Some(align) = dom.attr(id, "align").and_then(align_value) {
            inner.align = Some(align);
        }
        if let Some(style) = dom.attr(id, "style") {
            for (key, value) in css::declarations(style) {
                match key.as_str() {
                    "text-align" => {
                        if let Some(align) = align_value(&value) {
                            inner.align = Some(align);
                        }
                    }
                    "white-space" => {
                        let v = value.trim().to_ascii_lowercase();
                        inner.pre = v.starts_with("pre") || v == "break-spaces";
                    }
                    _ => {}
                }
            }
        }
        if let Some(name) = dom.attr(id, "data-libertum-style")
            && let Some(style) = self.baseline.style_named(name)
        {
            inner.style = Some(style);
            inner.outline_level = None;
        }
        inner
    }

    /// Character formatting from an element's `style`, over `base`.
    fn char_from(&self, id: NodeId, base: &CharFormat) -> CharFormat {
        self.char_css(id, base, false)
    }

    /// Like [`char_from`](Self::char_from) for a block element, whose background paints its
    /// box (a table cell's shading), not a highlight behind the text.
    fn block_char_from(&self, id: NodeId, base: &CharFormat) -> CharFormat {
        self.char_css(id, base, true)
    }

    fn char_css(&self, id: NodeId, base: &CharFormat, block: bool) -> CharFormat {
        let mut format = base.clone();
        let Some(style) = self.dom.attr(id, "style") else {
            return format;
        };
        let size_now = format
            .size_pt
            .unwrap_or(self.baseline.sheet.defaults.size_pt);
        for (key, value) in css::declarations(style) {
            if block && key.starts_with("background") {
                continue;
            }
            style::apply_char_css(&mut format, &key, &value, size_now);
        }
        format
    }

    /// Opens a paragraph for block element `id` (its paragraph CSS applies).
    fn open(&mut self, id: NodeId, ctx: &Ctx, explicit: bool) {
        let mut props = self.props_for(ctx);
        if let Some(style) = self.dom.attr(id, "style") {
            let size = ctx
                .char
                .size_pt
                .unwrap_or(self.baseline.sheet.defaults.size_pt);
            for (key, value) in css::declarations(style) {
                style::apply_para_css(&mut props, &key, &value, size);
            }
        }
        if let Some(p) = &mut self.pending
            && p.text.is_empty()
            && p.images.is_empty()
            && !p.explicit
        {
            // Nothing written yet: take over the open paragraph.
            p.props = props;
            p.explicit = explicit;
            return;
        }
        self.flush();
        self.pending = Some(Pending {
            text: String::new(),
            runs: Vec::new(),
            images: Vec::new(),
            bookmarks: Vec::new(),
            ruby: Vec::new(),
            note_refs: Vec::new(),
            props,
            space: None,
            explicit,
        });
    }

    fn props_for(&self, ctx: &Ctx) -> ParaProps {
        let mut props = ParaProps::default();
        if let Some(style) = ctx.style {
            props.style = style;
        }
        props.outline_level = ctx.outline_level;
        if ctx.quote_depth > 0 && ctx.style.is_none() {
            props.style = StyleId::QUOTE;
            if ctx.quote_depth > 1 {
                props.format.indent_left_pt = Some(36.0 + f32::from(ctx.quote_depth - 1) * 18.0);
            }
        }
        if ctx.in_list_item
            && let Some(&style) = ctx.lists.last()
        {
            let level = (ctx.lists.len() - 1).min(usize::from(MAX_LIST_LEVEL)) as u8;
            props.list = Some(ListItem {
                style,
                level,
                group: ctx.list_group,
            });
        }
        props.format.align = ctx.align;
        props
    }

    /// Makes sure a paragraph is open for inline content.
    fn ensure(&mut self, ctx: &Ctx) {
        if self.pending.is_none() {
            let props = self.props_for(ctx);
            self.pending = Some(Pending {
                text: String::new(),
                runs: Vec::new(),
                images: Vec::new(),
                bookmarks: Vec::new(),
                ruby: Vec::new(),
                note_refs: Vec::new(),
                props,
                space: None,
                explicit: false,
            });
        }
    }

    /// Writes a pending collapsed space before more content.
    fn settle_space(&mut self) {
        if let Some(p) = &mut self.pending
            && let Some(format) = p.space.take()
        {
            push_run(p, " ", &format);
        }
    }

    pub fn text(&mut self, text: &str, ctx: &Ctx) {
        if ctx.pre {
            if text.is_empty() {
                return;
            }
            self.ensure(ctx);
            self.settle_space();
            let p = self.pending.as_mut().expect("open");
            // A newline straight after <pre> is not content.
            let text = if p.text.is_empty() {
                text.strip_prefix('\n').unwrap_or(text)
            } else {
                text
            };
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            push_run(p, &text, &ctx.char);
            return;
        }
        let mut words = text.split(is_html_space).peekable();
        let starts_with_space = text.starts_with(is_html_space);
        if text.chars().all(is_html_space) {
            // Whitespace only: at most one space, and only inside a paragraph.
            if let Some(p) = &mut self.pending
                && !p.text.is_empty()
                && !p.text.ends_with('\n')
                && p.space.is_none()
            {
                p.space = Some(ctx.char.clone());
            }
            return;
        }
        self.ensure(ctx);
        let p = self.pending.as_mut().expect("open");
        if starts_with_space && !p.text.is_empty() && !p.text.ends_with('\n') {
            p.space.get_or_insert_with(|| ctx.char.clone());
        }
        let mut first = true;
        while let Some(word) = words.next() {
            if word.is_empty() {
                continue;
            }
            if !first {
                p.space.get_or_insert_with(|| ctx.char.clone());
            }
            first = false;
            if let Some(format) = p.space.take()
                && !p.text.is_empty()
                && !p.text.ends_with('\n')
            {
                push_run(p, " ", &format);
            }
            push_run(p, word, &ctx.char);
            let _ = words.peek();
        }
        if text.ends_with(is_html_space) {
            p.space = Some(ctx.char.clone());
        }
    }

    fn line_break(&mut self, ctx: &Ctx) {
        self.ensure(ctx);
        let p = self.pending.as_mut().expect("open");
        p.space = None;
        push_run(p, "\n", &ctx.char);
    }

    fn image(&mut self, id: NodeId, ctx: &Ctx) {
        let dom = self.dom;
        let alt = dom.attr(id, "alt").unwrap_or("").to_owned();
        let src = dom.attr(id, "src").unwrap_or("").trim().to_owned();
        let media = if let Some(data) = src.strip_prefix("data:") {
            decode_data_uri(data).and_then(|bytes| {
                libertum_formats_common::media::media_from_bytes(bytes).map_err(|e| e.to_string())
            })
        } else if is_remote(&src) {
            Err("remote images are not downloaded".to_owned())
        } else if src.is_empty() {
            Err("it has no source".to_owned())
        } else if let Some(resolve) = self.resolve {
            // An EPUB's files are joined into one tree; each remembers its folder.
            let mut base = None;
            let mut at = Some(id);
            while let Some(node) = at {
                if let Some(b) = dom.attr(node, "data-libertum-base") {
                    base = Some(b);
                    break;
                }
                at = dom.nodes[node].parent;
            }
            let src = match base {
                Some(base) => crate::epub::join(base, &crate::util::percent_decode(&src)),
                None => src.clone(),
            };
            resolve(&src).and_then(|bytes| {
                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err("the file is larger than 32 MiB".to_owned());
                }
                libertum_formats_common::media::media_from_bytes(bytes).map_err(|e| e.to_string())
            })
        } else {
            Err("there is no folder to look in".to_owned())
        };
        let media = match media {
            Ok(media) => media,
            Err(why) => {
                let shown = if src.starts_with("data:") {
                    "an embedded image".to_owned()
                } else {
                    format!("the image \"{src}\"")
                };
                self.warn(format!("{shown} was not loaded: {why}"));
                if !alt.is_empty() {
                    self.text(&alt, ctx);
                }
                return;
            }
        };
        self.ensure(ctx);
        self.settle_space();
        let width_limit = self.baseline.content_width_pt;
        let p = self.pending.as_mut().expect("open");
        let mut image = InlineImage::fitted(p.text.len(), media, width_limit);
        let (natural_w, natural_h) = image.media.natural_size_pt();
        let mut width = None;
        let mut height = None;
        if let Some(style) = dom.attr(id, "style") {
            for (key, value) in css::declarations(style) {
                match key.as_str() {
                    "width" => width = css::length(&value, 12.0),
                    "height" => height = css::length(&value, 12.0),
                    _ => {}
                }
            }
        }
        let attr_px = |key: &str| {
            dom.attr(id, key)
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
                .map(|v| v * 0.75)
        };
        width = width.or_else(|| attr_px("width"));
        height = height.or_else(|| attr_px("height"));
        match (width, height) {
            (Some(w), Some(h)) => {
                image.width_pt = w;
                image.height_pt = h;
            }
            (Some(w), None) if natural_w > 0.0 => {
                image.width_pt = w;
                image.height_pt = w * natural_h / natural_w;
            }
            (None, Some(h)) if natural_h > 0.0 => {
                image.height_pt = h;
                image.width_pt = h * natural_w / natural_h;
            }
            _ => {}
        }
        image.clamp_size();
        image.alt = alt;
        image.name = if src.starts_with("data:") {
            String::new()
        } else {
            src
        };
        p.images.push(image);
    }

    /// Closes the open paragraph and adds it to the blocks.
    pub fn flush(&mut self) {
        let Some(mut p) = self.pending.take() else {
            return;
        };
        // A final line break is not shown by a browser (`<p>text<br></p>`).
        if p.text.ends_with('\n') {
            p.text.pop();
            for run in &mut p.runs {
                run.range.end = run.range.end.min(p.text.len());
            }
            p.runs.retain(|r| !r.range.is_empty());
        }
        if !p.explicit
            && p.text.trim().is_empty()
            && p.images.is_empty()
            && p.bookmarks.is_empty()
            && p.note_refs.is_empty()
        {
            return;
        }
        let id = self.alloc();
        let mut paragraph = Paragraph::with_runs(id, p.text, p.runs).with_props(p.props);
        if !p.images.is_empty() {
            paragraph.set_images(p.images);
        }
        if !p.bookmarks.is_empty() {
            paragraph.set_bookmarks(p.bookmarks);
        }
        if !p.ruby.is_empty() {
            paragraph.set_ruby(p.ruby);
        }
        if !p.note_refs.is_empty() {
            let notes = p
                .note_refs
                .into_iter()
                .filter_map(|(offset, target)| {
                    self.notes.get(&target).map(|(kind, text)| Note {
                        kind: *kind,
                        offset,
                        text: text.clone(),
                    })
                })
                .collect();
            paragraph.set_notes(notes);
        }
        self.blocks.push(Block::Paragraph(paragraph));
    }

    /// Adds a paragraph as it is (used by the AO3 rules for their labels).
    pub fn push_paragraph(&mut self, text: &str, props: ParaProps, format: CharFormat) {
        self.flush();
        let id = self.alloc();
        let runs = vec![Run {
            range: 0..text.len(),
            format,
        }];
        self.blocks.push(Block::Paragraph(
            Paragraph::with_runs(id, text, runs).with_props(props),
        ));
    }

    fn table(&mut self, id: NodeId, ctx: &Ctx) -> Table {
        let dom = self.dom;
        // Rows in order, from the table itself and its row groups (not nested tables).
        let mut rows: Vec<(NodeId, bool)> = Vec::new();
        for &child in dom.children(id) {
            match dom.name(child) {
                "tr" => rows.push((child, false)),
                "thead" | "tbody" | "tfoot" => {
                    let head = dom.name(child) == "thead";
                    for &row in dom.children(child) {
                        if dom.name(row) == "tr" {
                            rows.push((row, head));
                        }
                    }
                }
                _ => {}
            }
        }
        let table_border = dom
            .attr(id, "style")
            .map(css::declarations)
            .unwrap_or_default()
            .into_iter()
            .find(|(k, _)| k == "border")
            .and_then(|(_, v)| css::border(&v));
        let border_attr = dom
            .attr(id, "border")
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|v| *v > 0.0);
        let mut inner_border: Option<(f32, [u8; 4])> = None;

        let mut out_rows: Vec<TableRow> = Vec::new();
        let mut header_rows = 0u8;
        let mut counting_header = true;
        // Columns still covered from a row above: (columns spanned, rows still to cover).
        let mut carry: Vec<Option<(u8, u32)>> = Vec::new();
        let saved_blocks = std::mem::take(&mut self.blocks);
        let saved_pending = self.pending.take();
        for (row, in_head) in rows {
            let mut cells: Vec<TableCell> = Vec::new();
            let mut col = 0usize;
            let mut all_th = true;
            let html_cells: Vec<NodeId> = dom
                .children(row)
                .iter()
                .copied()
                .filter(|&c| matches!(dom.name(c), "td" | "th"))
                .collect();
            let mut html_iter = html_cells.into_iter().peekable();
            loop {
                if let Some(Some((span, left))) = carry.get_mut(col).map(|c| c.as_mut()) {
                    let span_now = *span;
                    *left -= 1;
                    if *left == 0 {
                        carry[col] = None;
                    }
                    let mut cell = TableCell::new(Paragraph::new(self.alloc(), ""));
                    cell.col_span = span_now;
                    cell.merged_into_above = true;
                    cells.push(cell);
                    col += usize::from(span_now);
                    continue;
                }
                let Some(cell_node) = html_iter.next() else {
                    // Columns covered from above after the last cell of this row.
                    if carry.iter().skip(col).any(Option::is_some) {
                        col += 1;
                        continue;
                    }
                    break;
                };
                if dom.name(cell_node) != "th" {
                    all_th = false;
                }
                let span = dom
                    .attr(cell_node, "colspan")
                    .and_then(|v| v.trim().parse::<u8>().ok())
                    .unwrap_or(1)
                    .clamp(1, 63);
                let rowspan = dom
                    .attr(cell_node, "rowspan")
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000);
                let mut fill = None;
                if let Some(style) = dom.attr(cell_node, "style") {
                    for (key, value) in css::declarations(style) {
                        match key.as_str() {
                            "background" | "background-color" => {
                                fill = css::background_colour(&value);
                            }
                            "border" if inner_border.is_none() => {
                                inner_border = css::border(&value);
                            }
                            _ => {}
                        }
                    }
                }
                if fill.is_none() {
                    fill = dom.attr(cell_node, "bgcolor").and_then(css::colour);
                }
                let mut cell_ctx = Ctx {
                    char: ctx.char.clone(),
                    ..Ctx::default()
                };
                if dom.name(cell_node) == "th" {
                    cell_ctx.char.weight.get_or_insert(BOLD);
                }
                let cell_ctx = self.block_ctx(cell_node, &cell_ctx);
                self.walk_children(cell_node, &cell_ctx);
                self.flush();
                let mut paragraphs: Vec<Paragraph> = Vec::new();
                for block in std::mem::take(&mut self.blocks) {
                    match block {
                        Block::Paragraph(p) => paragraphs.push(p),
                        // A table inside a cell: its text, paragraph by paragraph.
                        Block::Table(t) => {
                            for row in t.rows {
                                for c in row.cells {
                                    paragraphs.extend(c.paragraphs);
                                }
                            }
                        }
                    }
                }
                if paragraphs.is_empty() {
                    paragraphs.push(Paragraph::new(self.alloc(), ""));
                }
                cells.push(TableCell {
                    col_span: span,
                    merged_into_above: false,
                    paragraphs,
                    fill,
                });
                if rowspan > 1 {
                    if carry.len() <= col {
                        carry.resize(col + 1, None);
                    }
                    carry[col] = Some((span, rowspan - 1));
                }
                col += usize::from(span);
            }
            if counting_header && (in_head || (all_th && !cells.is_empty())) {
                header_rows = header_rows.saturating_add(1);
            } else {
                counting_header = false;
            }
            out_rows.push(TableRow {
                cells,
                min_height_pt: 0.0,
                keep_together: true,
            });
        }
        self.blocks = saved_blocks;
        self.pending = saved_pending;

        let span_of = |row: &TableRow| row.cells.iter().map(|c| usize::from(c.col_span)).sum();
        let mut columns: usize = out_rows.iter().map(span_of).max().unwrap_or(0).max(1);
        if out_rows.is_empty() {
            out_rows.push(TableRow {
                cells: vec![TableCell::new(Paragraph::new(self.alloc(), ""))],
                min_height_pt: 0.0,
                keep_together: true,
            });
            columns = 1;
        }
        for row in &mut out_rows {
            let mut have: usize = span_of(row);
            while have < columns {
                row.cells
                    .push(TableCell::new(Paragraph::new(self.alloc(), "")));
                have += 1;
            }
        }
        // Column widths from <col> elements, when every column has one.
        let mut widths: Vec<f32> = Vec::new();
        for col in dom.find_all(id, "col") {
            let span = dom
                .attr(col, "span")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 63);
            let width = dom
                .attr(col, "style")
                .map(css::declarations)
                .unwrap_or_default()
                .into_iter()
                .find(|(k, _)| k == "width")
                .and_then(|(_, v)| css::length(&v, 12.0))
                .or_else(|| {
                    dom.attr(col, "width")
                        .and_then(|v| v.trim_end_matches("px").parse::<f32>().ok())
                        .map(|v| v * 0.75)
                });
            for _ in 0..span {
                widths.push(width.unwrap_or(0.0));
            }
        }
        if widths.len() != columns || widths.iter().any(|w| *w <= 0.0) {
            let each = (self.baseline.content_width_pt / columns as f32)
                .max(libertum_model::MIN_TABLE_COLUMN_PT);
            widths = vec![each; columns];
        }
        let borders = match (table_border, inner_border, border_attr) {
            (None, None, None) => TableBorders::none(),
            (outer, inner, attr) => {
                let line = outer
                    .or(inner)
                    .unwrap_or((attr.unwrap_or(0.75), [0, 0, 0, 255]));
                TableBorders {
                    width_pt: line.0,
                    colour: line.1,
                    outer: outer.is_some() || attr.is_some(),
                    inner: inner.is_some() || attr.is_some(),
                }
            }
        };
        let mut table = Table {
            id: self.alloc(),
            widths_pt: widths,
            rows: out_rows,
            header_rows,
            borders,
            style: None,
        };
        table.normalise();
        table
    }

    fn finish(mut self, defaults: Defaults, ao3: bool) -> ReadResult {
        self.flush();
        if self.blocks.is_empty() {
            let id = self.alloc();
            self.blocks.push(Block::Paragraph(Paragraph::new(id, "")));
        }
        let mut document = Document::from_blocks(self.blocks);
        document.styles = self.baseline.sheet;
        if let Some(section) = document.sections.first_mut() {
            if let Some(page) = defaults.page {
                section.page = page;
            }
            section.page_colour = defaults.page_colour;
            section.language = defaults.language;
            section.language2 = defaults.language2;
        }
        ReadResult {
            document,
            warnings: self.warnings,
            ao3,
        }
    }
}

fn push_run(p: &mut Pending, text: &str, format: &CharFormat) {
    let start = p.text.len();
    p.text.push_str(text);
    if !format.is_empty() {
        p.runs.push(Run {
            range: start..p.text.len(),
            format: format.clone(),
        });
    }
}

fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

fn align_value(value: &str) -> Option<Align> {
    match value.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(Align::Start),
        "center" | "centre" => Some(Align::Centre),
        "right" | "end" => Some(Align::End),
        "justify" => Some(Align::Justify),
        _ => None,
    }
}

fn list_style(dom: &Dom, id: NodeId, name: &str) -> ListStyle {
    if name != "ol" {
        return ListStyle::Bullet;
    }
    match dom.attr(id, "type").map(str::trim) {
        Some("A" | "a") => ListStyle::Lettered,
        Some("I" | "i") => ListStyle::Harvard,
        _ => {
            match dom
                .attr(id, "style")
                .map(css::declarations)
                .unwrap_or_default()
                .into_iter()
                .find(|(k, _)| k == "list-style-type" || k == "list-style")
                .map(|(_, v)| v.to_ascii_lowercase())
                .as_deref()
            {
                Some(v) if v.contains("alpha") || v.contains("latin") => ListStyle::Lettered,
                Some(v) if v.contains("roman") => ListStyle::Harvard,
                _ => ListStyle::Numbered,
            }
        }
    }
}

/// A footnote's text: its paragraphs joined by line breaks, without the back-link.
fn note_text(dom: &Dom, li: NodeId) -> String {
    let mut out = String::new();
    let mut stack = vec![li];
    while let Some(at) = stack.pop() {
        if dom.is_element(at)
            && (dom.has_class(at, "footnote-back")
                || dom.has_class(at, "footnote-backref")
                || dom.attr(at, "role") == Some("doc-backlink"))
        {
            continue;
        }
        match &dom.nodes[at].kind {
            NodeKind::Text(text) => out.push_str(text),
            NodeKind::Element { .. } if matches!(dom.name(at), "p" | "br") && !out.is_empty() => {
                out.push('\n');
            }
            _ => {}
        }
        stack.extend(dom.children(at).iter().rev());
    }
    out.split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_remote(src: &str) -> bool {
    if src.starts_with("//") {
        return true;
    }
    let Some(colon) = src.find(':') else {
        return false;
    };
    let scheme = &src[..colon];
    if scheme.eq_ignore_ascii_case("file") {
        return false;
    }
    // A Windows drive letter (C:\) is a local path.
    scheme.len() >= 2
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// The bytes of a `data:` URI (the part after `data:`).
fn decode_data_uri(data: &str) -> Result<Vec<u8>, String> {
    let (meta, payload) = data
        .split_once(',')
        .ok_or_else(|| "the embedded data is malformed".to_owned())?;
    if meta.to_ascii_lowercase().ends_with(";base64") {
        crate::base64::decode(payload).ok_or_else(|| "the embedded data is not base64".to_owned())
    } else {
        Ok(crate::util::percent_decode_bytes(payload))
    }
}
