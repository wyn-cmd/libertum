//! Plain text, for the `.txt` export.

use libertum_model::{Block, Document, NoteKind, Paragraph, Table};

use crate::{Chapter, chapter_blocks};

/// The whole document as text: paragraphs separated by blank lines, table rows as tab-
/// separated cells, and the footnotes and endnotes collected at the end.
pub fn document_text(document: &Document) -> String {
    let blocks: Vec<&Block> = document.body().iter().collect();
    let mut out = text_blocks(&blocks);
    let notes = notes_text(document);
    if !notes.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&notes);
    }
    out
}

/// The text of one chapter, heading included.
pub fn chapter_text(document: &Document, chapter: &Chapter) -> String {
    let blocks = chapter_blocks(document, chapter);
    text_blocks(&blocks)
}

fn text_blocks(blocks: &[&Block]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => {
                if paragraph.props.toc_target.is_some() {
                    continue;
                }
                parts.push(paragraph.text().to_owned());
            }
            Block::Table(table) => parts.push(table_text(table)),
        }
    }
    parts.join("\n\n")
}

fn table_text(table: &Table) -> String {
    let mut rows: Vec<String> = Vec::new();
    for row in &table.rows {
        let cells: Vec<String> = row
            .cells
            .iter()
            .filter(|cell| !cell.merged_into_above)
            .map(|cell| {
                cell.paragraphs
                    .iter()
                    .map(Paragraph::text)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        rows.push(cells.join("\t"));
    }
    rows.join("\n")
}

/// Footnotes and endnotes in document order, numbered. Their reference marks stay in the
/// paragraph text; this section gives the reader the note bodies.
fn notes_text(document: &Document) -> String {
    let mut out = String::new();
    let mut number = 0usize;
    for paragraph in document.paragraphs() {
        for note in paragraph.notes() {
            number += 1;
            let kind = match note.kind {
                NoteKind::Footnote => "Footnote",
                NoteKind::Endnote => "Endnote",
            };
            out.push_str(&format!("{number}. {kind}: {}\n", note.text));
        }
    }
    out.trim_end().to_owned()
}
