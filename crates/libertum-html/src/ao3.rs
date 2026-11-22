//! Works downloaded from AO3 (Archive of Our Own) as HTML or EPUB.
//!
//! A download wraps the story in page furniture. What is kept, as the maintainer decided on
//! 2026-11-22: chapter headings, chapter text, and the author's summaries and notes (as
//! block quotes under a bold label such as "Summary", so they stay visibly apart from the
//! story). What is dropped: the preface's metadata (the archive message, title, byline,
//! tags and statistics) and the afterword's links and messages. The work's end notes, which
//! sit in the afterword, are the author's notes and are kept.
//!
//! The structure followed (from AO3's download templates as remembered, not from a live
//! file; check against a real download before relying on details):
//!
//! ```text
//! div#preface   p.message, div.meta (dl.tags, h1, div.byline,
//!               p "Summary" + blockquote.userstuff, p "Notes" + blockquote.userstuff)
//! div#chapters  per chapter: div.meta.group (h2.heading, p label + blockquote.userstuff ...),
//!               div.userstuff (the text), div.meta.group (end notes)
//!               a one-chapter work has its text directly in div#chapters
//! div#afterword div#endnotes (p label + blockquote.userstuff), p.message, links
//! ```

use libertum_model::{BOLD, CharFormat, ParaProps, StyleId};

use crate::dom::{Dom, NodeId};
use crate::read::{Ctx, Reader};

/// True when the tree is an AO3 download: a `#chapters` element alongside `#preface` or
/// `#afterword`.
pub fn is_download(dom: &Dom) -> bool {
    let has = |id: &str| {
        dom.find(Dom::ROOT, &|d, n| d.attr(n, "id") == Some(id))
            .is_some()
    };
    has("chapters") && (has("preface") || has("afterword"))
}

/// The `#preface`, `#chapters` and `#afterword` parts in document order. An EPUB joins
/// several files, so each may occur more than once.
fn parts(dom: &Dom) -> Vec<(NodeId, String)> {
    let mut out = Vec::new();
    let mut stack = vec![Dom::ROOT];
    while let Some(at) = stack.pop() {
        if let Some(id) = dom.attr(at, "id")
            && matches!(id, "preface" | "chapters" | "afterword")
        {
            out.push((at, id.to_owned()));
            continue;
        }
        stack.extend(dom.children(at).iter().rev());
    }
    out
}

/// Reads an AO3 download with the rules above.
pub fn read(reader: &mut Reader<'_>) {
    let dom = reader.dom;
    for (part, id) in parts(dom) {
        match id.as_str() {
            "preface" => labelled_quotes(reader, part),
            "chapters" => {
                let ctx = Ctx::default();
                for &child in dom.children(part) {
                    if !dom.is_element(child) {
                        reader.walk(child, &ctx);
                        continue;
                    }
                    if dom.has_class(child, "meta") {
                        chapter_meta(reader, child);
                    } else if dom.name(child) == "h2" && dom.has_class(child, "toc-heading") {
                        // A contents heading, not chapter text.
                    } else {
                        reader.walk(child, &ctx);
                    }
                }
                reader.flush();
            }
            _ => {
                // The afterword: only the work's end notes are the author's.
                if let Some(endnotes) = dom.find(part, &|d, n| d.attr(n, "id") == Some("endnotes"))
                {
                    labelled_quotes(reader, endnotes);
                }
            }
        }
    }
}

/// A chapter's meta block: its heading becomes Heading 1, its summary and notes quotes.
fn chapter_meta(reader: &mut Reader<'_>, meta: NodeId) {
    let dom = reader.dom;
    if let Some(heading) = dom.find(meta, &|d, n| {
        matches!(d.name(n), "h2" | "h3") && d.has_class(n, "heading")
    }) {
        let ctx = Ctx {
            style: Some(StyleId::HEADING_1),
            outline_level: Some(1),
            ..Ctx::default()
        };
        reader.flush();
        reader.walk_children(heading, &ctx);
        // Make sure the heading is a paragraph of its own even when its text was empty.
        reader.flush();
    }
    labelled_quotes(reader, meta);
}

/// Every `blockquote.userstuff` below `root`, each under its label (the text of the element
/// just before it, such as "Summary" or "Chapter End Notes").
fn labelled_quotes(reader: &mut Reader<'_>, root: NodeId) {
    let dom = reader.dom;
    let quotes: Vec<NodeId> = dom
        .find_all(root, "blockquote")
        .into_iter()
        .filter(|&q| dom.has_class(q, "userstuff"))
        .collect();
    for quote in quotes {
        let label = previous_element(dom, quote)
            .filter(|&p| matches!(dom.name(p), "p" | "h3" | "h4" | "b" | "strong"))
            .map(|p| dom.text(p).split_whitespace().collect::<Vec<_>>().join(" "))
            .map(|t| t.trim_end_matches(':').to_owned())
            .filter(|t| !t.is_empty() && t.chars().count() <= 40);
        if let Some(label) = label {
            let props = ParaProps {
                style: StyleId::QUOTE,
                ..ParaProps::default()
            };
            let format = CharFormat {
                weight: Some(BOLD),
                ..CharFormat::default()
            };
            reader.push_paragraph(&label, props, format);
        }
        let ctx = Ctx::default();
        reader.walk(quote, &ctx);
        reader.flush();
    }
}

fn previous_element(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let parent = dom.nodes[id].parent?;
    let siblings = dom.children(parent);
    let at = siblings.iter().position(|&s| s == id)?;
    siblings[..at]
        .iter()
        .rev()
        .copied()
        .find(|&s| dom.is_element(s))
}
