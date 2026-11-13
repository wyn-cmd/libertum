//! HTML for AO3 chapters and EPUB chapters.
//!
//! Only the markup AO3 accepts is emitted: paragraphs, headings, `em`, `strong`, `u`, `s`,
//! `sup`, `sub`, `blockquote`, `ul`/`ol`/`li`, `table`/`tr`/`td` and `br`. No classes, ids,
//! styles or page furniture. Everything else (fonts, colours, spacing) is dropped, because
//! AO3 would strip it anyway.

use std::collections::HashMap;
use std::ops::Range;

use libertum_model::{
    Block, Document, ListStyle, Paragraph, ResolvedChar, StyleId, Table, VerticalAlign,
};

use crate::{Chapter, chapter_blocks};

/// The tags this export emits, kept in one place so a future AO3 whitelist change has one
/// place to look.
pub const AO3_WHITELIST: &str = "p, h1-h6, em, strong, u, s, sup, sub, blockquote, ul, ol, li, table, tbody, tr, td, br, hr, img";

/// An image copied beside an EPUB chapter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageAsset {
    /// File name inside `OEBPS/images/`.
    pub name: String,
    pub bytes: Vec<u8>,
    pub mime: &'static str,
}

/// A chapter rendered to HTML, with the images it referenced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChapterHtml {
    pub body: String,
    pub images: Vec<ImageAsset>,
}

/// The AO3 fragment for one chapter: its body only, with the chapter heading left out (AO3
/// has a separate title field). Pastes into AO3's HTML editor as-is.
pub fn ao3_fragment(document: &Document, chapter: &Chapter) -> String {
    chapter_html(document, chapter, None).body
}

/// Renders `chapter` as a fragment. `image_prefix` is the URL prefix an `<img>` needs, or
/// `None` to leave images out (AO3 chapters cannot host local files).
pub fn chapter_html(
    document: &Document,
    chapter: &Chapter,
    image_prefix: Option<&str>,
) -> ChapterHtml {
    let blocks = chapter_blocks(document, chapter);
    // The chapter's own heading is its title: AO3 shows the title separately and EPUB
    // writes its own `<h1>` from it.
    let skip = usize::from(chapter.level.is_some());
    let mut renderer = Renderer {
        document,
        image_prefix,
        images: Vec::new(),
        image_names: HashMap::new(),
    };
    let mut body = String::new();
    renderer.blocks(&blocks[skip.min(blocks.len())..], &mut body);
    ChapterHtml {
        body,
        images: renderer.images,
    }
}

struct Renderer<'a> {
    document: &'a Document,
    image_prefix: Option<&'a str>,
    images: Vec<ImageAsset>,
    image_names: HashMap<u64, String>,
}

impl Renderer<'_> {
    fn blocks(&mut self, blocks: &[&Block], out: &mut String) {
        let mut i = 0;
        let mut wrote_anything = false;
        while i < blocks.len() {
            match blocks[i] {
                Block::Table(table) => {
                    self.table(table, out);
                    wrote_anything = true;
                    i += 1;
                }
                Block::Paragraph(p) if p.props.toc_target.is_some() => {
                    // Generated contents entries are not part of the chapter text.
                    i += 1;
                }
                Block::Paragraph(p) if p.props.style == StyleId::QUOTE => {
                    out.push_str("<blockquote>\n");
                    while i < blocks.len() {
                        let Some(q) = blocks[i].as_paragraph() else {
                            break;
                        };
                        if q.props.style != StyleId::QUOTE || q.props.toc_target.is_some() {
                            break;
                        }
                        out.push_str("<p>");
                        let inner = self.paragraph_inner(q);
                        out.push_str(&inner);
                        out.push_str("</p>\n");
                        i += 1;
                    }
                    out.push_str("</blockquote>\n");
                    wrote_anything = true;
                }
                Block::Paragraph(p) if p.props.list.is_some() => {
                    let group = p.props.list.map(|list| list.group);
                    let start = i;
                    while i < blocks.len() {
                        let Some(item) = blocks[i].as_paragraph() else {
                            break;
                        };
                        let Some(list) = item.props.list else {
                            break;
                        };
                        if Some(list.group) != group {
                            break;
                        }
                        i += 1;
                    }
                    let items: Vec<&Paragraph> = blocks[start..i]
                        .iter()
                        .filter_map(|block| block.as_paragraph())
                        .collect();
                    self.list(&items, out);
                    wrote_anything = true;
                }
                Block::Paragraph(p) => {
                    if wrote_anything && (p.props.page_break_before || p.props.section_start) {
                        out.push_str("<hr />\n");
                    }
                    let heading = p.props.outline_level;
                    if let Some(level) = heading {
                        let tag = level.clamp(1, 6);
                        out.push_str(&format!("<h{tag}>"));
                        let inner = self.paragraph_inner(p);
                        out.push_str(&inner);
                        out.push_str(&format!("</h{tag}>\n"));
                    } else if p.text().is_empty() && p.images().is_empty() {
                        out.push_str("<p><br /></p>\n");
                    } else {
                        out.push_str("<p>");
                        let inner = self.paragraph_inner(p);
                        out.push_str(&inner);
                        out.push_str("</p>\n");
                    }
                    wrote_anything = true;
                    i += 1;
                }
            }
        }
    }

    /// One paragraph's runs, emphasis and inline images, without the enclosing `<p>`.
    fn paragraph_inner(&mut self, paragraph: &Paragraph) -> String {
        let text = paragraph.text();
        let base = self.document.styles.resolve_char(paragraph.props.style);
        let runs = self.runs(paragraph, &base);
        let images = paragraph.images();
        let mut out = String::new();
        let mut cursor = 0;
        let mut image_index = 0;
        for (range, format) in runs {
            if cursor < range.start {
                self.write_text(&text[cursor..range.start], &base, &mut out);
            }
            let start = range.start.max(cursor);
            let mut position = start;
            while image_index < images.len() && images[image_index].offset <= range.end {
                let offset = images[image_index].offset.max(position).min(range.end);
                if offset > position {
                    self.write_text(&text[position..offset], &format, &mut out);
                }
                self.image(&images[image_index], &mut out);
                image_index += 1;
                position = offset;
            }
            if position < range.end {
                self.write_text(&text[position..range.end], &format, &mut out);
            }
            cursor = range.end;
        }
        if cursor < text.len() {
            self.write_text(&text[cursor..], &base, &mut out);
        }
        while image_index < images.len() {
            self.image(&images[image_index], &mut out);
            image_index += 1;
        }
        out
    }

    /// A paragraph's runs with their effective character formatting. A paragraph stored
    /// without runs (plain imported text) is one run in its style's own formatting.
    fn runs(
        &self,
        paragraph: &Paragraph,
        base: &ResolvedChar,
    ) -> Vec<(Range<usize>, ResolvedChar)> {
        if paragraph.runs().is_empty() {
            return vec![(0..paragraph.text().len(), base.clone())];
        }
        paragraph
            .runs()
            .iter()
            .map(|run| (run.range.clone(), base.apply(&run.format)))
            .collect()
    }

    fn write_text(&mut self, text: &str, format: &ResolvedChar, out: &mut String) {
        let mut tags: Vec<(&str, &str)> = Vec::new();
        if format.is_bold() {
            tags.push(("<strong>", "</strong>"));
        }
        if format.italic {
            tags.push(("<em>", "</em>"));
        }
        if format.underline {
            tags.push(("<u>", "</u>"));
        }
        if format.strikethrough {
            tags.push(("<s>", "</s>"));
        }
        match format.vertical {
            VerticalAlign::Superscript => tags.push(("<sup>", "</sup>")),
            VerticalAlign::Subscript => tags.push(("<sub>", "</sub>")),
            VerticalAlign::Baseline => {}
        }
        for (open, _) in &tags {
            out.push_str(open);
        }
        escape_into(text, out);
        for (_, close) in tags.iter().rev() {
            out.push_str(close);
        }
    }

    fn image(&mut self, image: &libertum_model::InlineImage, out: &mut String) {
        let Some(prefix) = self.image_prefix else {
            return;
        };
        let name = match self.image_names.get(&image.media.hash()) {
            Some(name) => name.clone(),
            None => {
                let name = format!(
                    "image-{:016x}.{}",
                    image.media.hash(),
                    image.media.format().extension()
                );
                self.image_names.insert(image.media.hash(), name.clone());
                self.images.push(ImageAsset {
                    name: name.clone(),
                    bytes: image.media.bytes().to_vec(),
                    mime: image.media.format().mime(),
                });
                name
            }
        };
        out.push_str("<img src=\"");
        escape_attr_into(&format!("{prefix}{name}"), out);
        out.push('"');
        if !image.alt.is_empty() {
            out.push_str(" alt=\"");
            escape_attr_into(&image.alt, out);
            out.push('"');
        }
        out.push_str(" />");
    }

    /// A run of consecutive list items. Levels are ranked (0, 1, 2...) so a jump from level
    /// 0 to level 2 still nests one list inside the other.
    fn list(&mut self, items: &[&Paragraph], out: &mut String) {
        let mut ranks: Vec<u8> = items
            .iter()
            .filter_map(|p| p.props.list.map(|list| list.level))
            .collect();
        ranks.sort_unstable();
        ranks.dedup();
        if ranks.is_empty() {
            return;
        }
        self.list_level(items, &ranks, 0, out);
    }

    fn list_level(&mut self, items: &[&Paragraph], ranks: &[u8], rank: usize, out: &mut String) {
        let rank_of = |level: u8| ranks.iter().position(|r| *r == level).unwrap_or(0);
        let mut i = 0;
        while i < items.len() {
            let Some(first) = items[i].props.list else {
                i += 1;
                continue;
            };
            let first_rank = rank_of(first.level);
            if first_rank < rank {
                return;
            }
            let style = first.style;
            out.push_str(if style == ListStyle::Bullet {
                "<ul>\n"
            } else {
                "<ol>\n"
            });
            while i < items.len() {
                let Some(item) = items[i].props.list else {
                    break;
                };
                let item_rank = rank_of(item.level);
                if item_rank < rank {
                    break;
                }
                if item_rank > rank {
                    // A jump with no item at this rank: the nested list belongs to the
                    // previous item, or starts here.
                    let start = i;
                    while i < items.len()
                        && rank_of(items[i].props.list.map_or(item.level, |l| l.level)) > rank
                    {
                        i += 1;
                    }
                    self.list_level(&items[start..i], ranks, rank + 1, out);
                    break;
                }
                if item.style != style {
                    break;
                }
                out.push_str("<li>");
                let inner = self.paragraph_inner(items[i]);
                out.push_str(&inner);
                i += 1;
                if i < items.len()
                    && rank_of(items[i].props.list.map_or(item.level, |l| l.level)) > rank
                {
                    let start = i;
                    while i < items.len()
                        && rank_of(items[i].props.list.map_or(item.level, |l| l.level)) > rank
                    {
                        i += 1;
                    }
                    self.list_level(&items[start..i], ranks, rank + 1, out);
                }
                out.push_str("</li>\n");
            }
            out.push_str(if style == ListStyle::Bullet {
                "</ul>\n"
            } else {
                "</ol>\n"
            });
            if i >= items.len() {
                break;
            }
            let Some(next) = items[i].props.list else {
                break;
            };
            if rank_of(next.level) < rank {
                break;
            }
        }
    }

    fn table(&mut self, table: &Table, out: &mut String) {
        out.push_str("<table>\n<tbody>\n");
        for row in &table.rows {
            out.push_str("<tr>\n");
            for cell in &row.cells {
                if cell.merged_into_above {
                    continue;
                }
                out.push_str("<td");
                if cell.col_span > 1 {
                    out.push_str(&format!(" colspan=\"{}\"", cell.col_span));
                }
                out.push('>');
                for paragraph in &cell.paragraphs {
                    if paragraph.props.toc_target.is_some() {
                        continue;
                    }
                    out.push_str("<p>");
                    let inner = self.paragraph_inner(paragraph);
                    out.push_str(&inner);
                    out.push_str("</p>");
                }
                out.push_str("</td>\n");
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n</table>\n");
    }
}

/// Escapes text for an HTML text node, and turns line breaks into `<br />`.
fn escape_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' => out.push_str("<br />"),
            _ => out.push(ch),
        }
    }
}

/// Escapes a value for a double-quoted HTML attribute.
fn escape_attr_into(text: &str, out: &mut String) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}
