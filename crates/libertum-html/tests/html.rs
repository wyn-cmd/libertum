//! HTML import and export: a fixture per construct, round trips, AO3 downloads and EPUB.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use libertum_html::{Html, HtmlFlavour, LossKind, import_epub_bytes, loss_report, parse_fragment};
use libertum_model::{
    Align, BOLD, Block, Bookmark, CharFormat, Document, InlineImage, LineSpacing, ListItem,
    ListStyle, Note, NoteKind, ParaFormat, ParaProps, Paragraph, Run, StyleId, VerticalAlign,
};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/html")
}

fn open(name: &str) -> Html {
    Html::open(fixtures().join(name)).expect("fixture opens")
}

fn texts(doc: &Document) -> Vec<String> {
    doc.paragraphs().map(|p| p.text().to_owned()).collect()
}

fn para<'a>(doc: &'a Document, text: &str) -> &'a Paragraph {
    doc.paragraphs()
        .find(|p| p.text() == text)
        .unwrap_or_else(|| panic!("no paragraph {text:?} in {:?}", texts(doc)))
}

/// Resolved character formatting of the first occurrence of `needle` in paragraph `p`.
fn char_at(doc: &Document, p: &Paragraph, needle: &str) -> libertum_model::ResolvedChar {
    let at = p.text().find(needle).expect("needle");
    doc.styles
        .resolve_char(p.props.style)
        .apply(&p.format_of(at))
}

#[test]
fn whitespace_collapses_except_in_pre() {
    let doc = open("whitespace.html").into_document();
    assert_eq!(
        texts(&doc),
        vec![
            "One paragraph, with collapsed spaces.",
            "Second bold italic end.",
            "  kept\n    as is"
        ]
    );
    let p = para(&doc, "Second bold italic end.");
    assert!(char_at(&doc, p, "bold").is_bold());
    assert!(
        !char_at(&doc, p, " italic").is_bold(),
        "the space is not bold"
    );
    assert!(char_at(&doc, p, "italic").italic);
    let pre = para(&doc, "  kept\n    as is");
    assert_eq!(&*char_at(&doc, pre, "kept").family, "Courier New");
}

#[test]
fn emphasis_tags_become_formatting() {
    let doc = open("emphasis.html").into_document();
    let p = doc.paragraphs().next().expect("one paragraph");
    assert!(char_at(&doc, p, "strong").is_bold());
    assert!(char_at(&doc, p, "b ").is_bold());
    assert!(char_at(&doc, p, "em").italic);
    assert!(char_at(&doc, p, "i ").italic);
    assert!(char_at(&doc, p, "u ").underline);
    for s in ["s ", "del", "strike"] {
        assert!(char_at(&doc, p, s).strikethrough, "{s}");
    }
    assert_eq!(char_at(&doc, p, "2 ").vertical, VerticalAlign::Superscript);
    assert_eq!(char_at(&doc, p, "2O").vertical, VerticalAlign::Subscript);
    assert_eq!(&*char_at(&doc, p, "code").family, "Courier New");
}

#[test]
fn inline_css_sets_character_and_paragraph_formatting() {
    let doc = open("inline-css.html").into_document();
    let p = doc.paragraphs().next().expect("first");
    let r = doc.styles.resolve_props(&p.props);
    assert_eq!(r.align, Align::Centre);
    assert_eq!(r.indent_left_pt, 18.0);
    assert_eq!(r.indent_first_pt, 12.0);
    assert_eq!(r.space_before_pt, 3.0);
    assert_eq!(r.space_after_pt, 9.0);
    assert_eq!(r.line_spacing, LineSpacing::Multiple(1.5));
    assert_eq!(char_at(&doc, p, "red heavy").colour, [255, 0, 0, 255]);
    assert_eq!(char_at(&doc, p, "heavy").weight, 700);
    assert!(char_at(&doc, p, "slanted").italic);
    let both = char_at(&doc, p, "both");
    assert!(both.underline && both.strikethrough);
    let georgia = char_at(&doc, p, "Georgia");
    assert_eq!((&*georgia.family, georgia.size_pt), ("Georgia", 14.0));
    assert_eq!(
        char_at(&doc, p, "marked").highlight,
        Some([255, 255, 0, 255])
    );
    let old = char_at(&doc, p, "old");
    assert_eq!((&*old.family, old.colour), ("Arial", [0, 0, 255, 255]));
    let right = para(&doc, "Right from the div");
    assert_eq!(doc.styles.resolve_props(&right.props).align, Align::End);
}

#[test]
fn headings_get_levels_and_styles() {
    let doc = open("headings.html").into_document();
    let levels: Vec<Option<u8>> = doc.paragraphs().map(|p| p.props.outline_level).collect();
    assert_eq!(levels, (1..=6).map(Some).collect::<Vec<_>>());
    assert_eq!(para(&doc, "One").props.style, StyleId::HEADING_1);
    assert_eq!(para(&doc, "Six").props.style, StyleId::HEADING_4);
}

#[test]
fn block_quotes_nest() {
    let doc = open("blockquote.html").into_document();
    let one = para(&doc, "Quoted one.");
    let two = para(&doc, "Quoted two.");
    assert_eq!(one.props.style, StyleId::QUOTE);
    assert_eq!(two.props.style, StyleId::QUOTE);
    assert!(
        doc.styles.resolve_props(&two.props).indent_left_pt
            > doc.styles.resolve_props(&one.props).indent_left_pt
    );
}

#[test]
fn lists_nest_and_keep_their_kind() {
    let doc = open("lists.html").into_document();
    let item = |t: &str| para(&doc, t).props.list.expect("list item");
    assert_eq!(item("Apple").style, ListStyle::Bullet);
    assert_eq!(item("Nested").level, 1);
    assert_eq!(item("Plum").level, 0);
    assert_eq!(item("Apple").group, item("Plum").group);
    assert_eq!(item("First").style, ListStyle::Numbered);
    assert_ne!(item("First").group, item("Apple").group);
    assert_eq!(item("Lettered").style, ListStyle::Lettered);
}

#[test]
fn rules_and_line_breaks() {
    let doc = open("rule-break.html").into_document();
    assert_eq!(texts(&doc), vec!["Before", "", "Line one\nLine two"]);
    let rule = doc.paragraphs().nth(1).expect("rule");
    assert_eq!(
        doc.styles.get(rule.props.style).map(|s| s.name.as_str()),
        Some("Horizontal Rule")
    );
}

#[test]
fn tables_keep_spans_headers_and_fills() {
    let doc = open("table.html").into_document();
    let table = doc
        .body()
        .iter()
        .find_map(Block::as_table)
        .expect("a table");
    assert_eq!(table.rows.len(), 4);
    assert_eq!(table.header_rows, 1);
    assert_eq!(table.columns(), 2);
    assert!(table.rows[2].cells[0].merged_into_above, "row span");
    assert_eq!(table.rows[3].cells[0].col_span, 2);
    assert_eq!(table.rows[1].cells[1].fill, Some([204, 255, 204, 255]));
    assert!(table.borders.is_visible());
    let name = &table.rows[0].cells[0].paragraphs[0];
    assert!(char_at(&doc, name, "Name").is_bold(), "th is bold");
}

#[test]
fn embedded_and_local_images_load_but_remote_ones_never_do() {
    let doc = open("image-data.html").into_document();
    let p = doc.paragraphs().next().expect("p");
    assert_eq!(p.text(), "Before  after");
    let image = &p.images()[0];
    assert_eq!((image.media.width_px(), image.media.height_px()), (4, 2));
    assert_eq!((image.width_pt, image.height_pt), (30.0, 15.0));
    assert_eq!(image.alt, "red");

    let doc = open("image-local.html").into_document();
    let image = &doc.paragraphs().next().expect("p").images()[0];
    assert_eq!((image.width_pt, image.height_pt), (30.0, 15.0));
    assert_eq!(image.name, "red-4x2.png");

    let html = open("image-remote.html");
    assert_eq!(texts(html.document()), vec!["See a remote picture here."]);
    assert!(
        html.warnings()[0].contains("not downloaded"),
        "{:?}",
        html.warnings()
    );
}

#[test]
fn page_colour_defaults_and_page_size_come_from_the_style_block() {
    let doc = open("page-colour.html").into_document();
    let section = &doc.sections[0];
    assert_eq!(section.page_colour, Some([0xe0, 0xe0, 0xe0, 255]));
    assert_eq!(section.page.paper, libertum_model::PaperSize::A5);
    assert_eq!(section.page.margins.left, 36.0);
    assert_eq!(&*doc.styles.defaults.family, "Georgia");
    assert_eq!(doc.styles.defaults.size_pt, 12.0);
    assert_eq!(doc.styles.defaults.colour, [0x20, 0x20, 0x20, 255]);
}

#[test]
fn footnotes_and_bookmarks_attach_to_their_paragraph() {
    let doc = open("footnotes.html").into_document();
    assert_eq!(texts(&doc), vec!["Text and more."]);
    let p = doc.paragraphs().next().expect("p");
    assert_eq!(p.notes().len(), 1);
    assert_eq!(p.notes()[0].offset, 4);
    assert_eq!(p.notes()[0].text, "The note.");
    assert_eq!(p.bookmarks()[0].name, "mark");
    assert_eq!(p.bookmarks()[0].offset, "Text and more".len());
}

#[test]
fn malformed_html_still_reads() {
    let html = open("malformed.html");
    let all = texts(html.document()).join("|");
    assert!(all.contains("Unclosed bold both"), "{all}");
    assert!(all.contains("stray item"));
    assert!(all.contains("unknown tag text"));
    assert!(!all.contains("alert"));
    assert!(all.contains("&\u{a0}A&bogus;"), "{all}");
}

#[test]
fn ao3_download_keeps_story_and_notes_but_not_metadata() {
    let html = open("ao3-download.html");
    assert!(html.is_ao3());
    let doc = html.document();
    assert_eq!(
        texts(doc),
        vec![
            "Summary",
            "A work summary.",
            "Notes",
            "Work notes at the start.",
            "Chapter 1: Beginning",
            "Chapter Summary",
            "First chapter summary.",
            "The first chapter's text.",
            "Second paragraph.",
            "Chapter End Notes",
            "End of chapter one notes.",
            "Chapter 2: Ending",
            "The second chapter's text.",
            "End Notes",
            "Thanks for reading.",
        ]
    );
    assert_eq!(
        para(doc, "Chapter 1: Beginning").props.outline_level,
        Some(1)
    );
    for quoted in ["Summary", "A work summary.", "End of chapter one notes."] {
        assert_eq!(para(doc, quoted).props.style, StyleId::QUOTE, "{quoted}");
    }
    assert!(char_at(doc, para(doc, "Notes"), "Notes").is_bold());
    assert_eq!(para(doc, "Second paragraph.").props.style, StyleId::BODY);
    let all = texts(doc).join(" ");
    for gone in [
        "Archive of Our Own",
        "General Audiences",
        "Someone",
        "comment",
        "The Test Work",
    ] {
        assert!(!all.contains(gone), "{gone} should be dropped");
    }
}

#[test]
fn ao3_single_chapter_work() {
    let html = open("ao3-single-chapter.html");
    assert!(html.is_ao3());
    assert_eq!(texts(html.document()), vec!["Only chapter text here."]);
}

#[test]
fn a_plain_page_is_not_taken_for_ao3() {
    assert!(!open("inline-css.html").is_ao3());
}

#[test]
fn a_pasted_fragment_round_trips_through_the_ao3_export() {
    let source = std::fs::read_to_string(fixtures().join("fragment-ao3.html")).expect("read");
    let doc = parse_fragment(&source);
    assert_eq!(
        texts(&doc),
        vec![
            "A pasted chapter with bold.",
            "A quote.",
            "item",
            "",
            "",
            "After a blank."
        ]
    );
    let chapter = libertum_export::chapters(&doc, Default::default())
        .into_iter()
        .next()
        .expect("whole document");
    let fragment = libertum_export::ao3_fragment(&doc, &chapter);
    let again = parse_fragment(&fragment);
    let core = |d: &Document| {
        d.paragraphs()
            .map(|p| {
                (
                    p.text().to_owned(),
                    p.props.style,
                    p.props.list.map(|l| l.style),
                )
            })
            .filter(|(t, _, _)| !t.is_empty())
            .collect::<Vec<_>>()
    };
    assert_eq!(core(&doc), core(&again), "{fragment}");
    let p = para(&again, "A pasted chapter with bold.");
    assert!(char_at(&again, p, "chapter").italic);
    assert!(char_at(&again, p, "bold").is_bold());
}

// ---- Writing ---------------------------------------------------------------------------

fn png() -> Arc<libertum_model::Media> {
    let bytes = std::fs::read(fixtures().join("red-4x2.png")).expect("png");
    Arc::new(libertum_model::Media::new(bytes).expect("media"))
}

/// A document that uses most of what the model can hold.
fn rich_document() -> Document {
    let mut styles_doc = Document::new();
    let mut id = || styles_doc.alloc_id();
    let run = |range: std::ops::Range<usize>, format: CharFormat| Run { range, format };
    let mut blocks = Vec::new();

    blocks.push(Block::Paragraph(
        Paragraph::new(id(), "The Title").with_props(ParaProps {
            style: StyleId::TITLE,
            ..ParaProps::default()
        }),
    ));
    blocks.push(Block::Paragraph(
        Paragraph::new(id(), "Chapter One").with_props(ParaProps {
            style: StyleId::HEADING_1,
            outline_level: Some(1),
            ..ParaProps::default()
        }),
    ));
    let text = "Plain bold italic red big Georgia high under struck  two spaces";
    let at = |s: &str| {
        let i = text.find(s).expect("word");
        i..i + s.len()
    };
    let mut p = Paragraph::with_runs(
        id(),
        text,
        vec![
            run(at("bold"), CharFormat::bold()),
            run(
                at("italic"),
                CharFormat {
                    italic: Some(true),
                    ..CharFormat::default()
                },
            ),
            run(
                at("red"),
                CharFormat {
                    colour: Some([200, 10, 10, 255]),
                    ..CharFormat::default()
                },
            ),
            run(at("big"), CharFormat::sized(15.5)),
            run(
                at("Georgia"),
                CharFormat {
                    family: Some(Arc::from("Georgia")),
                    letter_spacing_pt: Some(0.5),
                    ..CharFormat::default()
                },
            ),
            run(
                at("high"),
                CharFormat {
                    highlight: Some(Some([255, 240, 0, 255])),
                    vertical: Some(VerticalAlign::Superscript),
                    ..CharFormat::default()
                },
            ),
            run(
                at("under"),
                CharFormat {
                    underline: Some(true),
                    weight: Some(300),
                    ..CharFormat::default()
                },
            ),
            run(
                at("struck"),
                CharFormat {
                    strikethrough: Some(true),
                    ..CharFormat::default()
                },
            ),
        ],
    )
    .with_props(ParaProps {
        format: ParaFormat {
            align: Some(Align::Justify),
            indent_left_pt: Some(10.0),
            indent_first_pt: Some(20.0),
            space_before_pt: Some(4.0),
            line_spacing: Some(LineSpacing::AtLeast(14.0)),
            keep_with_next: Some(true),
            hyphenate: Some(true),
            borders: Some(libertum_model::Borders {
                sides: libertum_model::BorderSides::ALL,
                width_pt: 1.5,
                colour: [0, 0, 128, 255],
                space_pt: 3.0,
            }),
            ..ParaFormat::default()
        },
        page_break_before: true,
        ..ParaProps::default()
    });
    p.set_notes(vec![Note {
        kind: NoteKind::Footnote,
        offset: 5,
        text: "A footnote.".into(),
    }]);
    p.set_bookmarks(vec![Bookmark {
        name: "here".into(),
        offset: 0,
    }]);
    blocks.push(Block::Paragraph(p));

    // A heading with a non-bold run.
    blocks.push(Block::Paragraph(
        Paragraph::with_runs(
            id(),
            "Heading light",
            vec![run(
                8..13,
                CharFormat {
                    weight: Some(400),
                    ..CharFormat::default()
                },
            )],
        )
        .with_props(ParaProps {
            style: StyleId::HEADING_2,
            outline_level: Some(2),
            ..ParaProps::default()
        }),
    ));
    blocks.push(Block::Paragraph(
        Paragraph::new(id(), "A quotation.").with_props(ParaProps {
            style: StyleId::QUOTE,
            ..ParaProps::default()
        }),
    ));
    for (text, style, level) in [
        ("Bullet", ListStyle::Bullet, 0),
        ("Deeper", ListStyle::Bullet, 1),
        ("Back", ListStyle::Bullet, 0),
    ] {
        blocks.push(Block::Paragraph(Paragraph::new(id(), text).with_props(
            ParaProps {
                list: Some(ListItem {
                    style,
                    level,
                    group: 7,
                }),
                ..ParaProps::default()
            },
        )));
    }
    blocks.push(Block::Paragraph(Paragraph::new(id(), "Between lists")));
    blocks.push(Block::Paragraph(
        Paragraph::new(id(), "Lettered item").with_props(ParaProps {
            list: Some(ListItem {
                style: ListStyle::Lettered,
                level: 0,
                group: 8,
            }),
            ..ParaProps::default()
        }),
    ));
    let mut image_para = Paragraph::new(id(), "Picture: here");
    let mut image = InlineImage::fitted(9, png(), 400.0);
    image.width_pt = 60.0;
    image.height_pt = 30.0;
    image.alt = "red box".into();
    image_para.set_images(vec![image]);
    blocks.push(Block::Paragraph(image_para));
    blocks.push(Block::Paragraph(Paragraph::new(id(), "Line one\nLine two")));
    blocks.push(Block::Paragraph(Paragraph::new(id(), "")));

    let mut doc = Document::from_blocks(blocks);
    // A table with a header row, a row span, a column span and a fill.
    let mut table = doc.new_table(3, 3, 450.0);
    table.header_rows = 1;
    table.widths_pt = vec![100.0, 150.0, 200.0];
    let set = |t: &mut libertum_model::Table, r: usize, c: usize, text: &str| {
        let id = t.rows[r].cells[c].paragraphs[0].id;
        t.rows[r].cells[c].paragraphs[0] = Paragraph::new(id, text);
    };
    set(&mut table, 0, 0, "H1");
    set(&mut table, 0, 1, "H2");
    set(&mut table, 0, 2, "H3");
    set(&mut table, 1, 0, "Tall");
    set(&mut table, 1, 1, "Wide");
    table.rows[1].cells[1].col_span = 2;
    table.rows[1].cells.remove(2);
    table.rows[2].cells[0].merged_into_above = true;
    table.rows[2].cells[1].fill = Some([10, 200, 10, 255]);
    set(&mut table, 2, 1, "b");
    set(&mut table, 2, 2, "c");
    doc.append_blocks(vec![Block::Table(table)]);
    doc.sections[0].page_colour = Some([250, 240, 220, 255]);
    doc.sections[0].page.paper = libertum_model::PaperSize::Letter;
    doc.sections[0].page.margins.left = 54.0;
    doc.styles.defaults.family = Arc::from("Palatino");
    doc.styles.defaults.size_pt = 12.0;
    doc
}

/// What a reader sees: per paragraph, its text, resolved paragraph formatting, the resolved
/// formatting of each character, list kind and level, heading level, images, notes and
/// bookmarks; plus the page and tables.
fn snapshot(doc: &Document) -> Vec<String> {
    fn para_lines(doc: &Document, p: &Paragraph, out: &mut Vec<String>) {
        let r = doc.styles.resolve_props(&p.props);
        out.push(format!(
            "P {:?} level={:?} list={:?} break={} {:?}",
            p.text(),
            p.props.outline_level,
            p.props.list.map(|l| (l.style, l.level)),
            p.props.page_break_before,
            r
        ));
        let base = doc.styles.resolve_char(p.props.style);
        let mut last = String::new();
        for (i, _) in p.text().char_indices() {
            let c = base.apply(&p.format_of(i));
            let line = format!("  C {c:?}");
            if line != last {
                out.push(format!("  @{i}"));
                out.push(line.clone());
                last = line;
            }
        }
        for image in p.images() {
            out.push(format!(
                "  I {} {} {} {:?} {:x}",
                image.offset,
                image.width_pt,
                image.height_pt,
                image.alt,
                image.media.hash()
            ));
        }
        for note in p.notes() {
            out.push(format!("  N {note:?}"));
        }
        for bookmark in p.bookmarks() {
            out.push(format!("  B {bookmark:?}"));
        }
    }
    let mut out = vec![format!(
        "page {:?} colour {:?}",
        doc.sections[0].page, doc.sections[0].page_colour
    )];
    for block in doc.body().iter() {
        match block {
            Block::Paragraph(p) => para_lines(doc, p, &mut out),
            Block::Table(t) => {
                out.push(format!(
                    "T widths={:?} header={} borders={:?}",
                    t.widths_pt, t.header_rows, t.borders
                ));
                for (r, row) in t.rows.iter().enumerate() {
                    for cell in &row.cells {
                        out.push(format!(
                            " cell r{r} span={} merged={} fill={:?}",
                            cell.col_span, cell.merged_into_above, cell.fill
                        ));
                        if !cell.merged_into_above {
                            for p in &cell.paragraphs {
                                para_lines(doc, p, &mut out);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

#[test]
fn full_html_round_trips_what_the_model_holds() {
    let doc = rich_document();
    let bytes = Html::new(doc.clone()).to_bytes();
    let text = String::from_utf8(bytes).expect("utf-8");
    let back = Html::parse(&text, None);
    assert!(back.warnings().is_empty(), "{:?}", back.warnings());
    assert_eq!(back.flavour(), HtmlFlavour::Full);
    let want = snapshot(&doc);
    let got = snapshot(back.document());
    for (i, (a, b)) in want.iter().zip(&got).enumerate() {
        assert_eq!(a, b, "line {i} differs\n--- html ---\n{text}");
    }
    assert_eq!(want.len(), got.len(), "{text}");
    assert!(text.contains("<meta name=\"generator\" content=\"Libertum\">"));
    // A second trip changes nothing.
    let again = Html::new(back.document().clone()).to_bytes();
    assert_eq!(snapshot(Html::parse_bytes(&again, None).document()), got);
}

#[test]
fn a_document_with_its_own_style_keeps_the_look() {
    let mut doc = rich_document();
    doc.styles.paragraph.push(libertum_model::ParagraphStyle {
        name: "Poem".into(),
        base: Some(StyleId::BODY),
        char: CharFormat {
            italic: Some(true),
            colour: Some([0, 90, 0, 255]),
            ..CharFormat::default()
        },
        para: ParaFormat {
            align: Some(Align::Centre),
            ..ParaFormat::default()
        },
        next: None,
        outline_level: None,
    });
    let poem = StyleId(doc.styles.paragraph.len() as u32 - 1);
    let mut blocks: Vec<Block> = doc.body().iter().cloned().collect();
    let next = doc.next_free_id();
    blocks.push(Block::Paragraph(
        Paragraph::new(libertum_model::BlockId(next), "Roses are red").with_props(ParaProps {
            style: poem,
            ..ParaProps::default()
        }),
    ));
    let mut with_poem = Document::from_blocks(blocks);
    with_poem.styles = doc.styles.clone();
    with_poem.sections = doc.sections.clone();
    let html = String::from_utf8(Html::new(with_poem.clone()).to_bytes()).expect("utf-8");
    let back = Html::parse(&html, None);
    let p = para(back.document(), "Roses are red");
    let c = char_at(back.document(), p, "Roses");
    assert!(c.italic);
    assert_eq!(c.colour, [0, 90, 0, 255]);
    assert_eq!(
        back.document().styles.resolve_props(&p.props).align,
        Align::Centre
    );
    let losses = loss_report(&with_poem, HtmlFlavour::Full);
    assert!(
        losses
            .iter()
            .any(|l| l.kind == LossKind::ParagraphStyleName)
    );
}

#[test]
fn plain_html_keeps_structure_but_no_styles() {
    let doc = rich_document();
    let mut html = Html::new(doc.clone());
    html.set_flavour(HtmlFlavour::Plain);
    let text = String::from_utf8(html.to_bytes()).expect("utf-8");
    assert!(!text.contains("style="), "{text}");
    assert!(!text.contains("<style>"));
    let back = Html::parse(&text, None);
    assert_eq!(back.flavour(), HtmlFlavour::Plain);
    let back = back.document();
    assert_eq!(
        texts(back)
            .into_iter()
            .filter(|t| !t.is_empty())
            .take(4)
            .collect::<Vec<_>>(),
        vec![
            "The Title",
            "Chapter One",
            "Plain bold italic red big Georgia high under struck two spaces",
            "Heading light"
        ]
    );
    let p = para(
        back,
        "Plain bold italic red big Georgia high under struck two spaces",
    );
    assert!(char_at(back, p, "bold").is_bold());
    assert!(char_at(back, p, "italic").italic);
    assert_eq!(
        char_at(back, p, "red big").colour,
        [0, 0, 0, 255],
        "no colours"
    );
    assert_eq!(p.notes().len(), 1);
    assert_eq!(back.sections[0].page_colour, None);
    let losses = loss_report(&doc, HtmlFlavour::Plain);
    for kind in [
        LossKind::CharacterFormatting,
        LossKind::ParagraphFormatting,
        LossKind::PageColour,
        LossKind::PageBreak,
    ] {
        assert!(
            losses.iter().any(|l| l.kind == kind),
            "{kind:?} in {losses:?}"
        );
    }
    assert!(
        !loss_report(&doc, HtmlFlavour::Full)
            .iter()
            .any(|l| l.kind == LossKind::CharacterFormatting)
    );
}

#[test]
fn a_saved_file_reopens_with_its_page_colour() {
    let dir = std::env::temp_dir().join(format!("libertum-html-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("saved.html");
    let mut doc = Document::new();
    doc.sections[0].page_colour = Some([40, 40, 60, 255]);
    Html::new(doc).save(&path).expect("save");
    let back = Html::open(&path).expect("open");
    assert_eq!(
        back.document().sections[0].page_colour,
        Some([40, 40, 60, 255])
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---- EPUB ------------------------------------------------------------------------------

fn epub(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut out = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut out);
        let options: zip::write::SimpleFileOptions = Default::default();
        for (name, bytes) in files {
            zip.start_file(*name, options).expect("start");
            zip.write_all(bytes).expect("write");
        }
        zip.finish().expect("finish");
    }
    out.into_inner()
}

const CONTAINER: &[u8] = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
<rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

#[test]
fn an_epub_reads_its_chapters_in_spine_order_with_images() {
    let png = std::fs::read(fixtures().join("red-4x2.png")).expect("png");
    let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
<manifest>
<item id="c2" href="text/two.xhtml" media-type="application/xhtml+xml"/>
<item id="c1" href="text/one.xhtml" media-type="application/xhtml+xml"/>
<item id="img" href="images/red.png" media-type="image/png"/>
</manifest>
<spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;
    let one = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>1</title></head>
<body><h1>One</h1><p>First <em>text</em>.<img src="../images/red.png" alt="r"/></p></body></html>"#;
    let two = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Two</h1><p>Second.</p></body></html>"#;
    let bytes = epub(&[
        ("mimetype", b"application/epub+zip"),
        ("META-INF/container.xml", CONTAINER),
        ("OEBPS/content.opf", opf),
        ("OEBPS/text/one.xhtml", one),
        ("OEBPS/text/two.xhtml", two),
        ("OEBPS/images/red.png", &png),
    ]);
    let (doc, warnings) = import_epub_bytes(&bytes).expect("reads");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(texts(&doc), vec!["One", "First text.", "Two", "Second."]);
    assert_eq!(para(&doc, "First text.").images().len(), 1);
    assert_eq!(para(&doc, "Two").props.outline_level, Some(1));
}

#[test]
fn an_ao3_epub_gets_the_ao3_rules() {
    let opf = br#"<package><manifest>
<item id="p" href="preface.xhtml" media-type="application/xhtml+xml"/>
<item id="c" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
<item id="a" href="afterword.xhtml" media-type="application/xhtml+xml"/>
</manifest><spine><itemref idref="p"/><itemref idref="c"/><itemref idref="a"/></spine></package>"#;
    // Hand-made, modelled on AO3's layout; not taken from a real AO3 EPUB.
    let preface = br#"<html><body><div id="preface"><div class="meta"><dl class="tags"><dt>Rating:</dt><dd>Teen</dd></dl><h1>Work</h1><p>Summary</p><blockquote class="userstuff"><p>Sum.</p></blockquote></div></div></body></html>"#;
    let chapter = br#"<html><body><div id="chapters" class="userstuff"><div class="meta group"><h2 class="heading">Chapter 1</h2></div><div class="userstuff"><p>Story.</p></div></div></body></html>"#;
    let after = br#"<html><body><div id="afterword"><p class="message">Leave kudos!</p></div></body></html>"#;
    let bytes = epub(&[
        ("META-INF/container.xml", CONTAINER),
        ("OEBPS/content.opf", opf),
        ("OEBPS/preface.xhtml", preface),
        ("OEBPS/chapter1.xhtml", chapter),
        ("OEBPS/afterword.xhtml", after),
    ]);
    let (doc, _) = import_epub_bytes(&bytes).expect("reads");
    assert_eq!(texts(&doc), vec!["Summary", "Sum.", "Chapter 1", "Story."]);
}

#[test]
fn a_broken_epub_is_an_error_not_a_panic() {
    assert!(import_epub_bytes(b"not a zip").is_err());
    let bytes = epub(&[("META-INF/container.xml", CONTAINER)]);
    assert!(import_epub_bytes(&bytes).is_err());
}

// ---- Robustness ------------------------------------------------------------------------

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

    #[test]
    fn any_markup_reads_and_writes_without_panicking(
        pieces in proptest::collection::vec(
            proptest::prop_oneof![
                proptest::prelude::Just("<p>".to_owned()),
                proptest::prelude::Just("</p>".to_owned()),
                proptest::prelude::Just("<b>".to_owned()),
                proptest::prelude::Just("</i>".to_owned()),
                proptest::prelude::Just("<ul><li>".to_owned()),
                proptest::prelude::Just("<table><tr><td rowspan=3 colspan=2>".to_owned()),
                proptest::prelude::Just("<br>".to_owned()),
                proptest::prelude::Just("<img src='data:image/png;base64,AAAA'>".to_owned()),
                proptest::prelude::Just("<span style='font-size: 200%; color: rgb(1,'>".to_owned()),
                proptest::prelude::Just("<pre>".to_owned()),
                proptest::prelude::Just("<sup class=footnote-ref><a href='#x'>1</a></sup>".to_owned()),
                proptest::prelude::Just("<h7>".to_owned()),
                "[ -~\u{e9}\u{4e2d}\n]{0,12}",
            ],
            0..40,
        )
    ) {
        let source = pieces.concat();
        let html = Html::parse(&source, None);
        let bytes = html.to_bytes();
        let again = Html::parse_bytes(&bytes, None);
        let _ = parse_fragment(&source);
        proptest::prop_assert!(again.document().paragraph_count() >= 1);
    }
}

#[test]
fn bold_constant_is_what_strong_gives() {
    let doc = parse_fragment("<p><strong>x</strong></p>");
    let p = doc.paragraphs().next().expect("p");
    assert_eq!(p.format_of(0).weight, Some(BOLD));
}
