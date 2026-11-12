//! Phase 10: stitching several documents into one book.
//!
//! The first document supplies the document defaults and the style order. Every later document
//! follows as a new section:
//!
//! - paragraph and table ids are shifted past the ids already in use, so nothing collides;
//! - `toc_target` references (which point at chapter headings by id) shift with them;
//! - list groups are renumbered, so numbering does not continue from one source into the next;
//! - the source's first paragraph is marked as a section start, giving it a fresh page and its
//!   own headers and footers (an empty paragraph carries the mark when the source begins with
//!   a table, because a section starts at a paragraph);
//! - paragraph styles are matched by name: a name the book already has is reused as it is
//!   (what Word does when a file is inserted into another), a new name is appended, and every
//!   paragraph of the source is pointed at the style's place in the book's style sheet. Style
//!   ids are positions in a document's own sheet, so without this a source whose sheet is in
//!   another order would show its text in the wrong styles;
//! - a bookmark whose name the book already uses is renamed (`Intro` becomes `Intro_2`) and
//!   the source's own cross-references follow it, so a reference never jumps to another
//!   document's bookmark;
//! - the book keeps one table of contents, the first one found. Every later source's own
//!   contents list is left out (with its "Contents" title): it would list only that source's
//!   chapters and never be updated.
//!
//! The table of contents that is kept is rebuilt here from every chapter heading in the book,
//! so the editor finds it up to date when the stitched document opens.
//!
//! The blocks are gathered in one list and the document is built once at the end: indexing a
//! document is O(paragraphs), and doing it per source or per contents entry made large
//! stitches slow.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use libertum_model::{
    Block, BlockId, BlockList, Document, ParaProps, Paragraph, Section, StyleId, StyleSheet,
};

/// Why a stitch could not happen.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StitchError {
    #[error("there is nothing to stitch")]
    Empty,
}

/// A document read for stitching, with the file it came from.
#[derive(Debug, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub document: Document,
}

/// Title the editor gives a generated table of contents (`Editor::insert_table_of_contents`).
const TOC_TITLE: &str = "Contents";

/// Calls `visit` on every paragraph in `blocks`, table cells included.
fn for_each_paragraph(blocks: &mut [Block], mut visit: impl FnMut(&mut Paragraph)) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => visit(paragraph),
            Block::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        for paragraph in &mut cell.paragraphs {
                            visit(paragraph);
                        }
                    }
                }
            }
        }
    }
}

/// Where each of `source`'s styles lives in `book`: matched by name, appended when new.
/// Appended styles have their own `base` and `next` pointed at the book's sheet too.
fn merge_styles(book: &mut StyleSheet, source: &StyleSheet) -> Vec<StyleId> {
    let mut map = Vec::with_capacity(source.paragraph.len());
    let mut appended = Vec::new();
    for style in &source.paragraph {
        let known = book
            .paragraph
            .iter()
            .position(|existing| existing.name.eq_ignore_ascii_case(&style.name));
        let index = match known {
            Some(index) => index,
            None => {
                book.paragraph.push(style.clone());
                appended.push(book.paragraph.len() - 1);
                book.paragraph.len() - 1
            }
        };
        map.push(StyleId(index as u32));
    }
    let remap = |id: StyleId| map.get(id.0 as usize).copied();
    for index in appended {
        let style = &mut book.paragraph[index];
        style.base = style.base.and_then(remap);
        style.next = style.next.and_then(remap);
    }
    map
}

/// Leaves out the source's own table of contents: its entries, and the "Contents" title
/// directly before them. A page or section start on a removed paragraph moves to the
/// paragraph that follows, so the page flow is unchanged.
fn drop_contents(blocks: Vec<Block>) -> Vec<Block> {
    let is_entry = |block: &Block| {
        block
            .as_paragraph()
            .is_some_and(|p| p.props.toc_target.is_some())
    };
    if !blocks.iter().any(is_entry) {
        return blocks;
    }
    let mut remove = vec![false; blocks.len()];
    for (index, block) in blocks.iter().enumerate() {
        if !is_entry(block) {
            continue;
        }
        remove[index] = true;
        // The title sits right before the first entry of a run.
        if index > 0
            && !remove[index - 1]
            && blocks[index - 1]
                .as_paragraph()
                .is_some_and(|p| p.text().trim().eq_ignore_ascii_case(TOC_TITLE))
        {
            remove[index - 1] = true;
        }
    }
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    let (mut page_break, mut section_start) = (false, false);
    for (block, removed) in blocks.into_iter().zip(remove) {
        match block {
            Block::Paragraph(paragraph) if removed => {
                page_break |= paragraph.props.page_break_before;
                section_start |= paragraph.props.section_start;
            }
            Block::Paragraph(mut paragraph) => {
                if page_break || section_start {
                    let mut props = paragraph.props;
                    props.page_break_before |= page_break;
                    props.section_start |= section_start;
                    paragraph.props = props;
                    (page_break, section_start) = (false, false);
                }
                out.push(Block::Paragraph(paragraph));
            }
            other => out.push(other),
        }
    }
    out
}

/// Number of sections the blocks need: one, plus one per top-level section start.
fn sections_needed(blocks: &[Block]) -> usize {
    1 + blocks
        .iter()
        .filter_map(Block::as_paragraph)
        .filter(|p| p.props.section_start)
        .count()
}

/// `sections` cut or padded (by repeating the last) to exactly `count`.
fn fit_sections(mut sections: Vec<Section>, count: usize) -> Vec<Section> {
    sections.truncate(count);
    while sections.len() < count {
        sections.push(sections.last().cloned().unwrap_or_default());
    }
    sections
}

/// A bookmark name the book does not use yet, based on `name`.
fn free_name(name: &str, taken: &HashSet<String>) -> String {
    let mut number = 2u32;
    loop {
        let suffix = format!("_{number}");
        // Word allows 40 characters.
        let keep: String = name
            .chars()
            .take(40usize.saturating_sub(suffix.len()))
            .collect();
        let candidate = format!("{keep}{suffix}");
        if !taken.contains(&candidate.to_ascii_lowercase()) {
            return candidate;
        }
        number += 1;
    }
}

/// Renames the source's bookmarks that clash with the book's and points the source's own
/// cross-references at the new names. `taken` holds the book's names in lower case and gains
/// the source's.
fn separate_bookmarks(blocks: &mut [Block], taken: &mut HashSet<String>) {
    // First pass: decide the new names (a name may be referred to from anywhere in the source).
    let mut renamed: HashMap<String, String> = HashMap::new();
    for_each_paragraph(blocks, |paragraph| {
        for bookmark in paragraph.bookmarks() {
            let key = bookmark.name.to_ascii_lowercase();
            if renamed.contains_key(&key) {
                continue;
            }
            if taken.contains(&key) {
                let fresh = free_name(&bookmark.name, taken);
                taken.insert(fresh.to_ascii_lowercase());
                renamed.insert(key, fresh);
            } else {
                taken.insert(key);
            }
        }
    });
    if renamed.is_empty() {
        return;
    }
    for_each_paragraph(blocks, |paragraph| {
        let new_name = |name: &str| renamed.get(&name.to_ascii_lowercase()).cloned();
        if paragraph
            .bookmarks()
            .iter()
            .any(|b| new_name(&b.name).is_some())
        {
            let mut bookmarks = paragraph.bookmarks().to_vec();
            for bookmark in &mut bookmarks {
                if let Some(name) = new_name(&bookmark.name) {
                    bookmark.name = name;
                }
            }
            paragraph.set_bookmarks(bookmarks);
        }
        if paragraph
            .cross_refs()
            .iter()
            .any(|r| new_name(&r.target).is_some())
        {
            let mut cross_refs = paragraph.cross_refs().to_vec();
            for cross_ref in &mut cross_refs {
                if let Some(name) = new_name(&cross_ref.target) {
                    cross_ref.target = name;
                }
            }
            paragraph.set_cross_refs(cross_refs);
        }
    });
}

/// Rebuilds the book's table of contents (the first run of entries) from every chapter
/// heading, in order. Returns the next free id. Without a contents list, or without
/// chapters, nothing changes.
fn rebuild_contents(blocks: &mut Vec<Block>, mut next_id: u64) -> u64 {
    let entry = |block: &Block| block.as_paragraph().and_then(|p| p.props.toc_target);
    let Some(start) = blocks.iter().position(|b| entry(b).is_some()) else {
        return next_id;
    };
    let end = blocks[start..]
        .iter()
        .position(|b| entry(b).is_none())
        .map_or(blocks.len(), |offset| start + offset);
    let chapters: Vec<(BlockId, String)> = blocks
        .iter()
        .filter_map(Block::as_paragraph)
        .filter(|p| p.props.outline_level == Some(1) && p.props.toc_target.is_none())
        .map(|p| (p.id, p.text().to_owned()))
        .collect();
    if chapters.is_empty() {
        return next_id;
    }
    let Some(template) = blocks[start].as_paragraph().map(|p| p.props) else {
        return next_id;
    };
    let entries: Vec<Block> = chapters
        .into_iter()
        .map(|(target, text)| {
            let id = BlockId(next_id);
            next_id += 1;
            Block::Paragraph(Paragraph::new(id, text).with_props(ParaProps {
                toc_target: Some(target),
                ..template
            }))
        })
        .collect();
    blocks.splice(start..end, entries);
    next_id
}

/// Combines `documents` in the given order into one book. See the module notes for the rules.
pub fn stitch(documents: Vec<Document>) -> Result<Document, StitchError> {
    let mut sources = documents.into_iter();
    let Some(first) = sources.next() else {
        return Err(StitchError::Empty);
    };
    let mut styles = first.styles.clone();
    let mut next_id = first.next_free_id();
    let mut list_group = first.max_list_group();
    let mut blocks: Vec<Block> = first.body().iter().cloned().collect();
    let mut sections = fit_sections(first.sections.clone(), sections_needed(&blocks));
    let mut has_contents = blocks
        .iter()
        .filter_map(Block::as_paragraph)
        .any(|p| p.props.toc_target.is_some());
    let mut names: HashSet<String> = first
        .paragraphs()
        .flat_map(|p| p.bookmarks().iter().map(|b| b.name.to_ascii_lowercase()))
        .collect();
    drop(first);

    for mut source in sources {
        // A source that starts with a table needs a paragraph to carry its section break, so
        // the ids it shifts to leave one id free for that paragraph.
        let marker = matches!(source.body().at(0), Some(Block::Table(_)));
        let marker_id = next_id;
        let shift = marker_id + u64::from(marker);
        source.restamp(shift, list_group + 1);
        list_group = list_group.max(source.max_list_group());
        next_id = next_id.max(source.next_free_id()).max(shift + 1);

        let style_map = merge_styles(&mut styles, &source.styles);
        let mut incoming: Vec<Block> = source.body().iter().cloned().collect();
        let source_sections = source.sections.clone();
        drop(source);

        // The book keeps the first table of contents it meets and no other.
        let brings_contents = incoming
            .iter()
            .filter_map(Block::as_paragraph)
            .any(|p| p.props.toc_target.is_some());
        if has_contents {
            incoming = drop_contents(incoming);
        } else {
            has_contents = brings_contents;
        }

        for_each_paragraph(&mut incoming, |paragraph| {
            // A style id past the source's own sheet (a damaged file) falls back to Body.
            let style = style_map
                .get(paragraph.props.style.0 as usize)
                .copied()
                .unwrap_or(StyleId::BODY);
            if style != paragraph.props.style {
                let mut props = paragraph.props;
                props.style = style;
                paragraph.props = props;
            }
        });
        separate_bookmarks(&mut incoming, &mut names);

        // The source's own sections, one per section start it holds, before the start that
        // joins it to the book is added.
        let own_sections = fit_sections(source_sections, sections_needed(&incoming));
        match incoming.first_mut() {
            Some(Block::Paragraph(first)) if !marker => {
                let mut props = first.props;
                if props.section_start {
                    // Already a section start (unusual at the top of a document): it opens
                    // the source's first section, so no extra section is needed.
                    sections.extend(own_sections.into_iter().skip(1));
                } else {
                    props.section_start = true;
                    first.props = props;
                    sections.extend(own_sections);
                }
            }
            Some(_) => {
                let paragraph = Paragraph::new(BlockId(marker_id), "").with_props(ParaProps {
                    section_start: true,
                    ..ParaProps::default()
                });
                incoming.insert(0, Block::Paragraph(paragraph));
                sections.extend(own_sections);
            }
            // Nothing left of the source (it was only a contents list).
            None => continue,
        }
        blocks.append(&mut incoming);
    }

    next_id = rebuild_contents(&mut blocks, next_id);
    let sections = fit_sections(sections, sections_needed(&blocks));
    Ok(Document::from_parts(
        BlockList::from(blocks),
        next_id,
        sections,
        styles,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use libertum_model::{ListItem, ListStyle};

    fn doc(texts: &[&str]) -> Document {
        Document::from_paragraphs(
            texts
                .iter()
                .map(|text| Paragraph::new(BlockId(0), (*text).to_owned()))
                .collect(),
        )
    }

    fn texts(document: &Document) -> Vec<String> {
        document
            .paragraphs()
            .map(|paragraph| paragraph.text().to_owned())
            .collect()
    }

    #[test]
    fn documents_follow_in_order_and_each_starts_a_section() {
        let book =
            stitch(vec![doc(&["One", "Two"]), doc(&["Three"]), doc(&["Four"])]).expect("a book");
        assert_eq!(texts(&book), ["One", "Two", "Three", "Four"]);
        assert_eq!(book.sections.len(), 3, "one section per source");
        let starts: Vec<bool> = book
            .paragraphs()
            .map(|paragraph| paragraph.props.section_start)
            .collect();
        assert_eq!(
            starts,
            [false, false, true, true],
            "each source opens a section"
        );
    }

    #[test]
    fn a_source_that_starts_with_a_table_gets_a_section_paragraph_first() {
        let mut second = doc(&["Tail"]);
        second.append_blocks(vec![Block::Paragraph(Paragraph::new(BlockId(0), "After"))]);
        // Build a two-cell table by hand, then put it first.
        let table = second.new_table(1, 2, 400.0);
        let mut blocks = vec![Block::Table(table)];
        blocks.extend(second.body().iter().cloned());
        let second = Document::from_blocks(blocks);

        let book = stitch(vec![doc(&["One"]), second]).expect("a book");
        let body = book.body();
        let Some(Block::Paragraph(marker)) = body.at(1) else {
            panic!("a section paragraph was expected before the table");
        };
        assert!(marker.props.section_start);
        assert_eq!(marker.text(), "");
        assert!(matches!(body.at(2), Some(Block::Table(_))));
    }

    #[test]
    fn ids_do_not_collide_and_toc_targets_follow_their_heading() {
        // The second document has a TOC entry pointing at its own first paragraph.
        let mut second = doc(&["Chapter Two", "Body"]);
        let heading_id = second.paragraph_at(0).expect("heading").id;
        let entry = Paragraph::new(BlockId(0), "Chapter Two").with_props(ParaProps {
            toc_target: Some(heading_id),
            ..ParaProps::default()
        });
        second.append_blocks(vec![Block::Paragraph(entry)]);

        let book = stitch(vec![doc(&["Chapter One"]), second]).expect("a book");
        let ids: Vec<u64> = book.paragraphs().map(|paragraph| paragraph.id.0).collect();
        let unique: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "ids stay unique: {ids:?}");

        let entry = book
            .paragraphs()
            .find(|paragraph| paragraph.props.toc_target.is_some())
            .expect("the entry");
        let target = entry.props.toc_target.expect("a target");
        assert_eq!(
            book.paragraph(target).map(|paragraph| paragraph.text()),
            Some("Chapter Two"),
            "the reference still points at its heading"
        );
    }

    #[test]
    fn list_numbering_does_not_continue_across_sources() {
        let list = |text: &str, group: u64| {
            let mut paragraph = Paragraph::new(BlockId(0), text.to_owned());
            paragraph = paragraph.with_props(ParaProps {
                list: Some(ListItem {
                    style: ListStyle::Numbered,
                    level: 0,
                    group,
                }),
                ..ParaProps::default()
            });
            paragraph
        };
        let first = Document::from_paragraphs(vec![list("One", 1)]);
        let second = Document::from_paragraphs(vec![list("Two", 1)]);
        let book = stitch(vec![first, second]).expect("a book");
        let groups: Vec<u64> = book
            .paragraphs()
            .filter_map(|paragraph| paragraph.props.list.as_ref().map(|list| list.group))
            .collect();
        assert_eq!(groups.len(), 2);
        assert_ne!(groups[0], groups[1], "the lists are separate");
        // Each list still starts at 1, because the numbering is per group.
        let labels = book.list_labels();
        let labels: Vec<&String> = labels.iter().flatten().collect();
        assert_eq!(labels, ["1.", "1."]);
    }

    #[test]
    fn a_style_is_added_once_by_name() {
        let first = doc(&["One"]);
        let mut second = doc(&["Two"]);
        let style = first.styles.paragraph[0].clone();
        second.styles.paragraph[0].name = style.name.clone();
        let before = first.styles.paragraph.len();
        let book = stitch(vec![first, second]).expect("a book");
        assert_eq!(
            book.styles.paragraph.len(),
            before,
            "a style with a known name is reused"
        );
        let mut third = doc(&["Three"]);
        third.styles.paragraph[0].name = "A Style of Its Own".to_owned();
        let book = stitch(vec![book, third]).expect("a book");
        assert!(
            book.styles
                .paragraph
                .iter()
                .any(|style| style.name == "A Style of Its Own")
        );
    }

    #[test]
    fn nothing_to_stitch_is_an_error() {
        assert_eq!(stitch(Vec::new()), Err(StitchError::Empty));
    }

    fn heading(text: &str) -> Paragraph {
        Paragraph::new(BlockId(0), text.to_owned()).with_props(ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        })
    }

    /// A document with a generated contents list (title, entries) and its chapters.
    fn book_with_contents(chapters: &[&str]) -> Document {
        let mut blocks = vec![Block::Paragraph(
            Paragraph::new(BlockId(0), "Contents").with_props(ParaProps {
                style: StyleId::TITLE,
                ..ParaProps::default()
            }),
        )];
        // Chapters first get ids 100.., so the entries can point at them.
        for (i, _) in chapters.iter().enumerate() {
            let entry = Paragraph::new(BlockId(0), chapters[i]).with_props(ParaProps {
                toc_target: Some(BlockId(100 + i as u64)),
                ..ParaProps::default()
            });
            blocks.push(Block::Paragraph(entry));
        }
        for (i, chapter) in chapters.iter().enumerate() {
            let mut p = heading(chapter);
            p.id = BlockId(100 + i as u64);
            blocks.push(Block::Paragraph(p));
            blocks.push(Block::Paragraph(Paragraph::new(
                BlockId(0),
                format!("Text of {chapter}"),
            )));
        }
        Document::from_blocks(blocks)
    }

    #[test]
    fn a_source_with_its_own_style_order_keeps_its_looks() {
        use libertum_model::ParagraphStyle;
        let first = doc(&["One"]);
        // The second document's sheet has a custom style where the book has "Title", and
        // its heading style somewhere else, as an imported file would.
        let mut second = doc(&["Verse line", "A heading", "Plain"]);
        let custom = ParagraphStyle {
            name: "Verse".to_owned(),
            base: Some(StyleId(0)),
            ..second.styles.paragraph[0].clone()
        };
        let heading_style = second.styles.paragraph[StyleId::HEADING_1.0 as usize].clone();
        second.styles.paragraph = vec![
            second.styles.paragraph[0].clone(), // Body
            custom,
            heading_style.clone(),
        ];
        let ids: Vec<BlockId> = second.paragraphs().map(|p| p.id).collect();
        for (id, style) in ids.iter().zip([1u32, 2, 0]) {
            let mut props = second.paragraph(*id).expect("p").props;
            props.style = StyleId(style);
            second
                .apply(libertum_model::Operation::SetParaProps { block: *id, props })
                .expect("set style");
        }
        let book = stitch(vec![first, second]).expect("a book");
        let name_of = |text: &str| {
            let p = book.paragraphs().find(|p| p.text() == text).expect(text);
            book.styles
                .get(p.props.style)
                .map(|s| s.name.clone())
                .expect("a style in the book's sheet")
        };
        assert_eq!(name_of("Verse line"), "Verse");
        assert_eq!(name_of("A heading"), heading_style.name);
        assert_eq!(name_of("Plain"), book.styles.paragraph[0].name);
        // The appended style's base points into the book's sheet.
        let verse = book
            .styles
            .paragraph
            .iter()
            .find(|s| s.name == "Verse")
            .expect("appended");
        assert_eq!(verse.base, Some(StyleId::BODY));
    }

    #[test]
    fn the_book_keeps_one_contents_list_covering_every_chapter() {
        let first = book_with_contents(&["Alpha", "Beta"]);
        let second = book_with_contents(&["Gamma"]);
        let third = doc(&["Loose text"]);
        let mut fourth = doc(&["x"]);
        fourth.append_blocks(vec![Block::Paragraph(heading("Delta"))]);
        let book = stitch(vec![first, second, third, fourth]).expect("a book");

        let all = texts(&book);
        assert_eq!(
            all.iter().filter(|t| t.as_str() == "Contents").count(),
            1,
            "one title: {all:?}"
        );
        let entries: Vec<(&str, BlockId)> = book
            .paragraphs()
            .filter_map(|p| p.props.toc_target.map(|target| (p.text(), target)))
            .collect();
        let names: Vec<&str> = entries.iter().map(|e| e.0).collect();
        assert_eq!(names, ["Alpha", "Beta", "Gamma", "Delta"]);
        for (text, target) in &entries {
            let heading = book.paragraph(*target).expect("target exists");
            assert_eq!(heading.text(), *text);
            assert_eq!(heading.props.outline_level, Some(1));
        }
        // The entries sit together, right after the title.
        let title = all.iter().position(|t| t == "Contents").expect("title");
        assert_eq!(
            &all[title + 1..title + 5],
            ["Alpha", "Beta", "Gamma", "Delta"]
        );
        // Ids are still unique and sections match their starts.
        let ids: Vec<u64> = book.paragraphs().map(|p| p.id.0).collect();
        let unique: HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len());
        let starts = book.paragraphs().filter(|p| p.props.section_start).count();
        assert_eq!(book.sections.len(), starts + 1);
        assert_eq!(starts, 3, "each later source still opens a section");
    }

    #[test]
    fn a_contents_list_in_a_later_source_is_kept_when_the_first_has_none() {
        let book = stitch(vec![doc(&["Preface"]), book_with_contents(&["Only"])]).expect("book");
        let entries: Vec<&str> = book
            .paragraphs()
            .filter(|p| p.props.toc_target.is_some())
            .map(|p| p.text())
            .collect();
        assert_eq!(entries, ["Only"]);
    }

    #[test]
    fn clashing_bookmarks_are_renamed_and_their_references_follow() {
        use libertum_model::{Bookmark, CrossRef, CrossRefKind, Operation};
        let marked = |text: &str| {
            let mut d = doc(&[text, "See  here"]);
            let (a, b) = (
                d.paragraph_at(0).expect("a").id,
                d.paragraph_at(1).expect("b").id,
            );
            d.apply(Operation::SetBookmarks {
                block: a,
                bookmarks: vec![Bookmark {
                    name: "Intro".into(),
                    offset: 0,
                }],
            })
            .expect("bookmark");
            d.apply(Operation::SetCrossRefs {
                block: b,
                cross_refs: vec![CrossRef {
                    range: 0..3,
                    target: "intro".into(),
                    kind: CrossRefKind::Text,
                }],
            })
            .expect("reference");
            d
        };
        let book = stitch(vec![marked("First"), marked("Second"), marked("Third")]).expect("book");
        let names: Vec<String> = book
            .paragraphs()
            .flat_map(|p| p.bookmarks().iter().map(|b| b.name.clone()))
            .collect();
        assert_eq!(names, ["Intro", "Intro_2", "Intro_3"]);
        let targets: Vec<String> = book
            .paragraphs()
            .flat_map(|p| p.cross_refs().iter().map(|r| r.target.clone()))
            .collect();
        assert_eq!(targets, ["intro", "Intro_2", "Intro_3"]);
        // Each reference resolves to the bookmark of its own source.
        for (target, text) in targets.iter().zip(["First", "Second", "Third"]) {
            let holder = book
                .paragraphs()
                .find(|p| p.bookmark(target).is_some())
                .expect("bookmark");
            assert_eq!(holder.text(), text);
        }
    }

    #[test]
    fn sections_always_match_the_section_starts() {
        // A source whose section list is shorter than its section starts (seen in imports).
        let mut odd = doc(&["A", "B", "C"]);
        let id = odd.paragraph_at(2).expect("c").id;
        let mut props = odd.paragraph(id).expect("c").props;
        props.section_start = true;
        odd.apply(libertum_model::Operation::SetParaProps { block: id, props })
            .expect("props");
        assert_eq!(odd.sections.len(), 1);
        let book = stitch(vec![doc(&["One"]), odd.clone(), odd]).expect("book");
        let starts = book.paragraphs().filter(|p| p.props.section_start).count();
        assert_eq!(starts, 4);
        assert_eq!(book.sections.len(), 5);
    }

    #[test]
    fn large_documents_stitch_quickly_and_completely() {
        // Three books of 6,000 paragraphs each, with chapters, lists and a contents list.
        let source = |tag: &str| {
            let chapters: Vec<String> = (0..40).map(|c| format!("{tag} chapter {c}")).collect();
            let chapter_refs: Vec<&str> = chapters.iter().map(String::as_str).collect();
            let mut d = book_with_contents(&chapter_refs);
            let filler: Vec<Block> = (0..6_000)
                .map(|i| {
                    Block::Paragraph(Paragraph::new(
                        BlockId(0),
                        format!("{tag} paragraph {i} with some words in it."),
                    ))
                })
                .collect();
            let mut blocks: Vec<Block> = d.body().iter().cloned().collect();
            blocks.extend(filler);
            let rebuilt = Document::from_blocks(blocks);
            // `from_blocks` keeps ids that are unique, so the entries still point at the
            // chapters; keep the sheet too.
            d = rebuilt;
            d
        };
        let sources = vec![source("A"), source("B"), source("C")];
        let expected: usize = sources.iter().map(Document::paragraph_count).sum();
        let words: usize = sources.iter().map(Document::word_count).sum();
        let started = std::time::Instant::now();
        let book = stitch(sources).expect("book");
        let elapsed = started.elapsed();
        // Two contents lists (title + 40 entries each) are dropped; the kept one grows from
        // 40 to 120 entries.
        assert_eq!(book.paragraph_count(), expected - 2 * 41 + 80);
        assert!(book.word_count() > words - 500 && book.word_count() < words + 500);
        let entries = book
            .paragraphs()
            .filter(|p| p.props.toc_target.is_some())
            .count();
        assert_eq!(entries, 120);
        let ids: HashSet<u64> = book.paragraphs().map(|p| p.id.0).collect();
        assert_eq!(ids.len(), book.paragraph_count(), "ids are unique");
        assert!(book.next_free_id() > ids.iter().copied().max().unwrap_or(0));
        // Every paragraph is findable through the index the editor uses.
        for id in book.paragraph_ids().step_by(997) {
            assert!(book.paragraph(id).is_some());
        }
        // Generous for a debug build; the old per-entry re-indexing took far longer.
        assert!(elapsed.as_secs_f32() < 5.0, "stitch took {elapsed:?}");
    }
}
