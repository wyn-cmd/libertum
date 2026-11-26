//! Serialising the model as Markdown, and the fingerprints that decide which blocks are
//! still untouched.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use libertum_model::{
    Align, Block, CharFormat, Document, InlineImage, ListStyle, Media, Paragraph, Table, TableCell,
    VerticalAlign,
};

use crate::MarkdownAsset;
use crate::parse::{CodeMeta, Conventions, Hints, LinkSpan};

/// A hash of the model content of a group of blocks, including every paragraph version.
///
/// Image bytes are hashed through their cached hash rather than formatted, so a large image
/// costs nothing here.
pub(crate) fn fingerprint_blocks<B: Borrow<Block>>(blocks: &[B]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for block in blocks {
        let mut block = block.borrow().clone();
        let mut digests: Vec<(usize, u64, u32, u32, String, String)> = Vec::new();
        match &mut block {
            Block::Paragraph(paragraph) => strip_images(paragraph, &mut digests),
            Block::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        for paragraph in &mut cell.paragraphs {
                            strip_images(paragraph, &mut digests);
                        }
                    }
                }
            }
        }
        format!("{block:?}").hash(&mut hasher);
        digests.hash(&mut hasher);
    }
    hasher.finish()
}

fn strip_images(
    paragraph: &mut Paragraph,
    digests: &mut Vec<(usize, u64, u32, u32, String, String)>,
) {
    if paragraph.images().is_empty() {
        return;
    }
    for image in paragraph.images() {
        digests.push((
            image.offset,
            image.media.hash(),
            image.width_pt.to_bits(),
            image.height_pt.to_bits(),
            image.alt.clone(),
            image.name.clone(),
        ));
    }
    paragraph.set_images(Vec::new());
}

pub(crate) fn fingerprint_document(document: &Document) -> u64 {
    let blocks: Vec<&Block> = document.body().iter().collect();
    fingerprint_blocks(&blocks)
}

/// Collects what writing needs to put beside the Markdown text: image files for images that
/// have no file of their own yet, and warnings about things that could not be written.
pub(crate) struct Sink {
    stem: Option<String>,
    dir: Option<PathBuf>,
    counter: u32,
    names: HashMap<u64, String>,
    pub assets: Vec<MarkdownAsset>,
    pub warnings: Vec<String>,
}

impl Sink {
    /// `document_path` is where the Markdown will be written, if known. Without it an image
    /// that has no file name cannot be saved.
    pub fn new(document_path: Option<&Path>) -> Self {
        let stem = document_path
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().into_owned());
        let dir = document_path.map(|path| match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        });
        Self {
            stem,
            dir,
            counter: 0,
            names: HashMap::new(),
            assets: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn warn(&mut self, message: &str) {
        if self.warnings.len() < 100 && !self.warnings.iter().any(|seen| seen == message) {
            self.warnings.push(message.to_owned());
        }
    }

    /// The file name an unnamed image is saved under, `<stem>-image-<n>.<ext>`, picking the
    /// next free `n`. A file already there with the same bytes (the previous save) is reused;
    /// one with different bytes is never overwritten.
    fn image_file(&mut self, media: &Media) -> Option<String> {
        let stem = self.stem.clone()?;
        if let Some(name) = self.names.get(&media.hash()) {
            return Some(name.clone());
        }
        for _ in 0..10_000 {
            self.counter += 1;
            let name = format!(
                "{stem}-image-{}.{}",
                self.counter,
                media.format().extension()
            );
            if let Some(dir) = &self.dir {
                match std::fs::read(dir.join(&name)) {
                    Ok(existing) if existing == media.bytes() => {
                        self.names.insert(media.hash(), name.clone());
                        return Some(name);
                    }
                    Ok(_) => continue,
                    Err(_) => {}
                }
            }
            self.assets.push(MarkdownAsset {
                file_name: name.clone(),
                bytes: media.bytes().to_vec(),
            });
            self.names.insert(media.hash(), name.clone());
            return Some(name);
        }
        None
    }
}

/// Serialises a whole document.
pub(crate) fn serialise_document(
    document: &Document,
    hints: &Hints,
    conventions: Conventions,
    sink: &mut Sink,
) -> String {
    let blocks: Vec<&Block> = document.body().iter().collect();
    serialise_blocks(document, &blocks, hints, conventions, sink)
}

/// Serialises one imported top-level block, keeping the file's conventions.
pub(crate) fn serialise_group(
    document: &Document,
    blocks: &[&Block],
    hints: &Hints,
    conventions: Conventions,
    sink: &mut Sink,
) -> String {
    serialise_blocks(document, blocks, hints, conventions, sink)
}

fn serialise_blocks(
    document: &Document,
    blocks: &[&Block],
    hints: &Hints,
    conventions: Conventions,
    sink: &mut Sink,
) -> String {
    let mut writer = Writer {
        document,
        hints,
        conventions,
        sink,
    };
    let mut out = String::new();
    let mut index = 0usize;
    while index < blocks.len() {
        match blocks[index] {
            Block::Paragraph(paragraph) if paragraph.props.list.is_some() => {
                let Some(item) = paragraph.props.list else {
                    index += 1;
                    continue;
                };
                let mut run: Vec<&Paragraph> = Vec::new();
                let mut at = index;
                while let Some(Block::Paragraph(next)) = blocks.get(at) {
                    match next.props.list {
                        Some(next_item)
                            if next_item.group == item.group && next_item.style == item.style =>
                        {
                            run.push(next);
                            at += 1;
                        }
                        _ => break,
                    }
                }
                out.push_str(&writer.list(&run));
                index = at;
            }
            Block::Paragraph(paragraph) => {
                out.push_str(&writer.paragraph(paragraph));
                index += 1;
            }
            Block::Table(table) => {
                out.push_str(&writer.table(table));
                index += 1;
            }
        }
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

struct Writer<'a> {
    document: &'a Document,
    hints: &'a Hints,
    conventions: Conventions,
    sink: &'a mut Sink,
}

impl Writer<'_> {
    fn style_name(&self, paragraph: &Paragraph) -> &str {
        self.document
            .styles
            .get(paragraph.props.style)
            .map(|style| style.name.as_str())
            .unwrap_or("")
    }

    fn paragraph(&mut self, paragraph: &Paragraph) -> String {
        let style = self.style_name(paragraph).to_owned();
        if style == "Markdown Code" {
            return self.code_block(paragraph);
        }
        if style == "Markdown Rule" {
            return "---\n\n".to_owned();
        }
        if style == "Markdown Raw" {
            return format!("{}\n\n", paragraph.text());
        }
        if style == "Quote" {
            let prefix = "> ".repeat(quote_depth(paragraph));
            return format!("{prefix}{}\n\n", self.inline(paragraph));
        }
        if let Some(level) = paragraph.props.outline_level {
            let text = self.inline(paragraph);
            let level = level.clamp(1, 6);
            if self.conventions.setext && level <= 2 && !text.is_empty() {
                let underline = if level == 1 { '=' } else { '-' };
                let count = text.chars().count().max(3);
                return format!("{text}\n{}\n\n", underline.to_string().repeat(count));
            }
            return format!("{} {text}\n\n", "#".repeat(usize::from(level)));
        }
        let text = self.inline(paragraph);
        if text.is_empty() {
            return "\n".to_owned();
        }
        format!("{text}\n\n")
    }

    /// A code block. A block that came from a file is written with its own fence, info string
    /// and line breaks: the model shows it as one paragraph, so an edit is mapped back onto
    /// the original lines (the unchanged start and end keep their line breaks).
    fn code_block(&self, paragraph: &Paragraph) -> String {
        let meta: Option<&CodeMeta> = self.hints.code.get(&paragraph.id);
        let text = paragraph.text();
        let content = match meta {
            Some(meta) => reflow_code(meta, text),
            None => text.to_owned(),
        };
        let (fence_char, wanted, info) = match meta {
            Some(meta) => (meta.fence_char, meta.fence_len, meta.info.as_str()),
            None => (self.conventions.fence, 3, ""),
        };
        let longest = longest_run(&content, fence_char);
        let length = wanted.max(longest + 1).max(3);
        let fence = fence_char.to_string().repeat(length);
        let mut out = format!("{fence}{info}\n{content}");
        if !content.is_empty() && !content.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&fence);
        out.push_str("\n\n");
        out
    }

    /// The paragraph's text with its character formatting, links and images as Markdown.
    fn inline(&mut self, paragraph: &Paragraph) -> String {
        let text = paragraph.text();
        let length = text.len();
        let links = current_links(self.hints, paragraph);
        let images = paragraph.images();

        let mut cuts: Vec<usize> = vec![0, length];
        for run in paragraph.runs() {
            cuts.push(run.range.start.min(length));
            cuts.push(run.range.end.min(length));
        }
        for link in &links {
            cuts.push(link.range.start.min(length));
            cuts.push(link.range.end.min(length));
        }
        for image in images {
            cuts.push(image.offset.min(length));
        }
        cuts.retain(|cut| text.is_char_boundary(*cut));
        cuts.sort_unstable();
        cuts.dedup();

        let mut out = String::with_capacity(length + 8);
        let mut next_image = 0usize;
        let mut open: Option<&LinkSpan> = None;
        let mut skip_until = 0usize;
        for (index, &position) in cuts.iter().enumerate() {
            if let Some(link) = open
                && link.range.end <= position
            {
                out.push_str(&link_close(link));
                open = None;
            }
            while next_image < images.len() && images[next_image].offset.min(length) <= position {
                out.push_str(&self.image(&images[next_image], open.is_none()));
                next_image += 1;
            }
            if open.is_none()
                && position >= skip_until
                && let Some(link) = links
                    .iter()
                    .find(|link| link.range.start == position && link.range.end > link.range.start)
            {
                let end = link.range.end.min(length);
                let inside_image = images
                    .iter()
                    .any(|image| image.offset > position && image.offset < end);
                if text.get(position..end) == Some(link.url.as_str()) && !inside_image {
                    // A link whose text is its address was an autolink: write it as one.
                    out.push_str(&format!("<{}>", link.url));
                    skip_until = end;
                } else {
                    out.push('[');
                    open = Some(link);
                }
            }
            if let Some(&end) = cuts.get(index + 1)
                && end > position
                && position >= skip_until
            {
                let format = paragraph
                    .runs()
                    .iter()
                    .find(|run| run.range.start <= position && position < run.range.end)
                    .map(|run| run.format.clone())
                    .unwrap_or_default();
                out.push_str(&mark_text(&text[position..end], &format, self.conventions));
            }
        }
        if let Some(link) = open {
            out.push_str(&link_close(link));
        }
        out
    }

    /// `wrap_link` is false while a link's own brackets are open around this position (an
    /// image inside a link is not given a second link).
    fn image(&mut self, image: &InlineImage, wrap_link: bool) -> String {
        // A linked video has no Markdown form of its own: a link to it.
        if let Some(link) = image.link()
            && link.kind != libertum_model::MediaKind::Image
        {
            let label = if image.alt.is_empty() {
                link.url.clone()
            } else {
                image.alt.clone()
            };
            return format!("[{}]({})", escape_alt(&label), destination(&link.url));
        }
        let name = if image.name.trim().is_empty() {
            match self.sink.image_file(&image.media) {
                Some(name) => name,
                None => {
                    self.sink.warn(
                        "an image without a file name was not saved because the document has no path to save it beside; its alt text was written instead",
                    );
                    return escape_text(&image.alt);
                }
            }
        } else {
            image.name.clone()
        };
        let mut out = format!("![{}]({}", escape_alt(&image.alt), destination(&name));
        if let Some(title) = self.hints.image_titles.get(&image.name) {
            out.push_str(&format!(" \"{}\"", escape_title(title)));
        }
        out.push(')');
        if wrap_link && let Some(url) = self.hints.image_links.get(&image.name) {
            out = format!("[{}]({})", out, destination(url));
        }
        out
    }

    fn list(&mut self, paragraphs: &[&Paragraph]) -> String {
        let mut out = String::new();
        let mut counters = [0usize; 9];
        for paragraph in paragraphs {
            let Some(item) = paragraph.props.list else {
                continue;
            };
            let level = usize::from(item.level.min(8));
            counters[level] = counters[level].saturating_add(1);
            for deeper in counters.iter_mut().skip(level + 1) {
                *deeper = 0;
            }
            let indent = "    ".repeat(level);
            let marker = match item.style {
                ListStyle::Bullet => self.conventions.bullet.to_string(),
                _ => {
                    let start = self
                        .hints
                        .list_starts
                        .get(&(item.group, item.level.min(8)))
                        .copied()
                        .unwrap_or(1);
                    format!(
                        "{}{}",
                        start.saturating_add(counters[level] - 1),
                        self.conventions.ordered_delim
                    )
                }
            };
            out.push_str(&indent);
            out.push_str(&marker);
            out.push(' ');
            out.push_str(&self.inline(paragraph));
            out.push('\n');
        }
        out.push('\n');
        out
    }

    fn cell_text(&mut self, cell: &TableCell) -> String {
        if cell.merged_into_above {
            return String::new();
        }
        let mut parts = Vec::new();
        for paragraph in &cell.paragraphs {
            parts.push(self.inline(paragraph));
        }
        parts
            .join(" ")
            .replace('|', "\\|")
            .replace(['\n', '\r'], " ")
    }

    /// One row as grid columns: a cell that spans several columns is followed by empty ones.
    fn row_texts(&mut self, cells: &[TableCell]) -> Vec<String> {
        let mut out = Vec::new();
        for cell in cells {
            out.push(self.cell_text(cell));
            for _ in 1..cell.col_span.max(1) {
                out.push(String::new());
            }
        }
        out
    }

    fn table(&mut self, table: &Table) -> String {
        let columns = table.widths_pt.len().max(1);
        let row_line = |cells: &[String]| {
            let mut text = String::from("|");
            for index in 0..columns {
                text.push(' ');
                text.push_str(cells.get(index).map(String::as_str).unwrap_or(""));
                text.push_str(" |");
            }
            text.push('\n');
            text
        };

        let mut out = String::new();
        let header_count = usize::from(table.header_rows);
        let header_row = if header_count > 0 {
            table.rows.first()
        } else {
            None
        };
        let header_cells = match header_row {
            Some(row) => self.row_texts(&row.cells),
            None => vec![String::new(); columns],
        };
        out.push_str(&row_line(&header_cells));

        // Column alignment is read from the first row, where Markdown keeps it.
        let mut aligns: Vec<Option<Align>> = Vec::new();
        if let Some(first) = table.rows.first() {
            for cell in &first.cells {
                let align = cell
                    .paragraphs
                    .first()
                    .and_then(|paragraph| paragraph.props.format.align);
                aligns.push(align);
                for _ in 1..cell.col_span.max(1) {
                    aligns.push(None);
                }
            }
        }
        out.push('|');
        for index in 0..columns {
            out.push_str(match aligns.get(index).copied().flatten() {
                Some(Align::Centre) => " :---: |",
                Some(Align::End) => " ---: |",
                Some(Align::Start) => " :--- |",
                _ => " --- |",
            });
        }
        out.push('\n');

        let first_body = usize::from(table.header_rows).min(table.rows.len());
        for row in table.rows.iter().skip(first_body) {
            let cells = self.row_texts(&row.cells);
            out.push_str(&row_line(&cells));
        }
        out.push('\n');
        out
    }
}

/// How deeply a quote paragraph is nested, from its left indent (36 pt, then 18 pt more per
/// level; see the importer).
fn quote_depth(paragraph: &Paragraph) -> usize {
    match paragraph.props.format.indent_left_pt {
        Some(indent) if indent.is_finite() => {
            (((indent - 36.0) / 18.0).round().max(0.0) as usize + 1).min(6)
        }
        _ => 1,
    }
}

/// The links of a paragraph, carried over its edits. The spans were recorded against the text
/// as it was opened; the unchanged start and end of the text anchor them, so a link survives
/// typing inside it or elsewhere in the paragraph, and goes when its text is deleted.
fn current_links(hints: &Hints, paragraph: &Paragraph) -> Vec<LinkSpan> {
    let Some(saved) = hints.links.get(&paragraph.id) else {
        return Vec::new();
    };
    let now = paragraph.text();
    let old = saved.text.as_str();
    if now == old {
        return saved.spans.clone();
    }
    let prefix = common_prefix(old, now);
    let suffix = common_suffix(&old[prefix..], &now[prefix..]);
    let old_tail = old.len() - suffix;
    let now_tail = now.len() - suffix;
    let shift = |x: usize| x - old_tail + now_tail;
    let start_of = |x: usize| {
        if x >= old_tail {
            shift(x)
        } else if x <= prefix {
            x
        } else {
            now_tail
        }
    };
    let end_of = |x: usize| {
        if x <= prefix {
            x
        } else if x >= old_tail {
            shift(x)
        } else {
            prefix
        }
    };
    saved
        .spans
        .iter()
        .filter_map(|span| {
            let start = start_of(span.range.start);
            let end = end_of(span.range.end);
            (start < end && end <= now.len()).then(|| LinkSpan {
                range: start..end,
                url: span.url.clone(),
                title: span.title.clone(),
            })
        })
        .collect()
}

fn common_prefix(a: &str, b: &str) -> usize {
    let mut length = 0usize;
    for (x, y) in a.chars().zip(b.chars()) {
        if x != y {
            break;
        }
        length += x.len_utf8();
    }
    length
}

fn common_suffix(a: &str, b: &str) -> usize {
    let mut length = 0usize;
    for (x, y) in a.chars().rev().zip(b.chars().rev()) {
        if x != y {
            break;
        }
        length += x.len_utf8();
    }
    length
}

fn longest_run(text: &str, character: char) -> usize {
    let mut longest = 0usize;
    let mut current = 0usize;
    for c in text.chars() {
        if c == character {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// Maps the edited one-line text of a code block back onto the original lines. The text the
/// model holds is the code with its line breaks turned into spaces; whatever is unchanged at
/// the start and end of it keeps its original line breaks and indentation, and only the
/// changed stretch in the middle loses its line breaks.
fn reflow_code(meta: &CodeMeta, now: &str) -> String {
    if now == meta.shown {
        return meta.literal.clone();
    }
    let shown = meta.shown.as_str();
    let prefix = common_prefix(shown, now);
    let suffix = common_suffix(&shown[prefix..], &now[prefix..]);
    // `shown` is `literal` with each line break replaced by a space (same byte length) and
    // trimmed, so it starts `lead` bytes into the literal.
    let spaced = meta.literal.replace(['\n', '\r'], " ");
    let lead = spaced.len() - spaced.trim_start().len();
    let Some(head) = meta.literal.get(..lead + prefix) else {
        return now.to_owned();
    };
    let Some(tail) = meta.literal.get(lead + shown.len() - suffix..) else {
        return now.to_owned();
    };
    format!("{head}{}{tail}", &now[prefix..now.len() - suffix])
}

fn link_close(link: &LinkSpan) -> String {
    let title = if link.title.is_empty() {
        String::new()
    } else {
        let escaped = link
            .title
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace(['\n', '\r'], " ");
        format!(" \"{escaped}\"")
    };
    format!("]({}{title})", destination(&link.url))
}

/// A link or image destination, in angle brackets when it holds anything that would end a
/// bare destination.
fn destination(target: &str) -> String {
    let plain = !target.is_empty()
        && !target
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>'));
    if plain {
        target.to_owned()
    } else {
        let inner = target
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace(['\n', '\r'], "");
        format!("<{inner}>")
    }
}

fn is_monospace(family: &str) -> bool {
    let family = family.to_ascii_lowercase();
    family.contains("courier") || family.contains("mono")
}

fn mark_text(text: &str, format: &CharFormat, conventions: Conventions) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = if format.family.as_deref().is_some_and(is_monospace) {
        let ticks = if text.contains("``") {
            "```"
        } else if text.contains('`') {
            "``"
        } else {
            "`"
        };
        let spaced = text.contains('`');
        let inner = if spaced {
            format!(" {text} ")
        } else {
            text.to_owned()
        };
        format!("{ticks}{inner}{ticks}")
    } else {
        escape_text(text)
    };
    if format.strikethrough == Some(true) {
        out = format!("~~{out}~~");
    }
    if format.italic == Some(true) {
        out = format!("{}{out}{}", conventions.emphasis, conventions.emphasis);
    }
    if format.weight.is_some_and(|weight| weight >= 600) {
        out = format!("**{out}**");
    }
    // Markdown has no syntax for these three, so they are written as inline HTML, which the
    // reader turns back into formatting (parse.rs `html_tag`).
    if format.underline == Some(true) {
        out = format!("<u>{out}</u>");
    }
    match format.vertical {
        Some(VerticalAlign::Superscript) => out = format!("<sup>{out}</sup>"),
        Some(VerticalAlign::Subscript) => out = format!("<sub>{out}</sub>"),
        _ => {}
    }
    if let Some([red, green, blue, _alpha]) = format.colour {
        out = format!(r#"<span style="color:#{red:02x}{green:02x}{blue:02x}">{out}</span>"#);
    }
    out
}

fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    let mut at_start = true;
    for character in text.chars() {
        match character {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' => {
                out.push('\\');
                out.push(character);
            }
            '#' | '-' | '+' | '>' if at_start => {
                out.push('\\');
                out.push(character);
            }
            '\n' | '\r' => out.push(' '),
            other => {
                out.push(other);
                at_start = false;
                continue;
            }
        }
        at_start = false;
    }
    out
}

/// An image title goes inside the link's double quotes.
fn escape_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    for character in title.chars() {
        match character {
            '\\' | '"' => {
                out.push('\\');
                out.push(character);
            }
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

/// Alt text goes between `![` and `]`: brackets, backslashes and the inline markers that would
/// change how the text reads back are escaped, and line breaks become spaces.
fn escape_alt(alt: &str) -> String {
    let mut out = String::with_capacity(alt.len() + 4);
    for character in alt.chars() {
        match character {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '!' => {
                out.push('\\');
                out.push(character);
            }
            '\n' | '\r' => out.push(' '),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use libertum_model::{BlockId, ParaProps};

    #[test]
    fn escapes_inline_markers() {
        let text = mark_text(
            "a * b _ c [d] `e`",
            &CharFormat::default(),
            Conventions::default(),
        );
        assert_eq!(text, "a \\* b \\_ c \\[d\\] \\`e\\`");
    }

    #[test]
    fn code_spans_choose_a_safe_delimiter() {
        assert_eq!(
            mark_text(
                "x",
                &CharFormat {
                    family: Some("Courier New".into()),
                    ..CharFormat::default()
                },
                Conventions::default()
            ),
            "`x`"
        );
        assert_eq!(
            mark_text(
                "a`b",
                &CharFormat {
                    family: Some("Courier New".into()),
                    ..CharFormat::default()
                },
                Conventions::default()
            ),
            "`` a`b ``"
        );
    }

    #[test]
    fn emphasis_wraps_a_code_span() {
        let format = CharFormat {
            family: Some("Courier New".into()),
            italic: Some(true),
            ..CharFormat::bold()
        };
        assert_eq!(mark_text("x", &format, Conventions::default()), "***`x`***");
    }

    #[test]
    fn alt_text_escapes_brackets_and_markers() {
        assert_eq!(escape_alt("a [b] *c*\nd"), "a \\[b\\] \\*c\\* d");
    }

    #[test]
    fn destinations_with_spaces_use_angle_brackets() {
        assert_eq!(destination("pics/cat.png"), "pics/cat.png");
        assert_eq!(destination("my pics/cat (1).png"), "<my pics/cat (1).png>");
    }

    #[test]
    fn code_reflow_keeps_the_unchanged_lines() {
        let meta = CodeMeta {
            fence_char: '`',
            fence_len: 3,
            info: String::new(),
            literal: "fn a() {\n    one();\n    two();\n}\n".to_owned(),
            shown: "fn a() {     one();     two(); }".to_owned(),
        };
        assert_eq!(
            reflow_code(&meta, "fn a() {     one();     three(); }"),
            "fn a() {\n    one();\n    three();\n}\n"
        );
        assert_eq!(reflow_code(&meta, &meta.shown), meta.literal);
    }

    #[test]
    fn serialises_headings_and_paragraphs() {
        let paragraph =
            Paragraph::with_runs(BlockId(1), "Heading", Vec::new()).with_props(ParaProps {
                outline_level: Some(2),
                ..ParaProps::default()
            });
        let block = Block::Paragraph(paragraph);
        let document = Document::from_blocks(vec![block]);
        let text = serialise_document(
            &document,
            &Hints::default(),
            Conventions::default(),
            &mut Sink::new(None),
        );
        assert!(text.starts_with("## Heading"), "{text}");
    }
}
