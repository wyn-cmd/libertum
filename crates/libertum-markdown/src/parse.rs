//! Parsing Markdown into the model, and recording the source of every top-level block.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use comrak::nodes::{AstNode, ListDelimType, ListType, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};
use libertum_formats_common::media::media_from_bytes;
use libertum_model::{
    Align, Block, BlockId, Borders, CharFormat, Document, InlineImage, ListItem, ListStyle,
    MAX_LIST_LEVEL, Media, ParaFormat, ParaProps, Paragraph, ParagraphStyle, Run, StyleId,
    StyleSheet, Table, TableBorders, TableCell, TableRow,
};

use crate::MarkdownError;
use crate::write::fingerprint_document;

/// Display width an image opened from Markdown is fitted to, points (about 5.8 inches).
pub(crate) const IMAGE_READING_WIDTH_PT: f32 = 420.0;

/// Largest image file read from disk, bytes. A larger file is left as text with a warning.
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// One hyperlink in a paragraph's text, kept so an edited paragraph can write it back.
#[derive(Clone, Debug)]
pub(crate) struct LinkSpan {
    pub range: Range<usize>,
    pub url: String,
    pub title: String,
}

/// The links of a paragraph as they were when the file was opened.
#[derive(Clone, Debug)]
pub(crate) struct ParaLinks {
    /// The paragraph text at open time; the spans index into it.
    pub text: String,
    pub spans: Vec<LinkSpan>,
}

/// A fenced code block as it was written, for re-serialising an edited one.
#[derive(Clone, Debug)]
pub(crate) struct CodeMeta {
    pub fence_char: char,
    pub fence_len: usize,
    pub info: String,
    /// The block's lines, exactly as they were.
    pub literal: String,
    /// The paragraph text the block was shown as (lines joined by spaces, trimmed).
    pub shown: String,
}

/// Per-block facts that the model cannot hold but an edited block needs to be written back
/// faithfully. Keyed by paragraph id or list group.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hints {
    /// First number of each ordered list, by `(list group, level)`.
    pub list_starts: HashMap<(u64, u8), usize>,
    pub links: HashMap<BlockId, ParaLinks>,
    pub code: HashMap<BlockId, CodeMeta>,
}

/// The source of one top-level Markdown block, and the model blocks it produced.
#[derive(Clone, Debug)]
pub(crate) struct GroupSource {
    pub ids: Vec<BlockId>,
    /// The original source, including the blank lines after the block.
    pub source: String,
    pub fingerprint: u64,
}

/// What the file looked like when it was opened.
#[derive(Clone, Debug)]
pub(crate) struct Origin {
    /// Bytes before the first top-level block (byte-order mark, front matter).
    pub prefix: String,
    pub groups: Vec<GroupSource>,
    pub conventions: Conventions,
    pub hints: Hints,
    pub document_fingerprint: u64,
}

/// Writing conventions taken from the file, used when an edited block must be regenerated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conventions {
    pub emphasis: char,
    pub bullet: char,
    pub fence: char,
    pub ordered_delim: char,
    pub setext: bool,
}

impl Default for Conventions {
    fn default() -> Self {
        Self {
            emphasis: '*',
            bullet: '-',
            fence: '`',
            ordered_delim: '.',
            setext: false,
        }
    }
}

pub(crate) fn parse(
    bytes: &[u8],
    base_dir: Option<&Path>,
) -> Result<(Document, Origin, Vec<String>), MarkdownError> {
    let (text, bom) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (rest, "\u{feff}"),
        None => (bytes, ""),
    };
    let source =
        std::str::from_utf8(text).map_err(|error| MarkdownError::Encoding(error.to_string()))?;

    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    options.extension.front_matter_delimiter = Some("---".to_owned());

    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);

    let mut builder = Builder::new(base_dir.map(Path::to_path_buf));
    let mut groups = Vec::new();
    let top: Vec<&AstNode> = root.children().collect();
    let starts: Vec<usize> = top
        .iter()
        .map(|node| line_offset(source, node.data().sourcepos.start.line))
        .collect();

    for (index, node) in top.iter().enumerate() {
        let before = builder.blocks.len();
        let blocks = builder.build_block(node, 0);
        for block in blocks {
            builder.blocks.push(block);
        }
        if builder.blocks.len() == before {
            // Every top-level node keeps one model block, so source slices stay aligned.
            let paragraph = builder.raw_paragraph("");
            builder.blocks.push(Block::Paragraph(paragraph));
        }
        let end = starts.get(index + 1).copied().unwrap_or(source.len());
        let start = starts[index].min(end);
        let ids = builder.blocks[before..]
            .iter()
            .map(|block| block.id())
            .collect();
        let fingerprint = crate::write::fingerprint_blocks(&builder.blocks[before..]);
        groups.push(GroupSource {
            ids,
            source: source[start..end].to_owned(),
            fingerprint,
        });
    }

    let prefix = match starts.first() {
        Some(&start) if start <= source.len() => format!("{bom}{}", &source[..start]),
        _ => format!("{bom}{source}"),
    };
    let conventions = builder.detect_conventions(source);

    let mut document = Document::from_blocks(builder.blocks);
    document.styles = builder.styles;
    let origin = Origin {
        prefix,
        groups,
        conventions,
        hints: builder.hints,
        document_fingerprint: fingerprint_document(&document),
    };
    Ok((document, origin, builder.warnings))
}

fn line_offset(source: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut current = 1usize;
    for (index, _) in source.match_indices('\n') {
        current += 1;
        if current == line {
            return index + 1;
        }
    }
    source.len()
}

struct Builder {
    blocks: Vec<Block>,
    styles: StyleSheet,
    next_id: u64,
    next_group: u64,
    warnings: Vec<String>,
    base_dir: Option<std::path::PathBuf>,
    hints: Hints,
    first_setext: Option<bool>,
    first_bullet: Option<char>,
    first_delim: Option<char>,
    first_fence: Option<char>,
    code_style: StyleId,
    raw_style: StyleId,
    rule_style: StyleId,
}

impl Builder {
    fn new(base_dir: Option<std::path::PathBuf>) -> Self {
        let mut styles = StyleSheet::default();
        let code_style = add_style(
            &mut styles,
            "Markdown Code",
            CharFormat {
                family: Some(Arc::from("Courier New")),
                size_pt: Some(10.0),
                ..CharFormat::default()
            },
            ParaFormat {
                space_before_pt: Some(0.0),
                space_after_pt: Some(6.0),
                ..ParaFormat::default()
            },
        );
        let raw_style = add_style(
            &mut styles,
            "Markdown Raw",
            CharFormat {
                colour: Some([90, 90, 90, 255]),
                ..CharFormat::default()
            },
            ParaFormat {
                space_after_pt: Some(6.0),
                ..ParaFormat::default()
            },
        );
        let rule_style = add_style(
            &mut styles,
            "Markdown Rule",
            CharFormat::default(),
            ParaFormat {
                space_before_pt: Some(2.0),
                space_after_pt: Some(10.0),
                borders: Some(Borders {
                    sides: libertum_model::BorderSides {
                        bottom: true,
                        ..libertum_model::BorderSides::NONE
                    },
                    width_pt: 0.75,
                    colour: [150, 150, 150, 255],
                    space_pt: 1.0,
                }),
                ..ParaFormat::default()
            },
        );
        Self {
            blocks: Vec::new(),
            styles,
            next_id: 1,
            next_group: 1,
            warnings: Vec::new(),
            base_dir,
            hints: Hints::default(),
            first_setext: None,
            first_bullet: None,
            first_delim: None,
            first_fence: None,
            code_style,
            raw_style,
            rule_style,
        }
    }

    fn alloc(&mut self) -> BlockId {
        let id = BlockId(self.next_id);
        self.next_id += 1;
        id
    }

    fn warn(&mut self, message: impl Into<String>) {
        if self.warnings.len() < 100 {
            self.warnings.push(message.into());
        }
    }

    fn detect_conventions(&self, source: &str) -> Conventions {
        let emphasis = detect_emphasis(source);
        Conventions {
            emphasis,
            bullet: self.first_bullet.unwrap_or('-'),
            fence: self.first_fence.unwrap_or('`'),
            ordered_delim: self.first_delim.unwrap_or('.'),
            setext: self.first_setext.unwrap_or(false),
        }
    }

    fn paragraph(&mut self, text: String, runs: Vec<Run>, mut props: ParaProps) -> Paragraph {
        if props.format.space_after_pt.is_none() {
            props.format.space_after_pt = Some(6.0);
        }
        Paragraph::with_runs(self.alloc(), text, runs).with_props(props)
    }

    fn raw_paragraph(&mut self, text: &str) -> Paragraph {
        let props = ParaProps {
            style: self.raw_style,
            ..ParaProps::default()
        };
        self.paragraph(text.to_owned(), Vec::new(), props)
    }

    fn build_block<'a>(&mut self, node: &'a AstNode<'a>, quote_depth: u8) -> Vec<Block> {
        let value = node.data().value.clone();
        match value {
            NodeValue::Paragraph => vec![Block::Paragraph(self.inline_paragraph(
                node,
                self.body_props(quote_depth),
                None,
            ))],
            NodeValue::Heading(heading) => {
                if self.first_setext.is_none() {
                    self.first_setext = Some(heading.setext);
                }
                let level = heading.level.clamp(1, 9);
                let style = match level {
                    1 => StyleId::HEADING_1,
                    2 => StyleId::HEADING_2,
                    _ => StyleId::HEADING_3,
                };
                let props = ParaProps {
                    style,
                    outline_level: Some(level),
                    ..ParaProps::default()
                };
                vec![Block::Paragraph(self.inline_paragraph(node, props, None))]
            }
            NodeValue::ThematicBreak => {
                let props = ParaProps {
                    style: self.rule_style,
                    ..ParaProps::default()
                };
                vec![Block::Paragraph(self.paragraph(
                    String::new(),
                    Vec::new(),
                    props,
                ))]
            }
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                let mut out = Vec::new();
                for child in node.children() {
                    out.extend(self.build_block(child, quote_depth.saturating_add(1)));
                }
                out
            }
            NodeValue::List(list) => self.build_list(node, &list, 0, quote_depth, None),
            NodeValue::Item(_) | NodeValue::TaskItem(_) => {
                self.build_list(node, &empty_list(), 0, quote_depth, None)
            }
            NodeValue::Table(_) => vec![Block::Table(self.build_table(node))],
            NodeValue::CodeBlock(code) => {
                if self.first_fence.is_none() {
                    self.first_fence = Some(code.fence_char as char);
                }
                self.warn("a code block is shown as a single paragraph; its original fences are kept until it is edited");
                let text = code.literal.replace(['\n', '\r'], " ").trim().to_owned();
                let props = ParaProps {
                    style: self.code_style,
                    ..ParaProps::default()
                };
                let paragraph = self.paragraph(text.clone(), Vec::new(), props);
                let fence_char = if code.fenced {
                    code.fence_char as char
                } else {
                    self.first_fence.unwrap_or('`')
                };
                self.hints.code.insert(
                    paragraph.id,
                    CodeMeta {
                        fence_char,
                        fence_len: if code.fenced {
                            code.fence_length.max(3)
                        } else {
                            3
                        },
                        info: code.info.clone(),
                        literal: code.literal.clone(),
                        shown: text,
                    },
                );
                vec![Block::Paragraph(paragraph)]
            }
            NodeValue::HtmlBlock(html) => {
                let text = html.literal.replace(['\n', '\r'], " ").trim().to_owned();
                let props = ParaProps {
                    style: self.raw_style,
                    ..ParaProps::default()
                };
                vec![Block::Paragraph(self.paragraph(text, Vec::new(), props))]
            }
            NodeValue::FrontMatter(front) => {
                let text = front.replace(['\n', '\r'], " ").trim().to_owned();
                let props = ParaProps {
                    style: self.raw_style,
                    ..ParaProps::default()
                };
                vec![Block::Paragraph(self.paragraph(text, Vec::new(), props))]
            }
            NodeValue::FootnoteDefinition(footnote) => {
                let mut out = Vec::new();
                let mut prefix = Some(format!("[^{}]: ", footnote.name));
                for child in node.children() {
                    for mut block in self.build_block(child, quote_depth) {
                        if let (Some(prefix), Block::Paragraph(paragraph)) =
                            (prefix.take(), &mut block)
                        {
                            paragraph.insert_text(0, &prefix);
                            if let Some(links) = self.hints.links.get_mut(&paragraph.id) {
                                links.text = paragraph.text().to_owned();
                                for span in &mut links.spans {
                                    span.range = span.range.start + prefix.len()
                                        ..span.range.end + prefix.len();
                                }
                            }
                        }
                        out.push(block);
                    }
                }
                if out.is_empty() {
                    out.push(Block::Paragraph(
                        self.raw_paragraph(&format!("[^{}]:", footnote.name)),
                    ));
                }
                out
            }
            _ => {
                self.warn("an unsupported Markdown block is kept as raw text");
                vec![Block::Paragraph(self.raw_paragraph(""))]
            }
        }
    }

    fn body_props(&self, quote_depth: u8) -> ParaProps {
        if quote_depth == 0 {
            return ParaProps::default();
        }
        let indent = 36.0 + f32::from(quote_depth.saturating_sub(1)) * 18.0;
        ParaProps {
            style: StyleId::QUOTE,
            format: ParaFormat {
                indent_left_pt: Some(indent),
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        }
    }

    fn inline_paragraph<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        props: ParaProps,
        prefix: Option<&str>,
    ) -> Paragraph {
        let mut text = TextBuf::default();
        if let Some(prefix) = prefix {
            text.push(prefix, &CharFormat::default());
        }
        self.read_inline(node, &mut text);
        self.finish(text, props)
    }

    fn read_inline<'a>(&mut self, node: &'a AstNode<'a>, out: &mut TextBuf) {
        let mut cx = InlineCx {
            warnings: &mut self.warnings,
            base_dir: self.base_dir.as_deref(),
        };
        inline_text(node, &CharFormat::default(), out, &mut cx);
    }

    /// Builds the paragraph for read inline content: text, runs, images, and the links
    /// remembered for writing an edited paragraph back.
    fn finish(&mut self, buffer: TextBuf, props: ParaProps) -> Paragraph {
        let TextBuf {
            text,
            runs,
            images,
            links,
        } = buffer;
        let original = (!links.is_empty()).then(|| text.clone());
        let mut paragraph = self.paragraph(text, runs, props);
        if !images.is_empty() {
            paragraph.set_images(images);
        }
        if let Some(text) = original {
            self.hints
                .links
                .insert(paragraph.id, ParaLinks { text, spans: links });
        }
        paragraph
    }

    fn build_list<'a>(
        &mut self,
        node: &'a AstNode<'a>,
        list: &comrak::nodes::NodeList,
        level: u8,
        quote_depth: u8,
        group: Option<u64>,
    ) -> Vec<Block> {
        if self.first_bullet.is_none() && list.list_type == ListType::Bullet {
            self.first_bullet = Some(list.bullet_char as char);
        }
        if self.first_delim.is_none() && list.list_type == ListType::Ordered {
            self.first_delim = Some(match list.delimiter {
                ListDelimType::Paren => ')',
                ListDelimType::Period => '.',
            });
        }
        let group = group.unwrap_or_else(|| {
            let group = self.next_group;
            self.next_group += 1;
            group
        });
        let style = if list.list_type == ListType::Ordered {
            ListStyle::Numbered
        } else {
            ListStyle::Bullet
        };
        let level = level.min(MAX_LIST_LEVEL);
        if list.list_type == ListType::Ordered {
            self.hints
                .list_starts
                .entry((group, level))
                .or_insert(list.start);
        }
        let item = ListItem {
            style,
            level,
            group,
        };
        let mut out = Vec::new();

        // `build_block` can be called on an Item when it appears outside a List; in that case
        // treat the item as a one-item list.
        let children: Vec<&AstNode> = if matches!(node.data().value, NodeValue::List(_)) {
            node.children().collect()
        } else {
            vec![node]
        };

        for item_node in children {
            let mut prefix = None;
            if let NodeValue::TaskItem(task) = &item_node.data().value {
                prefix = Some(if task.symbol.is_some() {
                    "[x] ".to_owned()
                } else {
                    "[ ] ".to_owned()
                });
            }
            let mut first = true;
            let mut made = false;
            for child in item_node.children() {
                match &child.data().value {
                    NodeValue::Paragraph => {
                        let props = if quote_depth > 0 {
                            self.body_props(quote_depth)
                        } else {
                            ParaProps::default()
                        };
                        let props = ParaProps {
                            list: Some(item),
                            ..props
                        };
                        let prefix = if first { prefix.take() } else { None };
                        out.push(Block::Paragraph(self.inline_paragraph(
                            child,
                            props,
                            prefix.as_deref(),
                        )));
                        first = false;
                        made = true;
                    }
                    NodeValue::List(nested) => {
                        out.extend(self.build_list(
                            child,
                            nested,
                            level + 1,
                            quote_depth,
                            Some(group),
                        ));
                    }
                    NodeValue::TaskItem(task) => {
                        let prefix = if task.symbol.is_some() {
                            "[x] "
                        } else {
                            "[ ] "
                        };
                        let props = ParaProps {
                            list: Some(item),
                            ..ParaProps::default()
                        };
                        out.push(Block::Paragraph(self.inline_paragraph(
                            child,
                            props,
                            Some(prefix),
                        )));
                        made = true;
                    }
                    NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                        out.extend(self.build_block(child, quote_depth.saturating_add(1)));
                    }
                    _ => {
                        out.extend(self.build_block(child, quote_depth));
                    }
                }
            }
            if !made {
                // An empty item still needs a paragraph so it can be edited.
                let props = ParaProps {
                    list: Some(item),
                    ..ParaProps::default()
                };
                out.push(Block::Paragraph(self.paragraph(
                    prefix.take().unwrap_or_default(),
                    Vec::new(),
                    props,
                )));
            }
        }
        out
    }

    fn build_table<'a>(&mut self, node: &'a AstNode<'a>) -> Table {
        let (alignments, columns) = match &node.data().value {
            NodeValue::Table(table) => (table.alignments.clone(), table.num_columns.max(1)),
            _ => (Vec::new(), 1),
        };
        let width = 468.0 / columns as f32;
        let widths = vec![width.max(36.0); columns];
        let mut rows = Vec::new();
        let mut header_rows = 0u8;
        for child in node.children() {
            let NodeValue::TableRow(header) = child.data().value else {
                continue;
            };
            if header && rows.len() == usize::from(header_rows) {
                header_rows = header_rows.saturating_add(1);
            }
            let mut cells = Vec::new();
            for (index, cell) in child.children().enumerate() {
                let align = alignments
                    .get(index)
                    .copied()
                    .unwrap_or(TableAlignment::None);
                let mut text = TextBuf::default();
                self.read_inline(cell, &mut text);
                let props = ParaProps {
                    format: ParaFormat {
                        align: match align {
                            TableAlignment::Center => Some(Align::Centre),
                            TableAlignment::Right => Some(Align::End),
                            TableAlignment::Left => Some(Align::Start),
                            TableAlignment::None => None,
                        },
                        ..ParaFormat::default()
                    },
                    ..ParaProps::default()
                };
                let paragraph = self.finish(text, props);
                cells.push(TableCell::new(paragraph));
            }
            while cells.len() < columns {
                let paragraph = self.paragraph(String::new(), Vec::new(), ParaProps::default());
                cells.push(TableCell::new(paragraph));
            }
            rows.push(TableRow {
                cells,
                min_height_pt: 0.0,
                keep_together: true,
            });
        }
        if rows.is_empty() {
            let mut cells = Vec::new();
            for _ in 0..columns {
                let paragraph = self.paragraph(String::new(), Vec::new(), ParaProps::default());
                cells.push(TableCell::new(paragraph));
            }
            rows.push(TableRow {
                cells,
                min_height_pt: 0.0,
                keep_together: true,
            });
        }
        Table {
            style: None,
            id: self.alloc(),
            widths_pt: widths,
            rows,
            header_rows: header_rows.min(1),
            borders: TableBorders::default(),
        }
    }
}

fn empty_list() -> comrak::nodes::NodeList {
    comrak::nodes::NodeList {
        list_type: ListType::Bullet,
        marker_offset: 0,
        padding: 2,
        start: 1,
        delimiter: ListDelimType::Period,
        bullet_char: b'-',
        tight: true,
        is_task_list: false,
    }
}

fn add_style(styles: &mut StyleSheet, name: &str, char: CharFormat, para: ParaFormat) -> StyleId {
    styles.paragraph.push(ParagraphStyle {
        name: name.to_owned(),
        base: Some(StyleId::BODY),
        char,
        para,
        next: Some(StyleId::BODY),
        outline_level: None,
    });
    StyleId(styles.paragraph.len() as u32 - 1)
}

#[derive(Default)]
struct TextBuf {
    text: String,
    runs: Vec<Run>,
    images: Vec<InlineImage>,
    links: Vec<LinkSpan>,
}

impl TextBuf {
    fn push(&mut self, text: &str, format: &CharFormat) {
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
}

/// What inline reading needs beyond the node: where to report, and where relative image
/// targets are resolved from (`None` when the source has no folder, so no image is loaded).
struct InlineCx<'a> {
    warnings: &'a mut Vec<String>,
    base_dir: Option<&'a Path>,
}

impl InlineCx<'_> {
    fn warn(&mut self, message: String) {
        if self.warnings.len() < 100 {
            self.warnings.push(message);
        }
    }
}

/// True for a target that is a URL rather than a file path (`https:`, `data:`, `file:`,
/// `//host/..`). A single letter before the colon is a Windows drive, not a scheme.
fn is_remote_target(target: &str) -> bool {
    if target.starts_with("//") {
        return true;
    }
    let Some(colon) = target.find(':') else {
        return false;
    };
    let scheme = &target[..colon];
    scheme.len() >= 2
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            out.push((high * 16 + low) as u8);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Loads the image a Markdown target names. Never touches the network: a URL is refused.
fn load_image(base_dir: &Path, target: &str) -> Result<Arc<Media>, String> {
    if target.trim().is_empty() {
        return Err("it has no target".to_owned());
    }
    if is_remote_target(target) {
        return Err("remote images are not downloaded".to_owned());
    }
    let decoded = percent_decode(target);
    let mut found = None;
    for candidate in [target, decoded.as_str()] {
        let path = Path::new(candidate);
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            base_dir.join(path)
        };
        if full.is_file() {
            found = Some(full);
            break;
        }
    }
    let Some(path) = found else {
        return Err("the file was not found".to_owned());
    };
    let size = std::fs::metadata(&path)
        .map_err(|error| format!("the file could not be read ({error})"))?
        .len();
    if size > MAX_IMAGE_BYTES {
        return Err("the file is larger than 32 MiB".to_owned());
    }
    let bytes =
        std::fs::read(&path).map_err(|error| format!("the file could not be read ({error})"))?;
    media_from_bytes(bytes).map_err(|error| error.to_string())
}

/// The plain text inside an inline node (an image's alt text).
fn plain_text<'a>(node: &'a AstNode<'a>, out: &mut String) {
    for child in node.children() {
        match &child.data().value {
            NodeValue::Text(text) => out.push_str(text),
            NodeValue::Code(code) => out.push_str(&code.literal),
            NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
            _ => plain_text(child, out),
        }
    }
}

/// Maps inline Markdown to text and direct character formatting. An image that can be loaded
/// becomes an anchored image; one that cannot is flattened to its alt text with a warning.
/// A link is shown as its text, and its address is remembered for writing the paragraph back.
fn inline_text<'a>(
    node: &'a AstNode<'a>,
    format: &CharFormat,
    out: &mut TextBuf,
    cx: &mut InlineCx<'_>,
) {
    for child in node.children() {
        let value = child.data().value.clone();
        match value {
            NodeValue::Text(text) => out.push(&text, format),
            NodeValue::SoftBreak => out.push(" ", format),
            NodeValue::LineBreak => out.push(" ", format),
            NodeValue::Code(code) => {
                let code_format = format.overlay(&CharFormat {
                    family: Some(Arc::from("Courier New")),
                    ..CharFormat::default()
                });
                out.push(&code.literal, &code_format);
            }
            NodeValue::Strong => {
                let bold = format.overlay(&CharFormat::bold());
                inline_text(child, &bold, out, cx);
            }
            NodeValue::Emph => {
                let italic = format.overlay(&CharFormat {
                    italic: Some(true),
                    ..CharFormat::default()
                });
                inline_text(child, &italic, out, cx);
            }
            NodeValue::Strikethrough => {
                let strike = format.overlay(&CharFormat {
                    strikethrough: Some(true),
                    ..CharFormat::default()
                });
                inline_text(child, &strike, out, cx);
            }
            NodeValue::Link(link) => {
                cx.warn(
                    "a hyperlink is shown as its text; its address is kept when the paragraph is saved as Markdown"
                        .to_owned(),
                );
                let start = out.text.len();
                inline_text(child, format, out, cx);
                let end = out.text.len();
                if end > start {
                    out.links.push(LinkSpan {
                        range: start..end,
                        url: link.url.clone(),
                        title: link.title.clone(),
                    });
                }
            }
            NodeValue::Image(link) => {
                let Some(base_dir) = cx.base_dir else {
                    cx.warn("an image was flattened to its alt text".to_owned());
                    inline_text(child, format, out, cx);
                    continue;
                };
                match load_image(base_dir, &link.url) {
                    Ok(media) => {
                        let mut alt = String::new();
                        plain_text(child, &mut alt);
                        let mut image =
                            InlineImage::fitted(out.text.len(), media, IMAGE_READING_WIDTH_PT);
                        image.alt = alt;
                        image.name = link.url.clone();
                        out.images.push(image);
                    }
                    Err(reason) => {
                        let shown: String = link.url.chars().take(80).collect();
                        cx.warn(format!(
                            "the image \"{shown}\" was not loaded ({reason}); its alt text is shown instead"
                        ));
                        inline_text(child, format, out, cx);
                    }
                }
            }
            NodeValue::HtmlInline(html) => out.push(&html, format),
            NodeValue::FootnoteReference(reference) => {
                out.push(&format!("[^{}]", reference.name), format);
            }
            NodeValue::Math(math) => out.push(&math.literal, format),
            _ => inline_text(child, format, out, cx),
        }
    }
}

fn detect_emphasis(source: &str) -> char {
    let double_star = source.find("**");
    let double_under = source.find("__");
    match (double_star, double_under) {
        (Some(star), Some(under)) => {
            if under < star {
                '_'
            } else {
                '*'
            }
        }
        (Some(_), None) => '*',
        (None, Some(_)) => '_',
        (None, None) => {
            if source
                .find('_')
                .is_some_and(|under| source.find('*').is_none_or(|star| under < star))
            {
                '_'
            } else {
                '*'
            }
        }
    }
}
