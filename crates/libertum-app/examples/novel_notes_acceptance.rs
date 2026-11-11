//! Generates and measures the Phase 6 large-novel acceptance shape: about 300 pages with
//! 200 footnotes, a 10-entry table of contents, 20 inline images, ten chapter bookmarks with
//! a text and a page cross-reference, and a banded table whose header repeats and whose
//! rows split across pages.
//!
//! It reports full pagination, one incremental edit, and save/open for ODT, DOCX and RTF,
//! then checks that every mapped feature came back from each format.
//!
//! `cargo run --release -p libertum-app --example novel_notes_acceptance`

use std::time::Instant;

use libertum_docx::Docx;
use libertum_formats_common::media::sample_png;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
use libertum_model::{
    Block, BlockId, Bookmark, CrossRef, CrossRefKind, Document, InlineImage, Note, NoteKind,
    ParaProps, Paragraph, StyleId, Table, TableCell, TableRow, TableStyle,
};
use libertum_odt::Odt;
use libertum_rtf::Rtf;

const PARAGRAPHS: usize = 12_000;
const CHAPTERS: usize = 10;
const NOTES: usize = 200;
/// One image every `IMAGE_EVERY` paragraphs (`IMAGE_EVERY * IMAGES` stays below
/// `PARAGRAPHS`).
const IMAGE_EVERY: usize = 600;
const IMAGES: usize = 20;

/// Columns one cell covers, and whether it continues a vertical merge.
type CellShape = (u8, bool);

fn note_count(document: &Document) -> usize {
    document
        .paragraphs()
        .map(|paragraph| paragraph.notes().len())
        .sum()
}

fn image_count(document: &Document) -> usize {
    document
        .paragraphs()
        .map(|paragraph| paragraph.images().len())
        .sum()
}

fn bookmark_count(document: &Document) -> usize {
    document
        .paragraphs()
        .map(|paragraph| paragraph.bookmarks().len())
        .sum()
}

fn cross_ref_count(document: &Document) -> usize {
    document
        .paragraphs()
        .map(|paragraph| paragraph.cross_refs().len())
        .sum()
}

/// The name of the style of the document's first table, and its cell layout per row: the
/// columns each cell covers and whether it continues a vertical merge.
fn table_shape(document: &Document) -> Option<(String, Vec<Vec<CellShape>>)> {
    let table = document.body().iter().find_map(|block| block.as_table())?;
    let name = table
        .style
        .as_ref()
        .map(|style| style.name.clone())
        .unwrap_or_default();
    let rows = table
        .rows
        .iter()
        .map(|row| {
            row.cells
                .iter()
                .map(|cell| (cell.col_span, cell.merged_into_above))
                .collect()
        })
        .collect();
    Some((name, rows))
}

fn cell(text: &str) -> TableCell {
    TableCell::new(Paragraph::new(BlockId(0), text.to_owned()))
}

/// A two-column banded table with a header row, a row that spans both columns, a vertical
/// merge, and one very tall row (kept together off) that must split across pages.
fn table(id: BlockId) -> Result<Table, Box<dyn std::error::Error>> {
    let (style, borders) = TableStyle::builtin("Banded Rows").ok_or("no Banded Rows style")?;
    let long = "A long scene note that fills several lines so this row cannot stay on one \
                page and has to continue on the next. ";
    let mut rows = Vec::new();
    rows.push(TableRow {
        cells: vec![cell("Scene"), cell("Notes")],
        min_height_pt: 0.0,
        keep_together: true,
    });
    for row in 1..24usize {
        let mut cells = if row == 5 {
            // One cell across both columns (a horizontal merge).
            let mut merged = cell("A note across both columns");
            merged.col_span = 2;
            vec![merged]
        } else {
            vec![cell(&format!("Row {row}")), cell(&format!("{}", row * 10))]
        };
        if row == 2 {
            cells[1] = cell(&long.repeat(40));
        }
        rows.push(TableRow {
            cells,
            min_height_pt: 0.0,
            keep_together: row != 2,
        });
    }
    // Rows 10 and 11 are one cell: row 11 is its continuation.
    rows[11].cells[0].merged_into_above = true;
    rows[11].cells[0].paragraphs = vec![Paragraph::new(BlockId(0), "")];
    Ok(Table {
        id,
        widths_pt: vec![200.0, 200.0],
        rows,
        header_rows: 1,
        borders,
        style: Some(style),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut blocks = Vec::new();
    let mut next_id = 1u64;
    let mut alloc = || {
        let id = BlockId(next_id);
        next_id += 1;
        id
    };

    // 10 chapter headings, each with a TOC entry before it and a bookmark on its first
    // character.
    let chapter_ids: Vec<BlockId> = (1..=CHAPTERS).map(|_| alloc()).collect();
    for (i, target) in chapter_ids.iter().enumerate() {
        let mut entry = Paragraph::new(alloc(), format!("Chapter {}", i + 1));
        entry = entry.with_props(ParaProps {
            toc_target: Some(*target),
            ..ParaProps::default()
        });
        blocks.push(Block::Paragraph(entry));
    }

    // A cross-reference to the first chapter: its text and its page number.
    let mut reference = Paragraph::new(
        alloc(),
        "Chapter 1 begins on page 1, in the first chapter.".to_owned(),
    );
    reference.set_cross_refs(vec![
        CrossRef {
            range: 0..9,
            target: "chapter_1".to_owned(),
            kind: CrossRefKind::Text,
        },
        // The 1 after "page ".
        CrossRef {
            range: 25..26,
            target: "chapter_1".to_owned(),
            kind: CrossRefKind::Page,
        },
    ]);
    blocks.push(Block::Paragraph(reference));

    let chapter_starts: Vec<usize> = (0..CHAPTERS).map(|i| i * (PARAGRAPHS / CHAPTERS)).collect();
    let mut footnote_index = 0usize;
    let mut image_index = 0usize;
    let mut image_media = Vec::new();
    for i in 0..PARAGRAPHS {
        let id = if let Some(position) = chapter_starts.iter().position(|start| *start == i) {
            chapter_ids[position]
        } else {
            alloc()
        };
        let text = format!(
            "Paragraph {i}. The quick brown fox jumps over the lazy dog while the novel continues."
        );
        let mut paragraph = Paragraph::new(id, text);
        if let Some(position) = chapter_starts.iter().position(|start| *start == i) {
            paragraph = paragraph.with_props(ParaProps {
                style: StyleId::HEADING_1,
                outline_level: Some(1),
                ..ParaProps::default()
            });
            paragraph.set_bookmarks(vec![Bookmark {
                name: format!("chapter_{}", position + 1),
                offset: 0,
            }]);
        }
        if i % 60 == 0 && footnote_index < NOTES {
            paragraph.set_notes(vec![Note {
                kind: NoteKind::Footnote,
                offset: 0,
                text: format!(
                    "Footnote {}. A short note at the bottom of the page.",
                    footnote_index + 1
                ),
            }]);
            footnote_index += 1;
        }
        if i % IMAGE_EVERY == IMAGE_EVERY / 2 && image_index < IMAGES {
            let media = sample_png(320, 240)?;
            let fitted = InlineImage::fitted(0, media.clone(), 420.0);
            let mut image = fitted;
            image.alt = format!("Figure {}: a checker pattern", image_index + 1);
            image.name = format!("checker-{}.png", image_index + 1);
            paragraph.set_images(vec![image]);
            image_media.push(media);
            image_index += 1;
        }
        blocks.push(Block::Paragraph(paragraph));
    }
    // The table sits halfway through the body, where its tall row has to continue on the
    // next page.
    let body = blocks.split_off(CHAPTERS + 1);
    let (head, tail) = body.split_at(body.len() / 2);
    blocks.extend_from_slice(head);
    blocks.push(Block::Table(table(alloc())?));
    blocks.extend_from_slice(tail);

    let document = Document::from_blocks(blocks);
    println!(
        "blocks={} notes={} images={} bookmarks={} cross_refs={}",
        document.body().len(),
        note_count(&document),
        image_count(&document),
        bookmark_count(&document),
        cross_ref_count(&document)
    );

    let start = Instant::now();
    let mut layout =
        LayoutState::from_document(&document, PageGeometry::a4(), LayoutOptions::default());
    layout.paginate_all();
    println!(
        "pages={} pagination_ms={}",
        layout.page_count(),
        start.elapsed().as_millis()
    );

    // How the table landed: the pages its rows cover, and whether any row was split.
    let mut row_pages: std::collections::HashMap<(u64, usize), usize> =
        std::collections::HashMap::new();
    for page in layout.pages() {
        for draw in page.tables.iter().filter(|draw| !draw.header) {
            *row_pages.entry((draw.table, draw.row)).or_default() += 1;
        }
    }
    let split_rows = row_pages.values().filter(|count| **count > 1).count();
    let table_pages: std::collections::HashSet<usize> = layout
        .pages()
        .iter()
        .enumerate()
        .filter(|(_, page)| !page.tables.is_empty())
        .map(|(index, _)| index)
        .collect();
    println!(
        "table_pages={} split_rows={}",
        table_pages.len(),
        split_rows
    );

    // An edit halfway down the document re-paginates from that page only.
    let mid = layout.paragraph_count() / 2;
    let edit = Instant::now();
    let stats = layout.edit_paragraph(mid, |paragraph| paragraph.text.push('x'));
    println!(
        "edit_ms={:.3} pages_rebuilt={}",
        edit.elapsed().as_secs_f64() * 1_000.0,
        stats.map_or(0, |stats| stats.pages_rebuilt)
    );

    std::fs::create_dir_all("target")?;
    for path in [
        "target/novel-300.odt",
        "target/novel-300.docx",
        "target/novel-300.rtf",
    ] {
        let save = Instant::now();
        if path.ends_with(".odt") {
            Odt::new(document.clone()).save(path)?;
        } else if path.ends_with(".docx") {
            Docx::new(document.clone()).save(path)?;
        } else {
            Rtf::new(document.clone()).save(path)?;
        }
        let saved_ms = save.elapsed().as_millis();
        let open = Instant::now();
        let reopened = if path.ends_with(".odt") {
            Odt::open(path)?.document().clone()
        } else if path.ends_with(".docx") {
            Docx::open(path)?.document().clone()
        } else {
            Rtf::open(path)?.document().clone()
        };
        let open_ms = open.elapsed().as_millis();
        println!(
            "{path}: save_ms={saved_ms} open_ms={open_ms} notes={} images={} bookmarks={} \
             cross_refs={} table={:?}",
            note_count(&reopened),
            image_count(&reopened),
            bookmark_count(&reopened),
            cross_ref_count(&reopened),
            table_shape(&reopened).map(|(name, rows)| (name, rows.len()))
        );

        // Every mapped feature comes back.
        if note_count(&reopened) != NOTES {
            return Err(format!(
                "{path}: {} of {NOTES} notes came back",
                note_count(&reopened)
            )
            .into());
        }
        if image_count(&reopened) != IMAGES {
            return Err(format!(
                "{path}: {} of {IMAGES} images came back",
                image_count(&reopened)
            )
            .into());
        }
        if bookmark_count(&reopened) != CHAPTERS {
            return Err(format!(
                "{path}: {} of {CHAPTERS} bookmarks came back",
                bookmark_count(&reopened)
            )
            .into());
        }
        if cross_ref_count(&reopened) != 2 {
            return Err(format!(
                "{path}: {} of 2 cross-references came back",
                cross_ref_count(&reopened)
            )
            .into());
        }
        let Some((style, shape)) = table_shape(&document) else {
            return Err("the generated document has no table".into());
        };
        let Some((read_style, read_shape)) = table_shape(&reopened) else {
            return Err(format!("{path}: the table did not come back").into());
        };
        if read_style != style || read_shape != shape {
            return Err(format!(
                "{path}: the table came back as {read_style:?} {read_shape:?}, not {style:?} {shape:?}"
            )
            .into());
        }
        // The image bytes are the same ones, and the displayed width survives.
        let first_sent = image_media.first();
        let first_read = reopened
            .paragraphs()
            .find_map(|paragraph| paragraph.images().first());
        match (first_sent, first_read) {
            (Some(sent), Some(read)) => {
                if sent.bytes() != read.media.bytes() || (read.width_pt - 240.0).abs() > 1.0 {
                    return Err(format!("{path}: the first image came back differently").into());
                }
            }
            _ => return Err(format!("{path}: the first image is missing").into()),
        }
    }
    println!("round_trip=ok");
    Ok(())
}
