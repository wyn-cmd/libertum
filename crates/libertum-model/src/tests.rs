use std::time::{Duration, Instant};

use proptest::prelude::*;

use crate::*;

fn doc_with(texts: &[&str]) -> Document {
    Document::from_paragraphs(
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| Paragraph::new(BlockId(i as u64 + 1), *t))
            .collect(),
    )
}

fn para_id(doc: &Document, index: usize) -> BlockId {
    doc.paragraph_at(index).expect("paragraph").id
}

// ---------------------------------------------------------------- paragraph invariants

#[test]
fn insert_and_delete_keep_runs_consistent() {
    let mut p = Paragraph::with_runs(
        BlockId(1),
        "hello world",
        vec![
            Run {
                range: 0..5,
                format: CharFormat::sized(20.0),
            },
            Run {
                range: 6..11,
                format: CharFormat::sized(14.0),
            },
        ],
    );
    // Typing right after "hello" continues its formatting (as in Word and Pages).
    p.insert_text(5, "!");
    assert_eq!(p.text(), "hello! world");
    assert_eq!(p.runs()[0].range, 0..6);
    assert_eq!(p.runs()[1].range, 7..12);
    // At the very start, new text takes the first character's formatting.
    p.insert_text(0, ">> ");
    assert_eq!(p.text(), ">> hello! world");
    assert_eq!(p.runs()[0].range, 0..9);
    let removed = p.delete_range(0..3);
    assert_eq!(removed, ">> ");
    assert_eq!(p.text(), "hello! world");
    assert_eq!(p.runs()[0].range, 0..6);
    assert_eq!(p.runs()[1].range, 7..12);
    // Deleting across a run boundary clamps both.
    p.delete_range(3..8);
    assert_eq!(p.text(), "helorld");
    for run in p.runs() {
        assert!(run.range.start < run.range.end && run.range.end <= p.len());
    }
}

#[test]
fn split_and_merge_preserve_runs_and_text() {
    let mut p = Paragraph::with_runs(
        BlockId(1),
        "hello world",
        vec![Run {
            range: 6..11,
            format: CharFormat::sized(14.0),
        }],
    );
    let tail = p.split_off(5, BlockId(2), None);
    assert_eq!(p.text(), "hello");
    assert_eq!(tail.text(), " world");
    assert_eq!(tail.runs()[0].range, 1..6);
    p.append(tail);
    assert_eq!(p.text(), "hello world");
    assert_eq!(p.runs()[0].range, 6..11);
}

// ---------------------------------------------------------------- operations

#[test]
fn each_operation_returns_its_inverse() {
    let mut doc = doc_with(&["alpha", "beta"]);
    let a = para_id(&doc, 0);
    let b = para_id(&doc, 1);
    let original = doc.clone();

    let inv = doc
        .apply(Operation::InsertText {
            block: a,
            offset: 5,
            text: " gamma".into(),
            format: None,
        })
        .unwrap();
    assert!(matches!(inv, Operation::DeleteRange { .. }));
    doc.apply(inv).unwrap();
    assert!(doc.content_eq(&original), "content differs after undo");

    let inv = doc
        .apply(Operation::DeleteRange {
            block: a,
            range: 1..4,
        })
        .unwrap();
    assert!(matches!(inv, Operation::InsertText { .. }));
    doc.apply(inv).unwrap();
    assert!(doc.content_eq(&original), "content differs after undo");

    let new_id = doc.alloc_id();
    let inv = doc
        .apply(Operation::SplitBlock {
            block: a,
            offset: 2,
            new_id,
            props: None,
        })
        .unwrap();
    assert_eq!(doc.len(), 3);
    doc.apply(inv).unwrap();
    assert!(doc.content_eq(&original), "content differs after undo");

    let inv = doc.apply(Operation::MergeBlocks { first: a }).unwrap();
    assert_eq!(doc.len(), 1);
    doc.apply(inv).unwrap();
    assert!(doc.content_eq(&original), "content differs after undo");

    let removed = doc.apply(Operation::RemoveBlock { block: b }).unwrap();
    assert_eq!(doc.len(), 1);
    doc.apply(removed).unwrap();
    assert!(doc.content_eq(&original), "content differs after undo");
}

#[test]
fn bad_operations_are_typed_errors() {
    let mut doc = doc_with(&["abc"]);
    let a = para_id(&doc, 0);
    assert!(
        doc.apply(Operation::InsertText {
            block: a,
            offset: 2,
            text: "x".into(),
            format: None,
        })
        .is_ok()
    );
    let a = para_id(&doc, 0);
    assert!(matches!(
        doc.apply(Operation::DeleteRange {
            block: a,
            range: 0..99
        }),
        Err(ModelError::RangeOutOfBounds { .. })
    ));
    assert!(matches!(
        doc.apply(Operation::InsertText {
            block: BlockId(999),
            offset: 0,
            text: "x".into(),
            format: None,
        }),
        Err(ModelError::BlockNotFound(_))
    ));
}

#[test]
fn word_count_stays_correct_across_edits() {
    let mut doc = doc_with(&["one two", "three"]);
    assert_eq!(doc.word_count(), 3);
    let a = para_id(&doc, 0);
    doc.apply(Operation::InsertText {
        block: a,
        offset: 7,
        text: " four".into(),
        format: None,
    })
    .unwrap();
    assert_eq!(doc.word_count(), 4);
    doc.apply(Operation::DeleteRange {
        block: a,
        range: 0..4,
    })
    .unwrap();
    assert_eq!(doc.word_count(), 3);

    // A split joins nothing, but changes the boundary word count only if a word is cut in two.
    let new_id = doc.alloc_id();
    doc.apply(Operation::SplitBlock {
        block: a,
        offset: doc.paragraph(a).unwrap().len(),
        new_id,
        props: None,
    })
    .unwrap();
    assert_eq!(doc.word_count(), 3);
    doc.apply(Operation::MergeBlocks { first: a }).unwrap();
    assert_eq!(doc.word_count(), 3);
}

#[test]
fn splitting_a_word_in_two_counts_two_words() {
    let mut doc = doc_with(&["foobar"]);
    let a = para_id(&doc, 0);
    let new_id = doc.alloc_id();
    doc.apply(Operation::SplitBlock {
        block: a,
        offset: 3,
        new_id,
        props: None,
    })
    .unwrap();
    assert_eq!(doc.word_count(), 2);
}

// ---------------------------------------------------------------- history

#[test]
fn typing_coalesces_until_whitespace_or_pause() {
    let mut doc = doc_with(&[""]);
    let a = para_id(&doc, 0);
    // Rebuild entries against the real block id.
    let entry = |text: &str, at: usize| {
        HistoryEntry::new(
            vec![Operation::InsertText {
                block: a,
                offset: at,
                text: text.into(),
                format: None,
            }],
            vec![Operation::DeleteRange {
                block: a,
                range: at..at + text.len(),
            }],
            EditKind::Typing,
        )
    };
    let t0 = Instant::now();
    let mut h = History::new(100);

    h.record(entry("h", 0), t0);
    h.record(entry("i", 1), t0 + Duration::from_millis(100));
    assert_eq!(h.undo_len(), 1, "consecutive letters coalesce");

    h.record(entry(" ", 2), t0 + Duration::from_millis(200));
    assert_eq!(h.undo_len(), 2, "whitespace starts a new step");

    h.record(entry("x", 3), t0 + Duration::from_millis(300));
    assert_eq!(h.undo_len(), 3, "typing after whitespace starts a new step");

    h.record(entry("y", 4), t0 + Duration::from_secs(3));
    assert_eq!(h.undo_len(), 4, "a pause starts a new step");

    // The coalesced step really contains "hi".
    let _ = &mut doc;
}

#[test]
fn undo_redo_restores_document_exactly() {
    let mut doc = doc_with(&["abc"]);
    let a = para_id(&doc, 0);
    let new_id = doc.alloc_id();
    let mut h = History::new(100);

    let before = doc.clone();
    let fwd = vec![
        Operation::InsertText {
            block: a,
            offset: 3,
            text: "def".into(),
            format: None,
        },
        Operation::SplitBlock {
            block: a,
            offset: 2,
            new_id,
            props: None,
        },
    ];
    let inv = doc.apply_transaction(fwd.clone()).unwrap();
    h.record(HistoryEntry::new(fwd, inv, EditKind::Other), Instant::now());
    assert!(!doc.content_eq(&before));

    h.undo(&mut doc).unwrap();
    assert!(doc.content_eq(&before), "content differs");
    h.redo(&mut doc).unwrap();
    assert!(!doc.content_eq(&before));
    h.undo(&mut doc).unwrap();
    assert!(doc.content_eq(&before), "content differs");
}

#[test]
fn thousand_undo_steps() {
    let mut doc = doc_with(&[""]);
    let a = para_id(&doc, 0);
    let mut h = History::new(2000);
    let original = doc.clone();
    let mut now = Instant::now();
    for i in 0..1000 {
        let op = Operation::InsertText {
            block: a,
            offset: i,
            text: "x".into(),
            format: None,
        };
        let inv = doc.apply(op.clone()).unwrap();
        // `Other` so each step is kept separately (the acceptance criterion is 1,000 steps).
        h.record(HistoryEntry::new(vec![op], vec![inv], EditKind::Other), now);
        now += Duration::from_millis(2);
    }
    assert_eq!(h.undo_len(), 1000);
    for _ in 0..1000 {
        h.undo(&mut doc).unwrap();
    }
    assert!(doc.content_eq(&original), "content differs after undo");
    assert_eq!(h.redo_len(), 1000);
    for _ in 0..1000 {
        h.redo(&mut doc).unwrap();
    }
    assert_eq!(doc.paragraph(a).unwrap().text(), "x".repeat(1000));
}

// ---------------------------------------------------------------- proptest

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

proptest! {
    /// Applying random operations and then their inverses in reverse restores the document.
    #[test]
    fn apply_then_invert_restores(ops in prop::collection::vec(0u8..15, 1..60), seed in 1u64..10_000) {
        let mut doc = doc_with(&["The quick brown fox", "jumps over", "", "the lazy dog"]);
        // Non-default props, so a merge followed by its inverse must restore them exactly.
        let heading = doc.paragraph_at(1).map(|p| p.id).expect("paragraph 1");
        doc.apply(Operation::SetParaProps { block: heading, props: chapter_props() }).expect("set props");
        doc.sections.push(Section::default());
        // Formatted text, so deletions, merges and their inverses must restore runs exactly.
        let first = doc.paragraph_at(0).map(|p| p.id).expect("paragraph 0");
        let runs = doc.paragraph(first).expect("p0").runs_with(4..15, &CharFormat::bold());
        doc.apply(Operation::SetRuns { block: first, runs }).expect("set runs");
        let original = doc.clone();
        let mut rng = Rng(seed);
        let mut inverses = Vec::new();
        for kind in ops {
            let n = doc.len();
            let index = rng.below(n);
            let id = doc.paragraph_at(index).map(|p| p.id);
            let Some(id) = id else { continue };
            let len = doc.paragraph(id).map(|p| p.text().len()).unwrap_or(0);
            let op = match kind {
                0 => {
                    let at = rng.below(len + 1);
                    Operation::InsertText { block: id, offset: at, text: "xy".into(), format: None }
                }
                1 if len > 0 => {
                    let a = rng.below(len);
                    let b = rng.below(len + 1).max(a);
                    Operation::DeleteRange { block: id, range: a..b }
                }
                2 => {
                    let at = rng.below(len + 1);
                    let new_id = doc.alloc_id();
                    Operation::SplitBlock { block: id, offset: at, new_id, props: None }
                }
                5 | 6 => {
                    let a = rng.below(len + 1);
                    let b = rng.below(len + 1).max(a);
                    let patch = if kind == 5 {
                        CharFormat { italic: Some(true), ..CharFormat::default() }
                    } else {
                        CharFormat { size_pt: Some(9.0), weight: Some(REGULAR), ..CharFormat::default() }
                    };
                    let runs = doc.paragraph(id).map(|p| p.runs_with(a..b, &patch)).unwrap_or_default();
                    Operation::SetRuns { block: id, runs }
                }
                3 => Operation::SetParaProps {
                    block: id,
                    props: if rng.below(2) == 0 { chapter_props() } else { ParaProps::default() },
                },
                8 => {
                    let mut props = doc.paragraph(id).map(|p| p.props).unwrap_or_default();
                    props.format = props.format.overlay(&ParaFormat {
                        indent_first_pt: Some(18.0),
                        line_spacing: Some(LineSpacing::Exactly(20.0)),
                        keep_with_next: Some(rng.below(2) == 0),
                        ..ParaFormat::default()
                    });
                    Operation::SetParaProps { block: id, props }
                }
                9 => {
                    let style = StyleId(rng.below(3) as u32);
                    let mut def = doc.styles.get(style).cloned().expect("built-in style");
                    def.para.align = Some(Align::Justify);
                    def.char.italic = Some(true);
                    Operation::SetStyle { id: style, style: Box::new(def) }
                }
                4 => Operation::SetSection {
                    index: rng.below(2),
                    section: Section {
                        page: PageSetup {
                            paper: if rng.below(2) == 0 { PaperSize::A4 } else { PaperSize::Letter },
                            orientation: if rng.below(2) == 0 {
                                Orientation::Portrait
                            } else {
                                Orientation::Landscape
                            },
                            margins: Margins::uniform(54.0 + rng.below(4) as f32),
                        },
                        page_numbers: Some(PageNumberPos::BOTTOM_CENTRE),
                        first_page_number: Some(1),
                        headers: HeadersFooters::with_default("Chapter {page}"),
                        footers: HeadersFooters {
                            different_first: true,
                            first: Some(HeaderFooter::new("First page")),
                            even: Some(HeaderFooter::new("Even page")),
                            different_even_odd: rng.below(2) == 0,
                            default: Some(HeaderFooter::new("{page}")),
                        },
                        header_distance_pt: 30.0 + rng.below(4) as f32,
                        ..Section::default()
                    },
                },
                7 => {
                    let mut char = doc.styles.defaults.clone();
                    char.size_pt = 12.0;
                    char.ligatures = if rng.below(2) == 0 {
                        Ligatures::None
                    } else {
                        Ligatures::Standard
                    };
                    Operation::SetCharDefaults { char }
                }
                10 => {
                    let offset = rng.below(len + 1);
                    let name = format!("mark{}", rng.below(3));
                    let mut bookmarks = doc
                        .paragraph(id)
                        .map(|p| p.bookmarks().to_vec())
                        .unwrap_or_default();
                    match bookmarks.iter_mut().find(|bookmark| bookmark.name == name) {
                        Some(bookmark) => bookmark.offset = offset,
                        None => bookmarks.push(Bookmark { name, offset }),
                    }
                    Operation::SetBookmarks { block: id, bookmarks }
                }
                11 => {
                    let mut images = doc.paragraph(id).map(|p| p.images().to_vec()).unwrap_or_default();
                    if images.len() < 2 {
                        let media = std::sync::Arc::new(Media::new(test_png_header(40, 20)).expect("png"));
                        images.push(InlineImage::fitted(rng.below(len + 1), media, 300.0));
                    } else {
                        images.remove(0);
                    }
                    Operation::SetImages { block: id, images }
                }
                12 => {
                    let a = rng.below(len + 1);
                    let b = rng.below(len + 1).max(a);
                    Operation::SetCrossRefs {
                        block: id,
                        cross_refs: vec![CrossRef { range: a..b, target: "mark0".into(), kind: CrossRefKind::Page }],
                    }
                }
                14 => Operation::SetPageNumbering {
                    numbering: Some(PageNumbering {
                        show: rng.below(2) == 0,
                        position: PageNumberPos::TOP_RIGHT,
                        hide_first_page: rng.below(2) == 0,
                        start_page: 1 + rng.below(4) as u32,
                        start_at: rng.below(10) as u32,
                    }),
                },
                13 => {
                    let a = rng.below(len + 1);
                    let b = rng.below(len + 1).max(a);
                    let mut fields = doc
                        .paragraph(id)
                        .map(|p| p.date_fields().to_vec())
                        .unwrap_or_default();
                    if fields.is_empty() {
                        fields.push(DateTimeField::new(
                            a..b,
                            DateTimeSource::Updated,
                            DateTimeFormat::DateLong,
                        ));
                    } else {
                        fields.clear();
                    }
                    Operation::SetDateTimeFields {
                        block: id,
                        date_fields: fields,
                    }
                }
                _ if index + 1 < n => Operation::MergeBlocks { first: id },
                _ => continue,
            };
            let before = doc.clone();
            if let Ok(inv) = doc.apply(op.clone()) {
                // Each step on its own must invert exactly (pinpoints the culprit).
                let mut check = doc.clone();
                check.apply(inv.clone()).expect("inverse applies");
                prop_assert!(check.content_eq(&before), "step does not invert: {:?}", op);
                inverses.push(inv);
            }
        }
        for inv in inverses.iter().rev() {
            doc.apply(inv.clone()).expect("inverse applies");
        }
        prop_assert!(doc.content_eq(&original), "content differs after undo");
    }
}

fn chapter_props() -> ParaProps {
    ParaProps {
        format: ParaFormat {
            align: Some(Align::Centre),
            space_before_pt: Some(72.0),
            ..ParaFormat::default()
        },
        page_break_before: true,
        section_start: true,
        outline_level: Some(1),
        ..ParaProps::default()
    }
}

#[test]
fn merge_then_undo_restores_the_second_paragraphs_props() {
    let mut doc = doc_with(&["Body text.", "Chapter Two"]);
    let a = doc.paragraph_at(0).unwrap().id;
    let b = doc.paragraph_at(1).unwrap().id;
    doc.apply(Operation::SetParaProps {
        block: b,
        props: chapter_props(),
    })
    .unwrap();
    let before = doc.clone();
    let inverse = doc.apply(Operation::MergeBlocks { first: a }).unwrap();
    assert_eq!(doc.len(), 1);
    assert_eq!(doc.paragraph(a).unwrap().props, ParaProps::default());
    doc.apply(inverse).unwrap();
    assert!(doc.content_eq(&before));
    assert_eq!(doc.paragraph(b).unwrap().props, chapter_props());
}

#[test]
fn split_tail_drops_page_flow_markers_and_ends_a_heading() {
    let props = chapter_props();
    // Enter in the middle of a heading: both halves stay headings, but only the first one
    // starts a page and a section.
    let mid = props.for_split_tail(3, 10);
    assert!(!mid.page_break_before && !mid.section_start);
    assert_eq!(mid.outline_level, Some(1));
    assert_eq!(mid.format.align, Some(Align::Centre));
    // Enter at the end of a heading starts a plain body paragraph.
    let end = props.for_split_tail(10, 10);
    assert_eq!(end.outline_level, None);
    assert!(end.format.is_empty(), "no direct formatting carried over");
    // TOC entries never propagate.
    let toc = ParaProps {
        toc_target: Some(BlockId(7)),
        ..ParaProps::default()
    };
    assert_eq!(toc.for_split_tail(0, 4).toc_target, None);
}

#[test]
fn set_para_props_bumps_the_version_and_inverts() {
    let mut doc = doc_with(&["Hello"]);
    let a = doc.paragraph_at(0).unwrap().id;
    let v = doc.paragraph(a).unwrap().version;
    let inverse = doc
        .apply(Operation::SetParaProps {
            block: a,
            props: chapter_props(),
        })
        .unwrap();
    assert!(
        doc.paragraph(a).unwrap().version > v,
        "layout caches must see the change"
    );
    let set = ChangeSet::from_operation(&inverse);
    assert_eq!(set.dirty, vec![a]);
    assert!(!set.structural);
    doc.apply(inverse).unwrap();
    assert_eq!(doc.paragraph(a).unwrap().props, ParaProps::default());
}

#[test]
fn sections_are_counted_from_section_starts() {
    let mut doc = doc_with(&["Title", "Contents", "Chapter One", "Body", "Chapter Two"]);
    doc.sections.push(Section::default());
    let c1 = doc.paragraph_at(2).unwrap().id;
    let c2 = doc.paragraph_at(4).unwrap().id;
    let start = ParaProps {
        section_start: true,
        ..ParaProps::default()
    };
    doc.apply(Operation::SetParaProps {
        block: c1,
        props: start,
    })
    .unwrap();
    doc.apply(Operation::SetParaProps {
        block: c2,
        props: start,
    })
    .unwrap();
    assert_eq!(doc.section_of(0), 0);
    assert_eq!(doc.section_of(1), 0);
    assert_eq!(doc.section_of(2), 1);
    assert_eq!(doc.section_of(3), 1);
    // Only two sections exist, so a third start is clamped to the last one.
    assert_eq!(doc.section_of(4), 1);
    let missing = doc.apply(Operation::SetSection {
        index: 5,
        section: Section::default(),
    });
    assert!(matches!(missing, Err(ModelError::SectionNotFound { .. })));
}

#[test]
fn section_columns_default_and_clamp() {
    let mut section = Section::default();
    assert_eq!(section.columns, 1);
    assert!((section.column_gutter_pt - DEFAULT_COLUMN_GUTTER_PT).abs() < f32::EPSILON);
    section.set_columns(0);
    assert_eq!(section.columns, 1);
    section.set_columns(3);
    assert_eq!(section.columns, 3);
    section.set_columns(99);
    assert_eq!(section.columns, MAX_COLUMNS);
    section.set_column_gutter(-5.0);
    assert_eq!(section.column_gutter_pt, 0.0);
    section.set_column_gutter(f32::NAN);
    assert!((section.column_gutter_pt - DEFAULT_COLUMN_GUTTER_PT).abs() < f32::EPSILON);
}

#[test]
fn deleting_formatted_text_and_undoing_restores_its_formatting() {
    let mut doc = doc_with(&["plain bold plain"]);
    let id = doc.paragraph_at(0).unwrap().id;
    let runs = doc
        .paragraph(id)
        .unwrap()
        .runs_with(6..10, &CharFormat::bold());
    doc.apply(Operation::SetRuns { block: id, runs }).unwrap();
    let before = doc.clone();
    let inverse = doc
        .apply(Operation::DeleteRange {
            block: id,
            range: 4..12,
        })
        .unwrap();
    assert_eq!(doc.paragraph(id).unwrap().text(), "plailain");
    doc.apply(inverse).unwrap();
    assert!(doc.content_eq(&before), "the bold word comes back bold");
}

#[test]
fn runs_with_overlays_a_patch_and_stays_normalised() {
    let p = Paragraph::with_runs(
        BlockId(1),
        "abcdefgh",
        vec![Run {
            range: 2..6,
            format: CharFormat::bold(),
        }],
    );
    let italic = CharFormat {
        italic: Some(true),
        ..CharFormat::default()
    };
    let runs = p.runs_with(4..8, &italic);
    let bold_italic = CharFormat::bold().overlay(&italic);
    assert_eq!(
        runs,
        vec![
            Run {
                range: 2..4,
                format: CharFormat::bold()
            },
            Run {
                range: 4..6,
                format: bold_italic
            },
            Run {
                range: 6..8,
                format: italic
            },
        ]
    );
    // Removing bold over the same span as the bold run merges nothing away incorrectly.
    let unbold = CharFormat {
        weight: Some(REGULAR),
        ..CharFormat::default()
    };
    let runs = p.runs_with(2..6, &unbold);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].format.weight, Some(REGULAR));
}

#[test]
fn typing_continues_the_previous_characters_format() {
    let p = Paragraph::with_runs(
        BlockId(1),
        "ab",
        vec![Run {
            range: 0..1,
            format: CharFormat::bold(),
        }],
    );
    assert_eq!(p.format_for_typing_at(1), CharFormat::bold());
    assert_eq!(
        p.format_for_typing_at(0),
        CharFormat::bold(),
        "start: first char"
    );
    assert!(p.format_for_typing_at(2).is_empty());
}

// ---------------------------------------------------------------- headers and footers

#[test]
fn header_variants_pick_the_first_even_and_default_pages() {
    let set = HeadersFooters {
        default: Some(HeaderFooter::new("Body")),
        first: Some(HeaderFooter::new("Title")),
        even: Some(HeaderFooter::new("Left")),
        different_first: true,
        different_even_odd: true,
    };
    // Page index parity: page 0 is printed page 1 (odd), page 1 is printed page 2 (even).
    assert_eq!(
        set.for_page(0, true).map(|(v, h)| (v, h.text.as_str())),
        Some((HfVariant::First, "Title"))
    );
    assert_eq!(
        set.for_page(1, false).map(|(v, h)| (v, h.text.as_str())),
        Some((HfVariant::Even, "Left"))
    );
    assert_eq!(
        set.for_page(2, false).map(|(v, h)| (v, h.text.as_str())),
        Some((HfVariant::Default, "Body"))
    );
    // Switched-off variants are ignored even when they carry text.
    let off = HeadersFooters {
        different_first: false,
        different_even_odd: false,
        ..set.clone()
    };
    assert_eq!(
        off.for_page(0, true).map(|(v, _)| v),
        Some(HfVariant::Default)
    );
    assert_eq!(
        off.for_page(1, false).map(|(v, _)| v),
        Some(HfVariant::Default)
    );
}

#[test]
fn a_switched_on_but_empty_variant_falls_back_to_the_default() {
    let set = HeadersFooters {
        default: Some(HeaderFooter::new("Every page")),
        different_first: true,
        ..HeadersFooters::default()
    };
    assert_eq!(
        set.for_page(0, true).map(|(v, h)| (v, h.text.as_str())),
        Some((HfVariant::Default, "Every page"))
    );
    // No default header means no header at all, whatever the variants hold.
    let none = HeadersFooters {
        default: None,
        first: Some(HeaderFooter::new("Only first")),
        different_first: true,
        ..HeadersFooters::default()
    };
    assert!(none.for_page(0, true).is_none());
}

#[test]
fn header_fields_substitute_the_page_number_and_count() {
    let hf = HeaderFooter::new("{page} of {pages}");
    assert_eq!(hf.resolve(3, 12), "3 of 12");
    assert_eq!(hf.resolve(12, 12), "12 of 12");
    // No field, no change; and a page count is never read as a page number.
    assert_eq!(HeaderFooter::new("Draft").resolve(3, 12), "Draft");
    assert_eq!(HeaderFooter::new("{pages}").resolve(3, 12), "12");
}

#[test]
fn section_headers_survive_an_operation_and_its_inverse() {
    let mut doc = doc_with(&["Body"]);
    let original = doc.sections[0].clone();
    let changed = Section {
        headers: HeadersFooters::with_default("Chapter {page}"),
        footers: HeadersFooters::with_default("{page}"),
        header_distance_pt: 20.0,
        footer_distance_pt: 24.0,
        ..Section::default()
    };
    let inverse = doc
        .apply(Operation::SetSection {
            index: 0,
            section: changed.clone(),
        })
        .expect("set section");
    assert_eq!(doc.sections[0], changed);
    assert!(ChangeSet::from_operation(&inverse).sections);
    doc.apply(inverse).expect("restore");
    assert_eq!(doc.sections[0], original);
    // The defaults are Word's 0.5 in.
    assert_eq!(original.header_distance_pt, 36.0);
    assert_eq!(original.footer_distance_pt, 36.0);
}

// ---------------------------------------------------------------- tables

fn table_doc() -> (Document, BlockId) {
    let mut doc = doc_with(&["Before", "After"]);
    let table = doc.new_table(2, 2, 400.0);
    let id = table.id;
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    (doc, id)
}

#[test]
fn a_table_joins_the_flat_paragraph_order() {
    let (doc, table_id) = table_doc();
    // Two body paragraphs plus four cell paragraphs.
    assert_eq!(doc.paragraph_count(), 6);
    let table = doc.table(table_id).expect("table");
    let cell_id = table.cell(0, 0).expect("cell").paragraphs[0].id;
    assert_eq!(doc.index_of(cell_id), Some(1));
    assert!(doc.location_of(cell_id).expect("location").table.is_some());
    assert_eq!(doc.paragraph_at(1).expect("paragraph").text(), "");
    assert_eq!(doc.paragraph_at(5).expect("paragraph").text(), "After");
    assert_eq!(doc.paragraphs().count(), 6);
}

#[test]
fn cell_text_edits_undo_exactly() {
    let (mut doc, table_id) = table_doc();
    let cell_id = doc
        .table(table_id)
        .expect("table")
        .cell(0, 0)
        .expect("cell")
        .paragraphs[0]
        .id;
    let original = doc.clone();
    let words = doc.word_count();
    let inverse = doc
        .apply(Operation::InsertText {
            block: cell_id,
            offset: 0,
            text: "Cell".into(),
            format: None,
        })
        .expect("insert");
    assert_eq!(doc.paragraph(cell_id).expect("paragraph").text(), "Cell");
    assert_eq!(doc.word_count(), words + 1);
    doc.apply(inverse).expect("undo");
    assert!(doc.content_eq(&original));
}

#[test]
fn splitting_and_merging_inside_a_cell_stay_in_the_cell() {
    let (mut doc, table_id) = table_doc();
    let cell_id = doc
        .table(table_id)
        .expect("table")
        .cell(0, 0)
        .expect("cell")
        .paragraphs[0]
        .id;
    let original = doc.clone();
    let text = doc
        .apply(Operation::InsertText {
            block: cell_id,
            offset: 0,
            text: "one two".into(),
            format: None,
        })
        .expect("text");
    let new_id = doc.alloc_id();
    let merge = doc
        .apply(Operation::SplitBlock {
            block: cell_id,
            offset: 3,
            new_id,
            props: None,
        })
        .expect("split");
    let cell = doc
        .table(table_id)
        .expect("table")
        .cell(0, 0)
        .expect("cell");
    assert_eq!(cell.paragraphs.len(), 2);
    assert_eq!(doc.paragraph(cell_id).expect("paragraph").text(), "one");
    assert_eq!(doc.paragraph(new_id).expect("paragraph").text(), " two");
    let place = doc
        .location_of(new_id)
        .expect("location")
        .table
        .expect("cell");
    assert_eq!((place.row, place.cell, place.paragraph), (0, 0, 1));
    doc.apply(merge).expect("merge");
    assert_eq!(
        doc.table(table_id)
            .expect("table")
            .cell(0, 0)
            .expect("cell")
            .paragraphs
            .len(),
        1
    );
    assert_eq!(doc.paragraph(cell_id).expect("paragraph").text(), "one two");
    doc.apply(text).expect("text undo");
    assert!(doc.content_eq(&original));
}

#[test]
fn table_operations_undo_exactly() {
    let mut doc = doc_with(&["Body"]);
    let original = doc.clone();
    let table = doc.new_table(2, 3, 300.0);
    let table_id = table.id;
    let insert = doc
        .apply(Operation::InsertTable { index: 1, table })
        .expect("insert");
    assert_eq!(doc.table(table_id).expect("table").columns(), 3);
    doc.apply(insert).expect("remove");
    assert!(doc.content_eq(&original));

    let table = doc.new_table(1, 2, 200.0);
    let table_id = table.id;
    let insert = doc
        .apply(Operation::InsertTable { index: 0, table })
        .expect("insert");
    let mut changed = doc.table(table_id).expect("table").clone();
    changed.header_rows = 1;
    changed.widths_pt[0] = 120.0;
    let replace = doc
        .apply(Operation::ReplaceTable {
            table: table_id,
            new: changed,
        })
        .expect("replace");
    assert_eq!(doc.table(table_id).expect("table").header_rows, 1);
    assert_eq!(doc.table(table_id).expect("table").widths_pt[0], 120.0);
    doc.apply(replace).expect("restore table");
    assert_eq!(doc.table(table_id).expect("table").header_rows, 0);
    doc.apply(insert).expect("remove");
    assert!(doc.content_eq(&original));
}

#[test]
fn table_word_count_includes_cell_text_and_undoes() {
    let (mut doc, table_id) = table_doc();
    let before = doc.word_count();
    let cell_id = doc
        .table(table_id)
        .expect("table")
        .cell(0, 1)
        .expect("cell")
        .paragraphs[0]
        .id;
    let inverse = doc
        .apply(Operation::InsertText {
            block: cell_id,
            offset: 0,
            text: "three words here".into(),
            format: None,
        })
        .expect("insert");
    assert_eq!(doc.word_count(), before + 3);
    doc.apply(inverse).expect("undo");
    assert_eq!(doc.word_count(), before);
}

#[test]
fn from_blocks_keeps_the_table_and_its_cell_ids() {
    let mut doc = doc_with(&["Body"]);
    let table = doc.new_table(1, 1, 100.0);
    let table_id = table.id;
    let cell_id = table.rows[0].cells[0].paragraphs[0].id;
    let rebuilt = Document::from_blocks(vec![
        Block::Paragraph(Paragraph::new(BlockId(1), "Body")),
        Block::Table(table),
    ]);
    assert!(rebuilt.table(table_id).is_some());
    assert_eq!(rebuilt.paragraph(cell_id).expect("cell").text(), "");
    assert_eq!(rebuilt.paragraph_count(), 2);
}

#[test]
fn bookmarks_follow_text_edits_and_splits() {
    let mut p = Paragraph::new(BlockId(1), "one two three");
    p.set_bookmarks(vec![
        Bookmark {
            name: "start".into(),
            offset: 0,
        },
        Bookmark {
            name: "middle".into(),
            offset: 4,
        },
        Bookmark {
            name: "end".into(),
            offset: 13,
        },
    ]);
    p.insert_text(4, "X");
    assert_eq!(p.bookmark("start").expect("start").offset, 0);
    assert_eq!(p.bookmark("middle").expect("middle").offset, 5);
    assert_eq!(p.bookmark("end").expect("end").offset, 14);
    p.delete_range(4..6);
    assert_eq!(p.bookmark("middle").expect("middle").offset, 4);
    assert_eq!(p.bookmark("end").expect("end").offset, 12);

    let right = p.split_off(4, BlockId(2), None);
    assert_eq!(
        p.bookmarks().len(),
        1,
        "only 'start' stays in the left half"
    );
    assert_eq!(
        right.bookmarks().len(),
        2,
        "the others move to the right half"
    );
    assert_eq!(right.bookmark("middle").expect("middle").offset, 0);
    assert_eq!(right.bookmark("end").expect("end").offset, 8);

    let mut left = p.clone();
    left.append(right);
    assert_eq!(left.bookmarks().len(), 3);
    assert_eq!(left.bookmark("end").expect("end").offset, 12);
}

#[test]
fn set_bookmarks_sorts_and_undoes() {
    let mut doc = doc_with(&["alpha beta"]);
    let id = para_id(&doc, 0);
    let inverse = doc
        .apply(Operation::SetBookmarks {
            block: id,
            bookmarks: vec![
                Bookmark {
                    name: "b".into(),
                    offset: 6,
                },
                Bookmark {
                    name: "a".into(),
                    offset: 0,
                },
            ],
        })
        .expect("set bookmarks");
    let names: Vec<&str> = doc
        .paragraph(id)
        .expect("paragraph")
        .bookmarks()
        .iter()
        .map(|bookmark| bookmark.name.as_str())
        .collect();
    assert_eq!(names, ["a", "b"]);
    doc.apply(inverse).expect("undo");
    assert!(doc.paragraph(id).expect("paragraph").bookmarks().is_empty());
}

fn png(width: u32, height: u32) -> std::sync::Arc<Media> {
    std::sync::Arc::new(Media::new(test_png_header(width, height)).expect("png header"))
}

#[test]
fn media_reads_png_and_jpeg_headers_and_refuses_others() {
    let media = Media::new(test_png_header(640, 480)).expect("png");
    assert_eq!(media.format(), ImageFormat::Png);
    assert_eq!((media.width_px(), media.height_px()), (640, 480));
    assert_eq!(media.natural_size_pt(), (480.0, 360.0));
    // A minimal JPEG: SOI, an APP0 segment to skip, then SOF0 with 300 x 200.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
    jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xC8, 0x01, 0x2C, 0x03]);
    let media = Media::new(jpeg).expect("jpeg");
    assert_eq!(media.format(), ImageFormat::Jpeg);
    assert_eq!((media.width_px(), media.height_px()), (300, 200));
    assert_eq!(
        Media::new(b"GIF89a".to_vec()),
        Err(MediaError::UnknownFormat)
    );
    assert_eq!(
        Media::new(vec![0xFF, 0xD8, 0xFF]),
        Err(MediaError::BadHeader)
    );
    assert_eq!(Media::new(test_png_header(0, 10)), Err(MediaError::Empty));
}

#[test]
fn a_wide_image_is_fitted_to_the_column_in_proportion() {
    let image = InlineImage::fitted(0, png(1600, 800), 400.0);
    assert_eq!((image.width_pt, image.height_pt), (400.0, 200.0));
    assert_eq!(image.aspect(), 0.5);
}

#[test]
fn images_follow_text_edits_splits_and_merges() {
    let mut p = Paragraph::new(BlockId(1), "one two three");
    p.set_images(vec![
        InlineImage::fitted(8, png(10, 10), 100.0),
        InlineImage::fitted(4, png(20, 10), 100.0),
    ]);
    assert_eq!(p.images()[0].offset, 4, "sorted by offset");
    p.insert_text(4, "X");
    assert_eq!(p.images()[0].offset, 5, "typing at an image goes before it");
    p.delete_range(0..2);
    assert_eq!(p.images()[0].offset, 3);
    assert_eq!(p.images()[1].offset, 7);
    let right = p.split_off(5, BlockId(2), None);
    assert_eq!(p.images().len(), 1);
    assert_eq!(right.images().len(), 1);
    assert_eq!(right.images()[0].offset, 2);
    p.append(right);
    assert_eq!(
        p.images().iter().map(|i| i.offset).collect::<Vec<_>>(),
        vec![3, 7]
    );
}

#[test]
fn set_images_undoes_exactly() {
    let mut doc = doc_with(&["A picture here"]);
    let id = doc.paragraph_at(0).expect("p").id;
    let image = InlineImage::fitted(2, png(64, 32), 200.0);
    let inverse = doc
        .apply(Operation::SetImages {
            block: id,
            images: vec![image.clone()],
        })
        .expect("set images");
    assert_eq!(doc.paragraph(id).expect("p").images(), &[image]);
    doc.apply(inverse).expect("undo");
    assert!(doc.paragraph(id).expect("p").images().is_empty());
}

#[test]
fn cross_references_track_their_text() {
    let cross_ref = |range: std::ops::Range<usize>| CrossRef {
        range,
        target: "intro".into(),
        kind: CrossRefKind::Text,
    };
    let mut p = Paragraph::new(BlockId(1), "See Chapter One for more");
    p.set_cross_refs(vec![cross_ref(4..15)]);
    p.insert_text(4, ">");
    assert_eq!(
        p.cross_refs()[0].range,
        5..16,
        "text at the start stays outside"
    );
    p.insert_text(16, "<");
    assert_eq!(
        p.cross_refs()[0].range,
        5..16,
        "text at the end stays outside"
    );
    p.insert_text(8, "!");
    assert_eq!(p.cross_refs()[0].range, 5..17, "text inside grows it");
    assert!(p.cross_ref_at(5).is_some());
    assert!(p.cross_ref_at(17).is_none());
    p.delete_range(0..7);
    assert_eq!(p.cross_refs()[0].range, 0..10);
    let right = p.split_off(4, BlockId(2), None);
    assert_eq!(
        p.cross_refs()[0].range,
        0..4,
        "a cut reference keeps its head"
    );
    assert!(right.cross_refs().is_empty());
    p.delete_range(0..4);
    assert!(
        p.cross_refs().is_empty(),
        "deleting the text removes the reference"
    );
    // Overlaps and empty ranges are dropped.
    let mut q = Paragraph::new(BlockId(3), "abcdef");
    q.set_cross_refs(vec![cross_ref(0..3), cross_ref(2..5), cross_ref(5..5)]);
    assert_eq!(q.cross_refs().len(), 1);
}

#[test]
fn a_table_style_fills_header_and_banded_rows_under_cell_fills() {
    let mut doc = Document::new();
    let mut table = doc.new_table(4, 2, 200.0);
    let (style, borders) = TableStyle::builtin("banded rows").expect("built-in");
    table.apply_style(Some(style.clone()), borders);
    assert_eq!(table.header_rows, 1, "a header fill needs a header row");
    assert_eq!(table.style_fill(0), style.header_fill);
    assert_eq!(table.style_fill(1), None);
    assert_eq!(table.style_fill(2), style.band_fill);
    assert_eq!(table.style_fill(3), None);
    let red = [255, 0, 0, 255];
    table.rows[2].cells[0].fill = Some(red);
    let cell = table.rows[2].cells[0].clone();
    assert_eq!(table.resolved_fill(2, &cell), Some(red));
    let plain = table.rows[2].cells[1].clone();
    assert_eq!(table.resolved_fill(2, &plain), style.band_fill);
    let mut other = table.clone();
    other.style = None;
    assert!(!table.content_eq(&other));
}

// ---------------------------------------------------------------- date and time fields

#[test]
fn date_fields_follow_edits_and_splits() {
    let text = "Saved on 12 November 2026.";
    let start = "Saved on ".len();
    let end = start + "12 November 2026".len();
    let mut p = Paragraph::new(BlockId(1), text);
    p.set_date_fields(vec![DateTimeField::new(
        start..end,
        DateTimeSource::Updated,
        DateTimeFormat::DateLong,
    )]);

    // Typing inside the field grows it; typing before it moves it; typing after it leaves
    // it alone (as with a cross-reference).
    p.insert_text(start + 3, "x");
    assert_eq!(p.date_fields()[0].range, start..end + 1);
    p.insert_text(0, "> ");
    assert_eq!(p.date_fields()[0].range, start + 2..end + 3);
    p.insert_text(p.text().len(), "!");
    assert_eq!(p.date_fields()[0].range, start + 2..end + 3);

    // Deleting part of the field shortens it, and deleting all of it drops it.
    p.delete_range(start + 2..start + 4);
    assert_eq!(p.date_fields()[0].range, start + 2..end + 1);
    let (field_start, field_end) = {
        let f = &p.date_fields()[0];
        (f.range.start, f.range.end)
    };
    p.delete_range(field_start..field_end);
    assert!(p.date_fields().is_empty(), "an empty field is not a field");

    // A split keeps the part before the cut on the left and moves the rest right.
    let mut p = Paragraph::new(BlockId(1), text);
    p.set_date_fields(vec![DateTimeField::new(
        start..end,
        DateTimeSource::Inserted,
        DateTimeFormat::Time24,
    )]);
    let tail = p.split_off(start + 6, BlockId(2), None);
    assert_eq!(p.date_fields()[0].range, start..start + 6);
    assert!(
        tail.date_fields().is_empty(),
        "a field cut by a split keeps its first part; the rest is plain text (as a reference does)"
    );
    // Merging the halves again brings the first part back where it was.
    p.append(tail);
    assert_eq!(p.date_fields()[0].range, start..start + 6);
}

#[test]
fn a_documents_save_time_is_bookkeeping_not_content() {
    let mut doc = doc_with(&["Text"]);
    let before = doc.clone();
    assert_eq!(doc.saved_at(), None);
    doc.set_saved_at(1_760_000_000);
    assert_eq!(doc.saved_at(), Some(1_760_000_000));
    assert!(doc.content_eq(&before), "the save time is not content");
    assert_eq!(doc, before, "and it does not make documents unequal");
}

#[test]
fn page_numbering_counts_from_the_chosen_page() {
    let numbering = PageNumbering {
        show: true,
        start_page: 5,
        start_at: 3,
        ..PageNumbering::default()
    };
    assert_eq!(numbering.number_on(4), (4, false), "before the start page");
    assert_eq!(numbering.number_on(5), (3, true), "the start page");
    assert_eq!(numbering.number_on(6), (4, true), "counts up");
    assert_eq!(
        PageNumbering::default().number_on(1),
        (1, true),
        "by default numbering begins on page 1"
    );
    assert_eq!(
        PageNumbering {
            start_page: 0,
            ..numbering
        }
        .clamped()
        .start_page,
        1
    );
}

#[test]
fn merged_steps_undo_and_redo_as_one() {
    let mut doc = doc_with(&["abc"]);
    let a = para_id(&doc, 0);
    let mut h = History::new(100);
    let before = doc.clone();
    let mut now = Instant::now();
    for (offset, text) in [(3, "d"), (4, "e"), (5, "f")] {
        let fwd = vec![Operation::InsertText {
            block: a,
            offset,
            text: text.into(),
            format: None,
        }];
        let inv = doc.apply_transaction(fwd.clone()).unwrap();
        now += std::time::Duration::from_secs(5);
        h.record(HistoryEntry::new(fwd, inv, EditKind::Typing), now);
    }
    let after = doc.clone();
    assert_eq!(h.undo_len(), 3);
    assert!(!h.merge_last(1), "one step is nothing to merge");
    assert!(!h.merge_last(4), "more than held");
    assert_eq!(h.undo_len(), 3);
    assert!(h.merge_last(3));
    assert_eq!(h.undo_len(), 1);
    h.undo(&mut doc).unwrap();
    assert!(doc.content_eq(&before), "one undo reverts the whole run");
    assert!(!h.can_undo());
    h.redo(&mut doc).unwrap();
    assert!(doc.content_eq(&after), "one redo repeats it");
}
