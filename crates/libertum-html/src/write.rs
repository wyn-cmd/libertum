//! The document model to HTML.
//!
//! [`HtmlFlavour::Full`] writes a standalone page that a browser shows much as Libertum
//! does and that Libertum reads back with little loss: for every paragraph and run, only the
//! CSS needed to get from the reader's baseline (see [`crate::style`]) to the document's own
//! resolved formatting. A `<style>` block repeats the baseline for browsers; the reader takes
//! nothing from it but the `body`, `html` and `@page` rules.
//!
//! [`HtmlFlavour::Plain`] writes semantic tags only: headings, paragraphs, emphasis, lists,
//! block quotes, tables, images and notes, with no styles or colours.

use std::fmt::Write as _;

use libertum_model::{
    Align, BOLD, Block, Borders, Document, InlineImage, Ligatures, LineSpacing, ListStyle,
    Paragraph, ResolvedChar, ResolvedPara, StyleId, Table, VerticalAlign,
};

use crate::HtmlFlavour;
use crate::css::{hex, pt};
use crate::style::{self, Baseline, RULE_STYLE};
use crate::util::{escape_attr, escape_text};

pub fn write(document: &Document, flavour: HtmlFlavour) -> String {
    let defaults = &document.styles.defaults;
    let baseline = Baseline::new(&defaults.family, defaults.size_pt, defaults.colour);
    let mut w = Writer {
        document,
        baseline,
        full: flavour == HtmlFlavour::Full,
        out: String::new(),
        notes: Vec::new(),
    };
    w.document();
    w.out
}

struct Writer<'a> {
    document: &'a Document,
    baseline: Baseline,
    full: bool,
    out: String,
    /// Notes in document order: kind is endnote, text.
    notes: Vec<(bool, String)>,
}

/// Floats closer than this are the same value (a format round trip, not a real change).
fn same(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

impl Writer<'_> {
    fn document(&mut self) {
        let title = self
            .document
            .paragraphs()
            .find(|p| p.props.outline_level.is_some() && !p.text().trim().is_empty())
            .map(|p| p.text().replace('\n', " ").trim().to_owned())
            .unwrap_or_else(|| "Untitled".to_owned());
        // The tag came through `libertum_model::language_tag`: letters, digits and hyphens.
        let language = self
            .document
            .sections
            .first()
            .and_then(|section| section.language.as_deref())
            .map(|tag| format!(" lang=\"{tag}\""))
            .unwrap_or_default();
        self.out.push_str(&format!(
            "<!DOCTYPE html>\n<html{language}>\n<head>\n<meta charset=\"utf-8\">\n"
        ));
        self.out
            .push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        self.out
            .push_str("<meta name=\"generator\" content=\"Libertum\">\n");
        // A second language, which `lang` cannot hold (the tag is letters, digits and hyphens).
        if let Some(second) = self
            .document
            .sections
            .first()
            .and_then(|section| section.language2.as_deref())
        {
            self.out.push_str(&format!(
                "<meta name=\"libertum-language-2\" content=\"{second}\">\n"
            ));
        }
        self.out.push_str("<title>");
        escape_text(&title, &mut self.out);
        self.out.push_str("</title>\n");
        if self.full {
            self.stylesheet();
        }
        self.out.push_str("</head>\n<body>\n");
        let blocks: Vec<&Block> = self.document.body().iter().collect();
        self.blocks(&blocks);
        self.footnotes();
        self.out.push_str("</body>\n</html>\n");
    }

    /// The `<style>` block: page size, document defaults and page colour (read back), then
    /// the baseline look of each tag (for browsers only).
    fn stylesheet(&mut self) {
        let doc = self.document;
        let section = doc.sections.first();
        let page = section.map(|s| s.page).unwrap_or_default();
        let d = &doc.styles.defaults;
        let mut s = String::from("<style>\n");
        let _ = writeln!(
            s,
            "@page {{ size: {} {}; margin: {} {} {} {}; }}",
            pt(page.width_pt()),
            pt(page.height_pt()),
            pt(page.margins.top),
            pt(page.margins.right),
            pt(page.margins.bottom),
            pt(page.margins.left)
        );
        let _ = write!(
            s,
            "body {{ font-family: '{}', serif; font-size: {}; color: {};",
            css_string(&d.family),
            pt(d.size_pt),
            hex(d.colour)
        );
        if let Some(colour) = section.and_then(|s| s.page_colour) {
            let _ = write!(s, " background-color: {};", hex(colour));
        }
        // Pageless: one column as wide as the page's text.
        let _ = writeln!(
            s,
            " max-width: {}; margin: 2em auto; padding: 0 1em; }}",
            pt(page.content_width_pt())
        );
        let b = &self.baseline.sheet;
        let mut rule = |selector: &str, para: ResolvedPara, char: Option<ResolvedChar>| {
            let _ = write!(
                s,
                "{selector} {{ margin: {} {} {} {}; text-indent: {};",
                pt(para.space_before_pt),
                pt(para.indent_right_pt),
                pt(para.space_after_pt),
                pt(para.indent_left_pt),
                pt(para.indent_first_pt)
            );
            if let Some(c) = char {
                let _ = write!(
                    s,
                    " font-size: {}; font-weight: {}; font-style: {};",
                    pt(c.size_pt),
                    c.weight,
                    if c.italic { "italic" } else { "normal" }
                );
            }
            if para.align == Align::Centre {
                s.push_str(" text-align: center;");
            }
            s.push_str(" }\n");
        };
        rule("p", b.resolve_para(StyleId::BODY), None);
        for level in 1..=6u8 {
            let id = style::heading_style(level);
            rule(
                &format!("h{level}"),
                b.resolve_para(id),
                Some(b.resolve_char(id)),
            );
        }
        rule(
            "blockquote p",
            b.resolve_para(StyleId::QUOTE),
            Some(b.resolve_char(StyleId::QUOTE)),
        );
        if let Some(title) = self.baseline.style_named("Title") {
            rule(
                "p[data-libertum-style=\"Title\"]",
                b.resolve_para(title),
                Some(b.resolve_char(title)),
            );
        }
        s.push_str(
            "blockquote { margin: 0; }\nul, ol { margin: 0; padding-left: 36pt; }\n\
             li { margin: 0 0 6pt; }\n\
             hr { border: 0; border-bottom: 0.75pt solid #969696; margin: 2pt 0 10pt; }\n\
             table { border-collapse: collapse; }\ntd p { margin: 0; }\n\
             img { vertical-align: baseline; }\n",
        );
        s.push_str("</style>\n");
        self.out.push_str(&s);
    }

    fn blocks(&mut self, blocks: &[&Block]) {
        let mut i = 0;
        while i < blocks.len() {
            match blocks[i] {
                Block::Table(table) => {
                    self.table(table);
                    i += 1;
                }
                Block::Paragraph(p) if p.props.toc_target.is_some() => i += 1,
                Block::Paragraph(p) if p.props.list.is_some() => {
                    let group = p.props.list.map(|l| l.group);
                    let start = i;
                    while i < blocks.len()
                        && blocks[i]
                            .as_paragraph()
                            .and_then(|q| q.props.list)
                            .map(|l| l.group)
                            == group
                        && group.is_some()
                    {
                        i += 1;
                    }
                    let items: Vec<&Paragraph> = blocks[start..i]
                        .iter()
                        .filter_map(|b| b.as_paragraph())
                        .collect();
                    self.list(&items);
                }
                Block::Paragraph(p) if self.is_quote(p) => {
                    self.out.push_str("<blockquote>\n");
                    while i < blocks.len()
                        && let Some(q) = blocks[i].as_paragraph()
                        && self.is_quote(q)
                        && q.props.toc_target.is_none()
                    {
                        self.paragraph(q, "p", StyleId::QUOTE, None);
                        i += 1;
                    }
                    self.out.push_str("</blockquote>\n");
                }
                Block::Paragraph(p) => {
                    self.top_paragraph(p);
                    i += 1;
                }
            }
        }
    }

    fn style_name(&self, p: &Paragraph) -> &str {
        self.document
            .styles
            .get(p.props.style)
            .map(|s| s.name.as_str())
            .unwrap_or("")
    }

    fn is_quote(&self, p: &Paragraph) -> bool {
        p.props.outline_level.is_none()
            && p.props.list.is_none()
            && self.style_name(p).eq_ignore_ascii_case("Quote")
    }

    fn is_rule(&self, p: &Paragraph) -> bool {
        p.text().is_empty()
            && p.images().is_empty()
            && matches!(self.style_name(p), RULE_STYLE | "Markdown Rule")
    }

    /// A paragraph outside lists and quotes: a heading, a rule or a `p`.
    fn top_paragraph(&mut self, p: &Paragraph) {
        if self.is_rule(p) {
            self.out.push_str("<hr>\n");
            return;
        }
        if let Some(level) = p.props.outline_level {
            let level = level.clamp(1, 6);
            let tag = format!("h{level}");
            self.paragraph(p, &tag, style::heading_style(level), None);
            return;
        }
        let name = self.style_name(p).to_owned();
        match self.baseline.style_named(&name) {
            Some(id) if id != StyleId::BODY && id != StyleId::QUOTE => {
                self.paragraph(p, "p", id, Some(&name));
            }
            _ => self.paragraph(p, "p", StyleId::BODY, None),
        }
    }

    /// Writes `<tag ...>content</tag>` for a paragraph read back with style `base`.
    fn paragraph(&mut self, p: &Paragraph, tag: &str, base: StyleId, named: Option<&str>) {
        self.open_paragraph(p, tag, base, named);
        self.inline(p, base);
        let _ = writeln!(self.out, "</{tag}>");
    }

    fn open_paragraph(&mut self, p: &Paragraph, tag: &str, base: StyleId, named: Option<&str>) {
        let _ = write!(self.out, "<{tag}");
        if !self.full {
            // Plain keeps no styles, but alignment travels as the `align` attribute, which the
            // reader understands and AO3 accepts.
            let align = self.document.styles.resolve_props(&p.props).align;
            if let Some(value) = align.html_value() {
                let _ = write!(self.out, " align=\"{value}\"");
            }
            self.out.push('>');
            return;
        }
        if let Some(name) = named {
            self.out.push_str(" data-libertum-style=\"");
            escape_attr(name, &mut self.out);
            self.out.push('"');
        }
        let baseline_props = libertum_model::ParaProps {
            style: base,
            list: p.props.list,
            ..libertum_model::ParaProps::default()
        };
        let from = self.baseline.sheet.resolve_props(&baseline_props);
        let to = self.document.styles.resolve_props(&p.props);
        let mut css = para_css(&from, &to);
        if p.props.page_break_before || p.props.section_start {
            css.push("break-before: page".to_owned());
        }
        if needs_pre_wrap(p.text()) {
            css.push("white-space: pre-wrap".to_owned());
        }
        if !css.is_empty() {
            self.out.push_str(" style=\"");
            escape_attr(&css.join("; "), &mut self.out);
            self.out.push('"');
        }
        self.out.push('>');
    }

    /// A paragraph's text, formatting, images, bookmarks and note references.
    fn inline(&mut self, p: &Paragraph, base: StyleId) {
        let text = p.text();
        if text.is_empty() && p.images().is_empty() && p.notes().is_empty() {
            // An empty paragraph needs content to keep its height in a browser; the reader
            // drops a final line break.
            self.bookmarks_at(p, 0);
            self.out.push_str("<br>");
            return;
        }
        let from = self.baseline.sheet.resolve_char(base);
        let style_char = self.document.styles.resolve_char(p.props.style);
        let mut cuts: Vec<usize> = vec![0, text.len()];
        for run in p.runs() {
            cuts.push(run.range.start);
            cuts.push(run.range.end);
        }
        cuts.extend(p.images().iter().map(|i| i.offset));
        cuts.extend(p.bookmarks().iter().map(|b| b.offset));
        for r in p.ruby() {
            cuts.push(r.range.start);
            cuts.push(r.range.end);
        }
        cuts.extend(p.notes().iter().map(|n| n.offset));
        cuts.retain(|&c| c <= text.len() && text.is_char_boundary(c));
        cuts.sort_unstable();
        cuts.dedup();
        for (index, &at) in cuts.iter().enumerate() {
            self.bookmarks_at(p, at);
            for image in p.images().iter().filter(|i| i.offset == at) {
                self.image(image);
            }
            for note in p.notes().iter().filter(|n| n.offset == at) {
                self.notes.push((
                    note.kind == libertum_model::NoteKind::Endnote,
                    note.text.clone(),
                ));
                let n = self.notes.len();
                let _ = write!(
                    self.out,
                    "<sup class=\"footnote-ref\"><a href=\"#fn-{n}\" id=\"fnref-{n}\">{n}</a></sup>"
                );
            }
            let Some(&next) = cuts.get(index + 1) else {
                break;
            };
            if next <= at {
                continue;
            }
            let format = p
                .runs()
                .iter()
                .find(|r| r.range.start <= at && at < r.range.end)
                .map(|r| style_char.apply(&r.format))
                .unwrap_or_else(|| style_char.clone());
            // A reading goes in `ruby` around its base; `rp` holds the brackets a browser
            // without ruby support shows.
            for _ in p.ruby().iter().filter(|r| r.range.start == at) {
                self.out.push_str("<ruby>");
            }
            self.text(&text[at..next], &from, &format);
            for r in p.ruby().iter().filter(|r| r.range.end == next) {
                self.out.push_str("<rp>(</rp><rt>");
                escape_text(&r.text, &mut self.out);
                self.out.push_str("</rt><rp>)</rp></ruby>");
            }
        }
    }

    fn bookmarks_at(&mut self, p: &Paragraph, at: usize) {
        for bookmark in p.bookmarks().iter().filter(|b| b.offset == at) {
            self.out.push_str("<a id=\"");
            escape_attr(&bookmark.name, &mut self.out);
            self.out.push_str("\"></a>");
        }
    }

    fn text(&mut self, text: &str, from: &ResolvedChar, to: &ResolvedChar) {
        let (tags, css) = char_diff(from, to, self.full);
        if !css.is_empty() {
            self.out.push_str("<span style=\"");
            escape_attr(&css.join("; "), &mut self.out);
            self.out.push_str("\">");
        }
        for tag in &tags {
            let _ = write!(self.out, "<{tag}>");
        }
        let mut first = true;
        for line in text.split('\n') {
            if !first {
                self.out.push_str("<br>");
            }
            first = false;
            escape_text(line, &mut self.out);
        }
        for tag in tags.iter().rev() {
            let _ = write!(self.out, "</{tag}>");
        }
        if !css.is_empty() {
            self.out.push_str("</span>");
        }
    }

    fn image(&mut self, image: &InlineImage) {
        let media = &image.media;
        let _ = write!(
            self.out,
            "<img src=\"data:{};base64,{}\"",
            media.format().mime(),
            crate::base64::encode(media.bytes())
        );
        self.out.push_str(" alt=\"");
        escape_attr(&image.alt, &mut self.out);
        self.out.push('"');
        if self.full {
            let _ = write!(
                self.out,
                " style=\"width: {}; height: {}\"",
                pt(image.width_pt),
                pt(image.height_pt)
            );
        } else {
            let _ = write!(
                self.out,
                " width=\"{}\" height=\"{}\"",
                (image.width_pt / 0.75).round() as u32,
                (image.height_pt / 0.75).round() as u32
            );
        }
        self.out.push('>');
    }

    /// Consecutive items of one list, nested by level.
    fn list(&mut self, items: &[&Paragraph]) {
        // Open lists: (level, tag).
        let mut open: Vec<(u8, &'static str)> = Vec::new();
        for item in items {
            let Some(list) = item.props.list else {
                continue;
            };
            let tag = list_tag(list.style);
            // Close deeper lists, and the previous item at this level.
            while let Some(&(level, open_tag)) = open.last() {
                if level > list.level || (level == list.level && open_tag != tag) {
                    let _ = write!(self.out, "</li>\n</{open_tag}>\n");
                    open.pop();
                } else {
                    break;
                }
            }
            match open.last() {
                Some(&(level, _)) if level == list.level => self.out.push_str("</li>\n"),
                _ => {
                    self.out.push('<');
                    self.out.push_str(tag);
                    match list.style {
                        ListStyle::Lettered => self.out.push_str(" type=\"A\""),
                        ListStyle::Harvard => self.out.push_str(" type=\"I\""),
                        _ => {}
                    }
                    self.out.push_str(">\n");
                    open.push((list.level, tag));
                }
            }
            self.open_paragraph(item, "li", StyleId::BODY, None);
            self.inline(item, StyleId::BODY);
        }
        while let Some((_, tag)) = open.pop() {
            let _ = write!(self.out, "</li>\n</{tag}>\n");
        }
    }

    fn table(&mut self, table: &Table) {
        let b = table.borders;
        let visible = b.is_visible();
        let line = format!("{} solid {}", pt(b.width_pt), hex(b.colour));
        if self.full {
            self.out
                .push_str("<table style=\"border-collapse: collapse");
            if visible && b.outer {
                let _ = write!(self.out, "; border: {line}");
            }
            self.out.push_str("\">\n<colgroup>");
            for width in &table.widths_pt {
                let _ = write!(self.out, "<col style=\"width: {}\">", pt(*width));
            }
            self.out.push_str("</colgroup>\n");
        } else if visible {
            self.out.push_str("<table border=\"1\">\n");
        } else {
            self.out.push_str("<table>\n");
        }
        let header_rows = usize::from(table.header_rows).min(table.rows.len());
        for (r, row) in table.rows.iter().enumerate() {
            if r == 0 && header_rows > 0 {
                self.out.push_str("<thead>\n");
            }
            if r == header_rows {
                self.out.push_str("<tbody>\n");
            }
            self.out.push_str("<tr>\n");
            for (c, cell) in row.cells.iter().enumerate() {
                if cell.merged_into_above {
                    continue;
                }
                self.out.push_str("<td");
                if cell.col_span > 1 {
                    let _ = write!(self.out, " colspan=\"{}\"", cell.col_span);
                }
                let rowspan = rowspan(table, r, c);
                if rowspan > 1 {
                    let _ = write!(self.out, " rowspan=\"{rowspan}\"");
                }
                if self.full {
                    let mut css = Vec::new();
                    if let Some(fill) = table.resolved_fill(r, cell) {
                        css.push(format!("background-color: {}", hex(fill)));
                    }
                    if visible && b.inner {
                        css.push(format!("border: {line}"));
                    }
                    if !css.is_empty() {
                        let _ = write!(self.out, " style=\"{}\"", css.join("; "));
                    }
                }
                self.out.push('>');
                for paragraph in &cell.paragraphs {
                    if paragraph.props.toc_target.is_some() {
                        continue;
                    }
                    self.paragraph(paragraph, "p", StyleId::BODY, None);
                }
                self.out.push_str("</td>\n");
            }
            self.out.push_str("</tr>\n");
            if r + 1 == header_rows {
                self.out.push_str("</thead>\n");
            }
        }
        if header_rows < table.rows.len() {
            self.out.push_str("</tbody>\n");
        }
        self.out.push_str("</table>\n");
    }

    fn footnotes(&mut self) {
        if self.notes.is_empty() {
            return;
        }
        self.out
            .push_str("<section class=\"footnotes\">\n<hr>\n<ol>\n");
        let notes = std::mem::take(&mut self.notes);
        for (index, (endnote, text)) in notes.iter().enumerate() {
            let n = index + 1;
            let _ = write!(self.out, "<li id=\"fn-{n}\"");
            if *endnote {
                self.out.push_str(" data-kind=\"endnote\"");
            }
            self.out.push_str("><p>");
            let mut first = true;
            for line in text.split('\n') {
                if !first {
                    self.out.push_str("<br>");
                }
                first = false;
                escape_text(line, &mut self.out);
            }
            let _ = writeln!(
                self.out,
                " <a href=\"#fnref-{n}\" class=\"footnote-back\">\u{21a9}</a></p></li>"
            );
        }
        self.out.push_str("</ol>\n</section>\n");
    }
}

fn list_tag(style: ListStyle) -> &'static str {
    match style {
        ListStyle::Bullet => "ul",
        _ => "ol",
    }
}

/// How many rows the cell at (`row`, `cell`) covers: itself and the merged cells below it in
/// the same grid column.
fn rowspan(table: &Table, row: usize, cell: usize) -> usize {
    let Some(col) = table.column_of(row, cell) else {
        return 1;
    };
    let mut span = 1;
    for below in row + 1..table.rows.len() {
        match table.cell_at_column(below, col) {
            Some((_, c)) if c.merged_into_above => span += 1,
            _ => break,
        }
    }
    span
}

/// Text whose spaces a browser would collapse.
fn needs_pre_wrap(text: &str) -> bool {
    text.starts_with(' ')
        || text.ends_with(' ')
        || text.contains("  ")
        || text.contains('\t')
        || text.contains(" \n")
        || text.contains("\n ")
}

/// Text for a single-quoted CSS string inside a double-quoted attribute.
fn css_string(text: &str) -> String {
    text.replace(['\'', '"', '\\', ';', '<', '>'], "")
}

/// The CSS that turns paragraph formatting `from` into `to`.
fn para_css(from: &ResolvedPara, to: &ResolvedPara) -> Vec<String> {
    let mut css = Vec::new();
    if from.align != to.align {
        let value = match to.align {
            Align::Start => "left",
            Align::Centre => "center",
            Align::End => "right",
            Align::Justify => "justify",
        };
        css.push(format!("text-align: {value}"));
    }
    let lengths = [
        ("margin-top", from.space_before_pt, to.space_before_pt),
        ("margin-bottom", from.space_after_pt, to.space_after_pt),
        ("margin-left", from.indent_left_pt, to.indent_left_pt),
        ("margin-right", from.indent_right_pt, to.indent_right_pt),
        ("text-indent", from.indent_first_pt, to.indent_first_pt),
    ];
    for (key, a, b) in lengths {
        if !same(a, b) {
            css.push(format!("{key}: {}", pt(b)));
        }
    }
    if from.line_spacing != to.line_spacing {
        match to.line_spacing {
            LineSpacing::Multiple(m) => css.push(format!("line-height: {m}")),
            LineSpacing::Exactly(v) => css.push(format!("line-height: {}", pt(v))),
            LineSpacing::AtLeast(v) => {
                css.push(format!("line-height: {}", pt(v)));
                css.push(format!("--libertum-line: at-least {}", pt(v)));
            }
            LineSpacing::Leading(v) => {
                css.push(format!("line-height: calc(1.15em + {})", pt(v)));
                css.push(format!("--libertum-line: leading {}", pt(v)));
            }
        }
    }
    let flag = |css: &mut Vec<String>, a: bool, b: bool, on: &str, off: &str| {
        if a != b {
            css.push(if b { on } else { off }.to_owned());
        }
    };
    flag(
        &mut css,
        from.keep_with_next,
        to.keep_with_next,
        "break-after: avoid",
        "break-after: auto",
    );
    flag(
        &mut css,
        from.keep_together,
        to.keep_together,
        "break-inside: avoid",
        "break-inside: auto",
    );
    flag(
        &mut css,
        from.widow_control,
        to.widow_control,
        "widows: 2; orphans: 2",
        "widows: 1; orphans: 1",
    );
    flag(
        &mut css,
        from.hyphenate,
        to.hyphenate,
        "hyphens: auto",
        "hyphens: manual",
    );
    if from.borders != to.borders {
        css.push("border: none".to_owned());
        if let Some(b) = to.borders {
            border_sides(&b, &mut css);
        }
    }
    css
}

fn border_sides(b: &Borders, css: &mut Vec<String>) {
    let line = format!("{} solid {}", pt(b.width_pt), hex(b.colour));
    for (on, side) in [
        (b.sides.top, "top"),
        (b.sides.right, "right"),
        (b.sides.bottom, "bottom"),
        (b.sides.left, "left"),
    ] {
        if on {
            css.push(format!("border-{side}: {line}"));
        }
    }
    css.push(format!("padding: {}", pt(b.space_pt)));
}

/// The tags and CSS that turn character formatting `from` into `to`. The span with the CSS
/// goes outside the tags, so a tag that turns something on wins over a `none` in the CSS.
fn char_diff(
    from: &ResolvedChar,
    to: &ResolvedChar,
    full: bool,
) -> (Vec<&'static str>, Vec<String>) {
    let mut tags = Vec::new();
    let mut css = Vec::new();
    let bold = |w: u16| w >= 600;
    if to.weight != from.weight {
        if bold(to.weight) && !bold(from.weight) && (to.weight == BOLD || !full) {
            tags.push("strong");
        } else if full {
            css.push(format!("font-weight: {}", to.weight));
        }
    }
    if to.italic && !from.italic {
        tags.push("em");
    } else if !to.italic && from.italic && full {
        css.push("font-style: normal".to_owned());
    }
    if (from.underline && !to.underline) || (from.strikethrough && !to.strikethrough) {
        if full {
            css.push("text-decoration: none".to_owned());
        }
        if to.underline {
            tags.push("u");
        }
        if to.strikethrough {
            tags.push("s");
        }
    } else {
        if to.underline && !from.underline {
            tags.push("u");
        }
        if to.strikethrough && !from.strikethrough {
            tags.push("s");
        }
    }
    if to.vertical != from.vertical {
        match to.vertical {
            VerticalAlign::Superscript => tags.push("sup"),
            VerticalAlign::Subscript => tags.push("sub"),
            VerticalAlign::Baseline => {
                if full {
                    css.push("vertical-align: baseline".to_owned());
                }
            }
        }
    }
    if !full {
        return (tags, css);
    }
    if to.family != from.family {
        css.push(format!("font-family: '{}'", css_string(&to.family)));
    }
    if !same(to.size_pt, from.size_pt) {
        css.push(format!("font-size: {}", pt(to.size_pt)));
    }
    if to.colour[..3] != from.colour[..3] {
        css.push(format!("color: {}", hex(to.colour)));
    }
    if to.highlight != from.highlight {
        match to.highlight {
            Some(c) => css.push(format!("background-color: {}", hex(c))),
            None => css.push("background-color: transparent".to_owned()),
        }
    }
    if !same(to.letter_spacing_pt, from.letter_spacing_pt) {
        css.push(format!("letter-spacing: {}", pt(to.letter_spacing_pt)));
    }
    if to.ligatures != from.ligatures {
        css.push(
            match to.ligatures {
                Ligatures::None => "font-variant-ligatures: none",
                Ligatures::Standard => "font-variant-ligatures: normal",
                Ligatures::All => {
                    "font-variant-ligatures: common-ligatures discretionary-ligatures"
                }
            }
            .to_owned(),
        );
    }
    (tags, css)
}
