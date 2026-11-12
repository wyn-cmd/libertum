use crate::spike::generate_document;
use crate::*;

fn state(pages: usize) -> LayoutState {
    LayoutState::new(
        PageGeometry::a4(),
        generate_document(pages, 7),
        LayoutOptions::default(),
    )
}

#[test]
fn generator_is_deterministic_and_spans_are_valid() {
    let a = generate_document(5, 1);
    let b = generate_document(5, 1);
    assert_eq!(a, b);
    for p in &a {
        for (r, _) in &p.spans {
            assert!(r.start < r.end && r.end <= p.text.len());
            assert!(p.text.is_char_boundary(r.start) && p.text.is_char_boundary(r.end));
        }
    }
}

#[test]
fn pagination_covers_every_line_exactly_once() {
    let mut s = state(20);
    let stats = s.paginate_all();
    assert!(s.is_complete());
    assert!(stats.total_pages >= 10, "pages = {}", stats.total_pages);
    let h = s.geometry().content_height();
    let mut expect = FlowPos::default();
    for page in s.pages() {
        assert_eq!(page.start, expect);
        assert!(page.used_height <= h + 8.0 + 0.1, "page overflow");
        for sl in &page.slices {
            assert_eq!(sl.para, expect.para);
            assert_eq!(sl.line_start, expect.line);
            let pl = s.paragraph_layout(sl.para).unwrap();
            if sl.line_end == pl.lines.len() {
                expect = FlowPos {
                    para: sl.para + 1,
                    line: 0,
                };
            } else {
                expect = FlowPos {
                    para: sl.para,
                    line: sl.line_end,
                };
            }
        }
    }
    assert_eq!(expect.para, s.paragraphs().len());
}

fn tabbed_doc(text: &str, stops: Option<libertum_model::TabStops>) -> libertum_model::Document {
    use libertum_model::{BlockId, Document, ParaFormat, ParaProps, Paragraph};
    Document::from_paragraphs(vec![Paragraph::new(BlockId(1), text).with_props(
        ParaProps {
            format: ParaFormat {
                tab_stops: stops,
                ..ParaFormat::default()
            },
            ..ParaProps::default()
        },
    )])
}

#[test]
fn tabs_advance_to_default_stops_every_half_inch() {
    let doc = tabbed_doc("A\tB\tC", None);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let margin = s.geometry().margins.left;
    let x = |offset| s.caret_rect(0, offset, 1.0).expect("caret").x;
    let before = x(1);
    let b = x(2);
    let c = x(4);
    assert!(b > before, "the tab advances the following text");
    assert!(
        (b - (margin + 36.0)).abs() < 1.0,
        "first default stop: {} from the margin",
        b - margin
    );
    assert!(
        (c - (margin + 72.0)).abs() < 1.0,
        "second default stop: {} from the margin",
        c - margin
    );
}

#[test]
fn tab_stops_are_measured_from_the_margin_not_the_indent() {
    use libertum_model::{BlockId, Document, ParaFormat, ParaProps, Paragraph};
    let para = Paragraph::new(BlockId(1), "A\tB").with_props(ParaProps {
        format: ParaFormat {
            indent_first_pt: Some(36.0),
            ..ParaFormat::default()
        },
        ..ParaProps::default()
    });
    let doc = Document::from_paragraphs(vec![para]);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let margin = s.geometry().margins.left;
    let x = s.caret_rect(0, 2, 1.0).expect("caret").x;
    assert!(
        (x - (margin + 72.0)).abs() < 1.0,
        "B should sit on the 72 pt stop after a 36 pt indent, got {} pt",
        x - margin
    );
}

#[test]
fn explicit_tab_stops_align_right_centre_and_decimal() {
    use libertum_model::{TabStop, TabStops};

    let margin = PageGeometry::a4().margins.left;
    let caret_close = |s: &LayoutState, offset: usize, expected: f32| {
        let x = s.caret_rect(0, offset, 1.0).expect("caret").x;
        assert!(
            (x - expected).abs() < 1.0,
            "caret at {offset} is {x}, expected {expected}"
        );
    };

    let mut right = TabStops::new();
    assert!(right.push(TabStop::right(144.0)));
    let doc = tabbed_doc("Total\t1,234.50", Some(right));
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    caret_close(&s, 14, margin + 144.0);

    let mut centre = TabStops::new();
    assert!(centre.push(TabStop::centre(216.0)));
    let doc = tabbed_doc("Status\tShipped", Some(centre));
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let start = s.caret_rect(0, 7, 1.0).expect("caret").x;
    let end = s.caret_rect(0, 14, 1.0).expect("caret").x;
    assert!(
        ((start + end) / 2.0 - (margin + 216.0)).abs() < 1.0,
        "centred text midpoint {} vs {}",
        (start + end) / 2.0 - margin,
        216.0
    );

    let mut decimal = TabStops::new();
    assert!(decimal.push(TabStop::decimal(120.0)));
    let doc = tabbed_doc("Price\t12.50", Some(decimal));
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    caret_close(&s, 8, margin + 120.0);
}

#[test]
fn a_tab_past_the_right_margin_stays_stable() {
    let mut stops = libertum_model::TabStops::new();
    assert!(stops.push(libertum_model::TabStop::left(900.0)));
    let doc = tabbed_doc("Some text before\tand after", Some(stops));
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    assert!(s.page_count() >= 1);
    assert!(
        s.paragraphs()[0].text.contains('\t'),
        "the tab character is kept in the model text"
    );
}

#[test]
fn paragraph_borders_draw_four_lines_around_the_text() {
    use libertum_model::{BlockId, Borders, Document, ParaFormat, ParaProps, Paragraph};

    let bordered = Paragraph::new(BlockId(1), "Bordered paragraph").with_props(ParaProps {
        format: ParaFormat {
            borders: Some(Borders::line(1.0)),
            ..ParaFormat::default()
        },
        ..ParaProps::default()
    });
    let doc =
        Document::from_paragraphs(vec![bordered, Paragraph::new(BlockId(2), "No border here")]);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let rects: Vec<(f32, f32, f32, f32)> = s
        .page_decorations(0)
        .into_iter()
        .filter_map(|d| match d {
            Decoration::Rect {
                x,
                y,
                width,
                height,
                ..
            } => Some((x, y, width, height)),
            _ => None,
        })
        .collect();
    assert_eq!(rects.len(), 4, "four sides of one box: {rects:?}");
    let horizontal = rects.iter().filter(|(_, _, w, h)| w > h).count();
    let vertical = rects.iter().filter(|(_, _, w, h)| h > w).count();
    assert_eq!((horizontal, vertical), (2, 2), "{rects:?}");

    // The box surrounds the text: inside the page, left of the right margin, above the text.
    let geo = s.geometry();
    let text_left = geo.margins.left;
    let text_right = text_left + geo.content_width();
    for (x, y, w, h) in &rects {
        assert!(
            *x >= text_left - 6.0 && x + w <= text_right + 6.0,
            "{rects:?}"
        );
        assert!(*y >= geo.margins.top - 6.0, "{rects:?}");
        assert!(*h > 0.0 && *w > 0.0);
    }
    let top = rects
        .iter()
        .filter(|(_, _, w, h)| w > h)
        .map(|(_, y, _, _)| *y)
        .fold(f32::INFINITY, f32::min);
    assert!(top < geo.margins.top + 20.0, "top line near the first line");
}

#[test]
fn set_geometry_repaginates_and_matches_a_fresh_layout() {
    let paragraphs = generate_document(20, 3);
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        paragraphs.clone(),
        LayoutOptions::default(),
    );
    s.paginate_all();
    let before = s.page_count();
    let letter = PageGeometry::letter();
    assert!(s.set_geometry(letter), "a new geometry is a change");
    assert_eq!(s.page_count(), 0, "pages are dropped");
    assert!(!s.is_complete());

    s.paginate_all();
    let mut fresh = LayoutState::new(letter, paragraphs, LayoutOptions::default());
    fresh.paginate_all();
    assert!(before > 0);
    assert_eq!(s.page_count(), fresh.page_count());
    for page in 0..s.page_count() {
        assert_eq!(
            s.page_content_hash(page),
            fresh.page_content_hash(page),
            "page {page} differs from a from-scratch layout"
        );
    }
    assert_eq!(s.pages().len(), s.page_count());
    assert!(!s.set_geometry(letter), "the same geometry is not a change");
}

#[test]
fn set_geometry_with_wider_margins_reflows_every_page() {
    let paragraphs = generate_document(20, 4);
    let mut s = LayoutState::new(PageGeometry::a4(), paragraphs, LayoutOptions::default());
    s.paginate_all();
    let narrow = s.page_count();
    let mut wide = PageGeometry::a4();
    wide.margins = Margins::uniform(144.0);
    assert!(s.set_geometry(wide));
    s.paginate_all();
    assert!(
        s.page_count() > narrow,
        "half the content width must need more pages: {} vs {narrow}",
        s.page_count()
    );
}

#[test]
fn incremental_repagination_matches_full_and_converges() {
    let mut s = state(500);
    s.paginate_all();
    assert!(s.page_count() >= 400, "pages = {}", s.page_count());
    let mid = s.pages()[s.page_count() / 2].start.para + 1;
    let before = s.page_count();
    let stats = s
        .edit_paragraph(mid, |p| p.text.insert(0, 'x'))
        .expect("paragraph exists");
    assert!(stats.converged, "{stats:?}");
    assert!(stats.pages_rebuilt <= 3, "{stats:?}");
    assert!(stats.paragraphs_laid_out <= 2, "{stats:?}");
    assert_eq!(s.page_count(), before);

    // Ground truth: paginate the edited document from scratch.
    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        s.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.paginate_all();
    assert_eq!(s.pages(), fresh.pages());
}

#[test]
fn large_edit_that_reflows_many_pages_still_matches_full() {
    let mut s = state(60);
    s.paginate_all();
    let target = s.paragraphs().len() / 3;
    let long = "word ".repeat(2_000);
    let stats = s
        .edit_paragraph(target, |p| p.text.push_str(&long))
        .expect("exists");
    assert!(stats.pages_rebuilt >= 5, "{stats:?}");
    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        s.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.paginate_all();
    assert_eq!(s.pages(), fresh.pages());
}

#[test]
fn partial_pagination_then_edit_beyond_prefix_is_cheap() {
    let mut s = state(40);
    let st = s.paginate_until(1);
    assert_eq!(st.total_pages, 1);
    assert!(!s.is_complete());
    let last = s.paragraphs().len() - 1;
    let stats = s
        .edit_paragraph(last, |p| p.text.push('!'))
        .expect("exists");
    assert_eq!(stats.pages_rebuilt, 0);
    s.paginate_all();
    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        s.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.paginate_all();
    assert_eq!(s.pages(), fresh.pages());
}

#[test]
fn page_hash_changes_only_for_edited_page() {
    let mut s = state(30);
    s.paginate_all();
    let hashes: Vec<u64> = (0..s.page_count())
        .map(|i| s.page_content_hash(i).unwrap())
        .collect();
    let mid = s.pages()[s.page_count() / 2].start.para + 1;
    s.edit_paragraph(mid, |p| p.text.insert(0, 'y'));
    let after: Vec<u64> = (0..s.page_count())
        .map(|i| s.page_content_hash(i).unwrap())
        .collect();
    let changed = hashes.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert!((1..=3).contains(&changed), "changed = {changed}");
}

#[test]
fn missing_font_falls_back_without_panicking() {
    let opts = LayoutOptions {
        font_preference: vec!["No Such Font 12345".into()],
        ..Default::default()
    };
    let mut s = LayoutState::new(PageGeometry::a4(), generate_document(3, 3), opts);
    let report = s.font_report();
    assert!(report.using_fallback && report.resolved_family.is_none());
    s.paginate_all();
    assert!(s.page_count() >= 1);
}

#[test]
fn empty_document_and_empty_paragraph() {
    let mut s = LayoutState::new(PageGeometry::a4(), vec![], LayoutOptions::default());
    s.paginate_all();
    assert_eq!(s.page_count(), 0);
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![SpikeParagraph::new(0, ""), SpikeParagraph::new(1, "")],
        LayoutOptions::default(),
    );
    s.paginate_all();
    assert_eq!(s.page_count(), 1);
    assert_eq!(s.pages()[0].slices.len(), 2);
}

#[test]
fn hit_test_and_caret_round_trip() {
    let mut s = state(3);
    s.paginate_all();
    if s.font_report().family_count == 0 {
        return; // no fonts on this machine: nothing to measure
    }
    let para = s.pages()[1].slices[0].para;
    let hit_start = s.paragraphs()[para].text.len() / 2;
    // Snap to a char boundary.
    let mut off = hit_start;
    while !s.paragraphs()[para].text.is_char_boundary(off) {
        off += 1;
    }
    let caret = s.caret_rect(para, off, 1.0).expect("caret");
    assert!(caret.height > 5.0);
    let hit = s
        .hit_test(caret.page, caret.x + 0.5, caret.y + caret.height * 0.5)
        .expect("hit");
    assert_eq!(hit.para, para);
    // Clicking just right of the caret lands on the same cluster boundary (or the next).
    assert!(
        hit.byte_offset.abs_diff(off) <= 4,
        "{} vs {off}",
        hit.byte_offset
    );
}

/// Spike C evidence: parley 0.12 places inline boxes (images, footnote markers).
#[test]
fn parley_inline_boxes_are_positioned() {
    use parley::{
        FontContext, InlineBox, InlineBoxKind, LayoutContext, PositionedLayoutItem, VerticalAlign,
    };
    let mut fcx = FontContext::new();
    let mut lcx: LayoutContext<Brush> = LayoutContext::new();
    let text = "Hello world, see the figure here and then continue.";
    let mut b = lcx.ranged_builder(&mut fcx, text, 1.0, false);
    b.push_default(parley::StyleProperty::FontSize(12.0));
    b.push_inline_box(InlineBox {
        id: 42,
        kind: InlineBoxKind::InFlow,
        index: 24,
        width: 30.0,
        height: 20.0,
        baseline: None,
        vertical_align: VerticalAlign::BASELINE,
    });
    let mut layout: parley::Layout<Brush> = b.build(text);
    layout.break_all_lines(Some(400.0));
    let mut found = None;
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(ib) = item {
                found = Some(ib);
            }
        }
    }
    if fcx_has_fonts() {
        let ib = found.expect("inline box positioned");
        assert_eq!(ib.id, 42);
        assert!((ib.width - 30.0).abs() < 0.01 && (ib.height - 20.0).abs() < 0.01);
        assert!(ib.x > 10.0, "box should sit mid-line, x = {}", ib.x);
    }
}

fn fcx_has_fonts() -> bool {
    parley::FontContext::new()
        .collection
        .family_names()
        .next()
        .is_some()
}

/// The plan hands snapshots to a pagination worker thread, so the state must be Send + Sync.
#[test]
fn layout_state_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<LayoutState>();
    check::<ParagraphLayout>();
}

#[test]
fn overlong_word_wraps_instead_of_overflowing() {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![SpikeParagraph::new(0, "x".repeat(600))],
        LayoutOptions::default(),
    );
    s.paginate_all();
    if s.font_report().family_count == 0 {
        return;
    }
    let pl = s.paragraph_layout(0).expect("layout");
    assert!(pl.lines.len() > 1, "600 unbroken chars must wrap");
}

// ---------------------------------------------------------------- model integration

#[test]
fn from_document_matches_snapshot_layout() {
    use libertum_model::{BlockId, Document, Paragraph};
    let doc = Document::from_paragraphs(vec![
        Paragraph::new(BlockId(1), "The quick brown fox jumps over the lazy dog."),
        Paragraph::new(BlockId(2), "Second paragraph with a little more text."),
    ]);
    let mut from_doc =
        LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    from_doc.paginate_all();
    assert_eq!(from_doc.paragraph_count(), doc.len());

    let snaps: Vec<ParagraphSnapshot> = doc
        .body()
        .iter()
        .filter_map(|b| b.as_paragraph())
        .map(|p| ParagraphSnapshot::from_model(p, &doc.styles))
        .collect();
    let mut from_snaps = LayoutState::new(PageGeometry::a4(), snaps, LayoutOptions::default());
    from_snaps.paginate_all();
    assert_eq!(from_doc.pages(), from_snaps.pages());
}

#[test]
fn structural_edits_match_full_repagination() {
    let mut s = state(20);
    s.paginate_all();
    let index = s.paragraphs().len() / 2;
    let snapshot = ParagraphSnapshot::new(9_999_999, "inserted paragraph ".repeat(20));
    s.insert_paragraph(index, snapshot);

    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        s.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.paginate_all();
    assert_eq!(s.pages(), fresh.pages(), "after insert");

    s.remove_paragraph(index);
    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        s.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.paginate_all();
    assert_eq!(s.pages(), fresh.pages(), "after remove");
}

#[test]
fn structural_edit_before_resume_stops_at_old_prefix() {
    let mut s = state(40);
    s.paginate_until(1);
    assert!(!s.is_complete());
    let index = 1;
    s.insert_paragraph(
        index,
        ParagraphSnapshot::new(9_999_999, "new first paragraph"),
    );
    assert!(!s.is_complete(), "must not paginate the whole document");
    s.paginate_all();
    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        s.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.paginate_all();
    assert_eq!(s.pages(), fresh.pages());
}

#[test]
fn replace_paragraph_matches_edit_paragraph() {
    let mut a = state(10);
    a.paginate_all();
    let index = 3;
    let mut snapshot = a.paragraphs()[index].clone();
    snapshot.text.push_str(" extra words changed here");
    snapshot.version += 1;
    let stats = a
        .replace_paragraph(index, snapshot.clone())
        .expect("exists");
    assert!(stats.pages_rebuilt >= 1);

    let mut b = state(10);
    b.paginate_all();
    b.edit_paragraph(index, |p| *p = snapshot).expect("exists");
    assert_eq!(a.pages(), b.pages());
}

#[test]
fn selection_rects_map_to_pages() {
    let mut s = state(3);
    s.paginate_all();
    if s.font_report().family_count == 0 {
        return;
    }
    let Some(slice) = s.pages()[0]
        .slices
        .iter()
        .find(|s| s.line_end - s.line_start >= 1)
    else {
        return;
    };
    let text_len = s.paragraphs()[slice.para].text.len();
    if text_len < 2 {
        return;
    }
    let rects = s.selection_rects(slice.para, 0..text_len / 2);
    assert!(!rects.is_empty(), "a non-empty range must produce geometry");
    assert!(rects.iter().all(|r| r.page < s.page_count()));
    assert!(rects.iter().any(|r| r.page == 0));
    assert!(rects.iter().all(|r| r.width > 0.0 && r.height > 0.0));
    // Collapsed ranges produce nothing.
    assert!(s.selection_rects(slice.para, 5..5).is_empty());
}

// ---------------------------------------------------------------- page number placement

/// (x, y) of the page-number decoration showing `number` on `page`.
fn number_at(s: &LayoutState, page: usize, number: u32) -> Option<(f32, f32)> {
    s.page_decorations(page).into_iter().find_map(|d| match d {
        Decoration::Text { text, x, y, .. } if text == number.to_string() => Some((x, y)),
        _ => None,
    })
}

/// Two pages: "One" with a page break before "Two".
fn two_page_state() -> LayoutState {
    let mut first = para(1, "One");
    first.page_break_before = false;
    let mut second = para(2, "Two");
    second.page_break_before = true;
    LayoutState::new(
        PageGeometry::a4(),
        vec![first, second],
        LayoutOptions::default(),
    )
}

#[test]
fn page_numbers_sit_where_the_section_says_and_mirror_on_facing_pages() {
    use libertum_model::{PageNumberHorizontal, PageNumberVertical};
    let mut s = two_page_state();
    s.set_sections(vec![Section {
        page_numbers: Some(PageNumberPos {
            vertical: PageNumberVertical::Top,
            horizontal: PageNumberHorizontal::Right,
            mirror: true,
        }),
        ..Section::default()
    }]);
    s.paginate_all();
    let geo = *s.geometry();
    let (x0, y0) = number_at(&s, 0, 1).expect("page 1 number");
    let (x1, y1) = number_at(&s, 1, 2).expect("page 2 number");
    assert!(x0 > geo.width_pt * 0.5, "first page number on the right");
    assert!(x1 < geo.width_pt * 0.5, "second page number mirrored left");
    assert!(y0 < geo.margins.top && y1 < geo.margins.top, "top margin");
}

#[test]
fn page_numbers_can_start_at_zero() {
    let mut s = two_page_state();
    s.set_sections(vec![Section {
        page_numbers: Some(PageNumberPos::BOTTOM_CENTRE),
        first_page_number: Some(0),
        ..Section::default()
    }]);
    s.paginate_all();
    let geo = *s.geometry();
    let (_, y0) = number_at(&s, 0, 0).expect("page 0");
    assert!(y0 > geo.height_pt - geo.margins.bottom, "bottom margin");
    assert!(number_at(&s, 1, 1).is_some(), "the next page is 1");
}

#[test]
fn hiding_the_first_page_keeps_numbering_after_it() {
    let mut s = two_page_state();
    s.set_sections(vec![Section {
        page_numbers: Some(PageNumberPos::BOTTOM_CENTRE),
        hide_first_page: true,
        ..Section::default()
    }]);
    s.paginate_all();
    assert!(number_at(&s, 0, 1).is_none(), "first page number hidden");
    assert!(
        number_at(&s, 1, 2).is_some(),
        "the second page still carries its number"
    );
}

// ---------------------------------------------------------------- page breaks, sections, TOC

use libertum_model::{Align, PageNumberPos, Section};

fn para(id: u64, text: &str) -> ParagraphSnapshot {
    ParagraphSnapshot::new(id, text)
}

/// Title page, a contents page with one TOC entry, then chapters of `body_paras` paragraphs
/// each. Section 1 (numbered from 1 at the bottom centre) starts at the first chapter.
fn book_state(chapters: usize, body_paras: usize) -> LayoutState {
    let filler =
        "All happy families are alike; each unhappy family is unhappy in its own way. ".repeat(6);
    let mut paras = vec![para(1, "Title"), {
        let mut p = para(2, "Contents");
        p.page_break_before = true;
        p
    }];
    let mut next = 100;
    let mut entries = Vec::new();
    let mut body = Vec::new();
    for c in 0..chapters {
        let heading_id = next;
        next += 1;
        let mut h = para(heading_id, &format!("Chapter {}", c + 1));
        h.page_break_before = true;
        h.section_start = c == 0;
        h.align = Align::Centre;
        body.push(h);
        for _ in 0..body_paras {
            body.push(para(next, &filler));
            next += 1;
        }
        let mut e = para(10 + c as u64, &format!("Chapter {}", c + 1));
        e.toc_target = Some(heading_id);
        entries.push(e);
    }
    paras.extend(entries);
    paras.extend(body);
    let mut s = LayoutState::new(PageGeometry::a4(), paras, LayoutOptions::default());
    s.set_sections(vec![
        Section::default(),
        Section {
            page_numbers: Some(PageNumberPos::BOTTOM_CENTRE),
            first_page_number: Some(1),
            ..Section::default()
        },
    ]);
    s
}

#[test]
fn page_and_section_breaks_start_new_pages() {
    let mut s = book_state(2, 1);
    s.paginate_all();
    // Title, contents, chapter 1, chapter 2: one page each.
    assert_eq!(s.page_count(), 4);
    let firsts: Vec<usize> = s.pages().iter().map(|p| p.start.para).collect();
    assert_eq!(s.paragraphs()[firsts[1]].text, "Contents");
    assert_eq!(s.paragraphs()[firsts[2]].text, "Chapter 1");
    assert_eq!(s.paragraphs()[firsts[3]].text, "Chapter 2");
}

#[test]
fn numbering_restarts_at_the_first_chapter_and_only_shows_there() {
    let mut s = book_state(2, 1);
    s.paginate_all();
    let numbers: Vec<(usize, u32)> = s.pages().iter().map(|p| (p.section, p.number)).collect();
    assert_eq!(numbers, vec![(0, 1), (0, 2), (1, 1), (1, 2)]);
    let has_number = |page: usize| {
        s.page_decorations(page)
            .iter()
            .any(|d| matches!(d, Decoration::Text { y, .. } if *y > s.geometry().height_pt - s.geometry().margins.bottom))
    };
    assert!(!has_number(0), "no number on the title page");
    assert!(has_number(2), "chapter pages are numbered at the bottom");
}

#[test]
fn toc_entries_get_their_heading_page_number_and_a_leader() {
    let mut s = book_state(3, 6);
    s.paginate_all();
    let contents_page = 1;
    let decorations = s.page_decorations(contents_page);
    let leaders = decorations
        .iter()
        .filter(|d| matches!(d, Decoration::Leader { .. }))
        .count();
    assert_eq!(leaders, 3, "one leader per entry");
    // Chapter 1 is page 1 of section 1, and later chapters land on later pages.
    let n1 = s.page_number_of_block(100).expect("chapter 1 paginated");
    let n2 = s.page_number_of_block(107).expect("chapter 2 paginated");
    assert_eq!(n1, 1);
    assert!(n2 > n1);
    // Numbers are right-aligned at the right margin.
    let right = s.geometry().margins.left + s.geometry().content_width();
    for d in &decorations {
        if let Decoration::Text { layout, x, .. } = d {
            assert!((x + layout.layout.width() - right).abs() < 0.5);
        }
    }
}

#[test]
fn toc_page_repaints_when_a_chapter_moves() {
    let mut s = book_state(2, 2);
    s.paginate_all();
    let before = s.page_content_hash(1);
    // Grow chapter 1 by a page: chapter 2 moves, so the contents page must change too.
    let filler = "More words to push the next chapter onto a later page. ".repeat(80);
    let index = s
        .paragraphs()
        .iter()
        .position(|p| p.id == 101)
        .expect("chapter 1 body");
    s.edit_paragraph(index, |p| p.text.push_str(&filler));
    assert_ne!(s.page_content_hash(1), before);
    // Renumbering after the edit kept the counts consistent.
    let numbers: Vec<u32> = s.pages().iter().skip(2).map(|p| p.number).collect();
    assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "{numbers:?}");
}

#[test]
fn removing_a_page_break_pulls_the_paragraph_back() {
    let mut paras = vec![para(1, "First"), para(2, "Second")];
    paras[1].page_break_before = true;
    let mut s = LayoutState::new(PageGeometry::a4(), paras, LayoutOptions::default());
    s.paginate_all();
    assert_eq!(s.page_count(), 2);
    let mut fixed = s.paragraphs()[1].clone();
    fixed.page_break_before = false;
    fixed.version += 1;
    s.replace_paragraph(1, fixed);
    assert_eq!(s.page_count(), 1);
}

#[test]
fn centred_lines_are_offset_and_space_before_survives_a_break() {
    let mut title = para(1, "Title");
    title.align = Align::Centre;
    title.space_before_pt = 200.0;
    let mut s = LayoutState::new(PageGeometry::a4(), vec![title], LayoutOptions::default());
    s.paginate_all();
    let pl = s.paragraph_layout(0).expect("layout");
    let line = pl.layout.get(0).expect("line");
    let m = line.metrics();
    let content = s.geometry().content_width();
    assert!(
        (m.offset * 2.0 + m.advance - content).abs() < 1.0,
        "centred"
    );
    // Space before at the top of the document is kept, which places a title lower.
    assert!(s.pages()[0].slices[0].y >= 200.0);
}

#[test]
fn top_right_numbers_sit_in_the_header() {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![para(1, "Manuscript")],
        LayoutOptions::default(),
    );
    s.set_sections(vec![Section {
        page_numbers: Some(PageNumberPos::TOP_RIGHT),
        ..Section::default()
    }]);
    s.paginate_all();
    let d = s.page_decorations(0);
    let Some(Decoration::Text { layout, x, y, .. }) = d.first() else {
        panic!("page number expected");
    };
    let geo = s.geometry();
    assert!(*y < geo.margins.top);
    assert!((x + layout.layout.width() - (geo.margins.left + geo.content_width())).abs() < 0.5);
}

// ---------------------------------------------------------------- headers and footers

use libertum_model::{HeaderFooter, HeadersFooters, HfKind};

fn with_default(side: HfKind, text: &str, section: &mut Section) {
    *section.header_footer_mut(side) = HeadersFooters::with_default(text);
}

/// The text and position of a header or footer decoration on `page`.
fn furniture_at(s: &LayoutState, page: usize, text: &str) -> Option<(f32, f32, f32)> {
    s.page_decorations(page).into_iter().find_map(|d| match d {
        Decoration::Text {
            text: t,
            x,
            y,
            layout,
            ..
        } if t == text => Some((x, y, layout.height)),
        _ => None,
    })
}

/// Three pages, each starting with a page break.
fn three_page_state() -> LayoutState {
    let mut paras = vec![para(1, "One")];
    for (i, text) in ["Two", "Three"].iter().enumerate() {
        let mut p = para(i as u64 + 2, text);
        p.page_break_before = true;
        paras.push(p);
    }
    LayoutState::new(PageGeometry::a4(), paras, LayoutOptions::default())
}

#[test]
fn headers_and_footers_are_drawn_at_their_distances() {
    let mut s = two_page_state();
    let mut section = Section::default();
    with_default(HfKind::Header, "Draft", &mut section);
    with_default(HfKind::Footer, "Page {page}", &mut section);
    s.set_sections(vec![section]);
    s.paginate_all();
    let geo = *s.geometry();
    let (x, y, _) = furniture_at(&s, 0, "Draft").expect("header on page 1");
    assert!((x - geo.margins.left).abs() < 0.5, "aligned to the margin");
    assert!((y - 36.0).abs() < 0.5, "half an inch from the top");
    // The field is filled in per page.
    let (_, y1, h1) = furniture_at(&s, 0, "Page 1").expect("footer on page 1");
    assert!(
        y1 + h1 <= geo.height_pt - 36.0 + 0.5,
        "the footer sits above its distance line"
    );
    assert!(
        y1 > geo.height_pt - geo.margins.bottom,
        "in the bottom margin"
    );
    assert!(furniture_at(&s, 1, "Page 2").is_some(), "numbered per page");
}

#[test]
fn a_header_taller_than_the_margin_pushes_the_body_down() {
    let long =
        "A very long running header that wraps onto several lines in the top margin. ".repeat(3);
    let mut s = two_page_state();
    let base = *s.base_geometry();
    let mut section = Section::default();
    with_default(HfKind::Header, &long, &mut section);
    assert!(s.set_sections(vec![section]), "the body geometry changed");
    s.paginate_all();
    let geo = *s.geometry();
    assert!(
        geo.margins.top > base.margins.top,
        "the header pushes the body down: {} vs {}",
        geo.margins.top,
        base.margins.top
    );
    assert!(
        geo.content_height() < base.content_height(),
        "less room for body text, so no overlap"
    );
    let (_, y, h) = furniture_at(&s, 0, &long).expect("header layout");
    assert!(
        y + h <= geo.margins.top + 0.5,
        "header ends before the body starts"
    );
    // The first body line starts inside the (now smaller) text area.
    let line_top = geo.margins.top + s.pages()[0].slices[0].y;
    assert!(line_top >= geo.margins.top);
}

#[test]
fn a_footer_taller_than_the_margin_pushes_the_body_up() {
    let long = "A long footer that needs more room than the bottom margin leaves. ".repeat(3);
    let mut s = two_page_state();
    let base = *s.base_geometry();
    let mut section = Section::default();
    with_default(HfKind::Footer, &long, &mut section);
    assert!(s.set_sections(vec![section]), "the body geometry changed");
    let geo = *s.geometry();
    assert!(geo.margins.bottom > base.margins.bottom);
    assert!(
        (geo.margins.top - base.margins.top).abs() < 0.01,
        "top unchanged"
    );
}

#[test]
fn an_unused_first_page_header_reserves_no_room() {
    let long = "A long first-page header that is not in use. ".repeat(6);
    let mut s = two_page_state();
    let base = *s.base_geometry();
    let mut section = Section::default();
    with_default(HfKind::Header, "Short", &mut section);
    section.headers.first = Some(HeaderFooter::new(&long));
    section.headers.different_first = false;
    assert!(!s.set_sections(vec![section.clone()]), "no reflow");
    assert!(
        (s.geometry().margins.top - base.margins.top).abs() < 0.01,
        "a switched-off variant takes no space"
    );
    // Switching it on does take space.
    section.headers.different_first = true;
    assert!(s.set_sections(vec![section]), "the tall header now counts");
    assert!(s.geometry().margins.top > base.margins.top);
}

#[test]
fn first_and_even_pages_pick_their_own_header() {
    let mut s = three_page_state();
    let mut section = Section::default();
    let set = &mut section.headers;
    set.default = Some(HeaderFooter::new("Every page"));
    set.first = Some(HeaderFooter::new("First page"));
    set.even = Some(HeaderFooter::new("Even page"));
    set.different_first = true;
    set.different_even_odd = true;
    s.set_sections(vec![section.clone()]);
    s.paginate_all();
    assert!(furniture_at(&s, 0, "First page").is_some(), "page 1");
    assert!(furniture_at(&s, 1, "Even page").is_some(), "page 2");
    assert!(furniture_at(&s, 2, "Every page").is_some(), "page 3");
    assert!(furniture_at(&s, 0, "Every page").is_none());
    // A variant with no text of its own falls back to the default, rather than blanking.
    let mut fallback = section.clone();
    fallback.headers.first = None;
    s.set_sections(vec![fallback]);
    assert!(furniture_at(&s, 0, "Every page").is_some());
}

#[test]
fn changing_header_text_repaints_its_strip_without_re_paginating() {
    let mut s = two_page_state();
    let mut section = Section::default();
    with_default(HfKind::Header, "Draft", &mut section);
    s.set_sections(vec![section.clone()]);
    s.paginate_all();
    let before = s.page_content_hash(0).expect("hash");
    // The replacement has the same height, so the body does not move.
    with_default(HfKind::Header, "Final", &mut section);
    assert!(
        !s.set_sections(vec![section]),
        "same height: no re-pagination"
    );
    assert_eq!(s.page_count(), 2, "pages survive");
    assert_ne!(s.page_content_hash(0), Some(before), "the page repaints");
    // The header is an item on the page, so its strip is invalidated.
    let in_header = s
        .page_items(0)
        .iter()
        .any(|item| item.top < 36.0 && item.bottom > 30.0);
    assert!(in_header, "the header strip is listed");
}

#[test]
fn a_header_field_shows_each_pages_number() {
    let mut s = three_page_state();
    let mut section = Section::default();
    with_default(HfKind::Header, "{page} / {pages}", &mut section);
    s.set_sections(vec![section]);
    s.paginate_all();
    for page in 0..3 {
        let expected = format!("{} / 3", page + 1);
        assert!(
            furniture_at(&s, page, &expected).is_some(),
            "page {page} shows {expected}"
        );
    }
}

// ------------------------------------------------------------ paragraph formatting

/// The generated document with Word's defaults switched on: widow control everywhere, and
/// headings (the generator's paragraphs with 18 pt before) kept with the next paragraph and
/// never split.
fn kept_paragraphs(pages: usize) -> Vec<ParagraphSnapshot> {
    let mut paras = generate_document(pages, 11);
    for p in &mut paras {
        p.widow_control = true;
        if p.space_before_pt == 18.0 {
            p.keep_with_next = true;
            p.keep_together = true;
        }
    }
    paras
}

/// Checks the pagination rules over every page of `s`.
fn assert_pagination_rules(s: &LayoutState) {
    let lines = |para: usize| s.paragraph_layout(para).map_or(1, |pl| pl.lines.len());
    for (i, page) in s.pages().iter().enumerate() {
        let last_page = i + 1 == s.page_count();
        for (k, sl) in page.slices.iter().enumerate() {
            let p = &s.paragraphs()[sl.para];
            let n = lines(sl.para);
            let at_top = k == 0;
            if p.widow_control && n >= 4 {
                // No orphan: a paragraph that starts on this page and continues has at least
                // two lines here (unless it is alone on the page).
                if sl.line_start == 0 && sl.line_end < n && !at_top {
                    assert!(sl.line_end >= 2, "orphan on page {i}: {sl:?}");
                }
                // No widow: a continued paragraph brings at least two lines.
                if sl.line_start > 0 && sl.line_end == n {
                    assert!(n - sl.line_start >= 2, "widow on page {i}: {sl:?}");
                }
            }
            if p.keep_together && sl.line_start == 0 && sl.line_end < n && !at_top {
                panic!("kept-together paragraph split on page {i}: {sl:?}");
            }
            // Keep with next: the next paragraph starts on the same page, unless this one
            // opens the page (the chain would not fit anywhere).
            let is_last = k + 1 == page.slices.len();
            if p.keep_with_next && is_last && sl.line_end == n && !at_top && !last_page {
                let next = &s.pages()[i + 1];
                let next_para_starts_there = next.start.para == sl.para + 1;
                let next_forces = s
                    .paragraphs()
                    .get(sl.para + 1)
                    .is_some_and(ParagraphSnapshot::forces_new_page);
                assert!(
                    !next_para_starts_there || next_forces,
                    "heading {} left alone at the bottom of page {i}",
                    sl.para
                );
            }
        }
    }
}

#[test]
fn widow_orphan_and_keep_rules_hold_on_every_page() {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        kept_paragraphs(60),
        LayoutOptions::default(),
    );
    s.paginate_all();
    assert!(s.page_count() >= 40, "pages = {}", s.page_count());
    assert_pagination_rules(&s);
    // The same document without the rules does break some of them, so the check above is
    // not vacuous.
    let mut loose = kept_paragraphs(60);
    for p in &mut loose {
        p.widow_control = false;
        p.keep_with_next = false;
        p.keep_together = false;
    }
    let mut free = LayoutState::new(PageGeometry::a4(), loose, LayoutOptions::default());
    free.paginate_all();
    let breaks =
        |st: &LayoutState| -> Vec<FlowPos> { st.pages().iter().map(|p| p.start).collect() };
    assert_ne!(
        breaks(&s),
        breaks(&free),
        "the rules moved some page breaks"
    );
}

#[test]
fn incremental_repagination_with_keep_rules_matches_full() {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        kept_paragraphs(80),
        LayoutOptions::default(),
    );
    s.paginate_all();
    // Edit a few paragraphs spread through the document, including one just after a
    // heading (whose keep-with-next depends on it).
    let heading = s
        .paragraphs()
        .iter()
        .position(|p| p.keep_with_next && p.id > 200)
        .expect("a heading");
    for index in [
        heading + 1,
        s.paragraph_count() / 3,
        s.paragraph_count() / 2,
    ] {
        let text = "More words to shift the lines about. ".repeat(4);
        s.edit_paragraph(index, |p| p.text.push_str(&text))
            .expect("paragraph exists");
        let mut fresh = LayoutState::new(
            PageGeometry::a4(),
            s.paragraphs().to_vec(),
            LayoutOptions::default(),
        );
        fresh.paginate_all();
        assert_eq!(s.pages(), fresh.pages(), "after editing paragraph {index}");
    }
    assert_pagination_rules(&s);
}

#[test]
fn indents_narrow_the_lines_and_move_the_caret() {
    let text =
        "All happy families are alike; each unhappy family is unhappy in its own way. ".repeat(4);
    let plain = para(1, &text);
    let mut indented = para(2, &text);
    indented.indent_left_pt = 72.0;
    indented.indent_right_pt = 36.0;
    indented.indent_first_pt = 36.0;
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![plain, indented],
        LayoutOptions::default(),
    );
    s.paginate_all();
    let geo = *s.geometry();
    let a = s.paragraph_layout(0).expect("plain");
    let b = s.paragraph_layout(1).expect("indented");
    assert_eq!(b.x_offset, 72.0);
    assert!(b.lines.len() > a.lines.len(), "narrower lines wrap more");
    // The caret at the start sits at the margin plus the left and first-line indents.
    let caret = s.caret_rect(1, 0, 1.0).expect("caret");
    assert!(
        (caret.x - (geo.margins.left + 72.0 + 36.0)).abs() < 0.5,
        "{caret:?}"
    );
    // The second line starts at the left indent only.
    let second = b.layout.get(1).expect("second line");
    assert!(second.metrics().offset.abs() < 0.5);
    // Clicking at the indented start finds offset 0.
    let hit = s
        .hit_test(caret.page, caret.x + 1.0, caret.y + caret.height * 0.5)
        .expect("hit");
    assert_eq!((hit.para, hit.byte_offset), (1, 0));
    // No line is wider than the space between the indents.
    let room = geo.content_width() - 72.0 - 36.0;
    for li in 0..b.lines.len() {
        let line = b.layout.get(li).expect("line");
        let m = line.metrics();
        assert!(
            m.offset + m.advance - m.hanging_advance <= room + 0.5,
            "line {li} overflows: {m:?} room {room}"
        );
    }
}

#[test]
fn line_spacing_rules_set_the_line_heights() {
    let text =
        "Lines of text that wrap more than once across the page width, again and again. ".repeat(4);
    let mut exact = para(1, &text);
    exact.line_spacing = Some(libertum_model::LineSpacing::Exactly(30.0));
    let mut double = para(2, &text);
    double.line_spacing = Some(libertum_model::LineSpacing::Multiple(2.0));
    let mut single = para(3, &text);
    single.line_spacing = Some(libertum_model::LineSpacing::Multiple(1.0));
    let s = LayoutState::new(
        PageGeometry::a4(),
        vec![exact, double, single],
        LayoutOptions::default(),
    );
    let e = s.paragraph_layout(0).expect("exact");
    assert!(e.lines.iter().all(|l| (l.height - 30.0).abs() < 0.01));
    let d = s.paragraph_layout(1).expect("double");
    let one = s.paragraph_layout(2).expect("single");
    let ratio = d.lines[0].height / one.lines[0].height;
    assert!((ratio - 2.0).abs() < 0.01, "ratio {ratio}");
}

#[test]
fn superscript_is_smaller_and_raised() {
    if !fcx_has_fonts() {
        return;
    }
    let mut p = para(1, "E = mc2");
    let base = SpanStyle::new(11.0);
    p.spans.push((
        6..7,
        SpanStyle {
            vertical: libertum_model::VerticalAlign::Superscript,
            ..base.clone()
        },
    ));
    let s = LayoutState::new(PageGeometry::a4(), vec![p], LayoutOptions::default());
    let pl = s.paragraph_layout(0).expect("layout");
    let line = pl.layout.get(0).expect("line");
    let runs: Vec<_> = line
        .items()
        .filter_map(|item| match item {
            parley::PositionedLayoutItem::GlyphRun(run) => Some(run),
            _ => None,
        })
        .collect();
    let sup = runs
        .iter()
        .find(|r| r.style().brush.rise_64 > 0)
        .expect("a raised run");
    let normal = runs
        .iter()
        .find(|r| r.style().brush.rise_64 == 0)
        .expect("a normal run");
    assert!(sup.run().font_size() < normal.run().font_size());
    assert!((sup.style().brush.rise_pt() - 11.0 * 0.33).abs() < 0.05);
}

#[test]
fn installed_font_families_are_listed_sorted() {
    let s = LayoutState::new(
        PageGeometry::a4(),
        vec![para(1, "x")],
        LayoutOptions::default(),
    );
    let names = s.font_families();
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    assert!(lower.windows(2).all(|w| w[0] <= w[1]));
    assert!(
        names
            .iter()
            .all(|n| !n.starts_with('@') && !n.starts_with('.'))
    );
    if fcx_has_fonts() {
        assert!(!names.is_empty());
    }
}

#[test]
fn ligatures_none_never_merges_glyphs() {
    if !fcx_has_fonts() {
        return;
    }
    let glyphs = |l: libertum_model::Ligatures| {
        let mut p = para(1, "office fifty affluent");
        p.base = Some(SpanStyle {
            ligatures: l,
            ..SpanStyle::new(11.0)
        });
        let s = LayoutState::new(PageGeometry::a4(), vec![p], LayoutOptions::default());
        let pl = s.paragraph_layout(0).expect("layout");
        let line = pl.layout.get(0).expect("line");
        line.items()
            .map(|item| match item {
                parley::PositionedLayoutItem::GlyphRun(run) => run.positioned_glyphs().count(),
                _ => 0,
            })
            .sum::<usize>()
    };
    let none = glyphs(libertum_model::Ligatures::None);
    let standard = glyphs(libertum_model::Ligatures::Standard);
    // One glyph per character without ligatures; fewer where the font has fi/ff ligatures.
    assert_eq!(none, "office fifty affluent".chars().count());
    assert!(standard <= none);
}

#[test]
fn hyphenation_breaks_only_at_syllables_and_marks_the_line() {
    if !fcx_has_fonts() {
        return;
    }
    // Long words, accented letters and a multi-code-point emoji before them, so a wrong
    // mapping from override calls to positions would show up as a break mid-syllable.
    let text = "Café 👩‍👩‍👧 extraordinarily incomprehensible responsibilities characterisation \
                internationalisation demonstrations consideration hyphenation algorithmically \
                uncharacteristically representations"
        .to_owned();
    let mut p = para(1, &text);
    p.hyphenate = true;
    p.indent_right_pt = 300.0; // a narrow column forces many breaks
    let mut plain = p.clone();
    plain.id = 2;
    plain.hyphenate = false;
    let s = LayoutState::new(PageGeometry::a4(), vec![p, plain], LayoutOptions::default());
    let pl = s.paragraph_layout(0).expect("layout");
    assert!(!pl.hyphen_lines.is_empty(), "some word was hyphenated");
    // Every break that is not after whitespace must be at a syllable boundary.
    let syllable_ends: std::collections::HashSet<usize> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| w.chars().count() >= 5)
        .flat_map(|w| {
            let base = w.as_ptr() as usize - text.as_ptr() as usize;
            let mut at = base;
            let syl: Vec<&str> = hypher::hyphenate(w, hypher::Lang::English).collect();
            syl.iter()
                .take(syl.len() - 1)
                .map(|s| {
                    at += s.len();
                    at
                })
                .collect::<Vec<_>>()
        })
        .collect();
    for li in 0..pl.layout.len() - 1 {
        let end = pl.layout.get(li).expect("line").text_range().end;
        let before = text[..end].chars().last().expect("char");
        let hyphenated = pl.hyphen_lines.contains(&li);
        if hyphenated {
            assert!(
                syllable_ends.contains(&end),
                "line {li} breaks mid-syllable at {end}"
            );
        } else {
            assert!(
                !before.is_alphabetic() || !text[end..].starts_with(char::is_alphabetic),
                "line {li} breaks inside a word without a hyphen"
            );
        }
    }
    // Hyphenation fits more text per line than not hyphenating.
    let plain = s.paragraph_layout(1).expect("plain");
    assert!(plain.hyphen_lines.is_empty());
    // A hyphen decoration is drawn on the page.
    let mut st = s;
    st.paginate_all();
    let hyphens = st
        .page_decorations(0)
        .into_iter()
        .filter(|d| matches!(d, Decoration::Text { text, .. } if text == "-"))
        .count();
    assert_eq!(hyphens, pl.hyphen_lines.len());
}

// ---------------------------------------------------------------- columns

fn columns_doc(paragraphs: usize, columns: u8, gutter: f32) -> libertum_model::Document {
    use libertum_model::{BlockId, Document, Paragraph, Section};
    let body = (0..paragraphs)
        .map(|i| {
            Paragraph::new(
                BlockId(i as u64 + 1),
                format!(
                    "Paragraph {i}: the quick brown fox jumps over the lazy dog, and keeps \
                     running past the margin until it wraps onto another line."
                ),
            )
        })
        .collect();
    let mut doc = Document::from_paragraphs(body);
    doc.sections = vec![Section {
        columns,
        column_gutter_pt: gutter,
        ..Section::default()
    }];
    doc
}

#[test]
fn column_layout_splits_the_width_and_drops_too_narrow_columns() {
    let (n, width, gutter) = column_layout(451.276, 2, 36.0);
    assert_eq!(n, 2);
    assert!((width - (451.276 - 36.0) / 2.0).abs() < 0.01);
    assert!((gutter - 36.0).abs() < f32::EPSILON);

    // Twelve columns cannot each be 0.5 in wide, so the count drops.
    let (n, width, _) = column_layout(451.276, 12, 36.0);
    assert!(n < 12, "too many columns for the width");
    assert!(width >= 36.0 - 0.01);

    let (n, width, _) = column_layout(451.276, 1, 36.0);
    assert_eq!(n, 1);
    assert!((width - 451.276).abs() < 0.01);

    // A negative gutter is treated as zero rather than producing overlapping columns.
    let (n, width, gutter) = column_layout(400.0, 2, -10.0);
    assert_eq!(n, 2);
    assert!((gutter - 0.0).abs() < f32::EPSILON);
    assert!((width - 200.0).abs() < 0.01);
}

#[test]
fn a_two_column_page_fills_the_left_column_then_the_right() {
    let doc = columns_doc(40, 2, 24.0);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let page = &s.pages()[0];
    assert!(page.slices.iter().any(|sl| sl.col == 0));
    assert!(
        page.slices.iter().any(|sl| sl.col == 1),
        "the first page uses both columns"
    );
    // Reading order: every left-column slice comes before every right-column slice.
    let first_right = page
        .slices
        .iter()
        .position(|sl| sl.col == 1)
        .expect("right column");
    assert!(page.slices[..first_right].iter().all(|sl| sl.col == 0));
    assert!(page.slices[first_right..].iter().all(|sl| sl.col == 1));
    // The right column starts at the top, one column plus the gutter from the left margin.
    let right = &page.slices[first_right];
    assert!(
        right.y < 1.0,
        "the second column starts at the top of the page"
    );
    let side = s.column_width(0) + 24.0;
    assert!((right.x - side).abs() < 0.01, "{} vs {side}", right.x);
    assert!(
        page.slices
            .iter()
            .filter(|sl| sl.col == 0)
            .all(|sl| sl.x.abs() < 0.01)
    );
}

#[test]
fn a_column_is_narrower_than_the_page_so_text_wraps_more() {
    use libertum_model::{BlockId, Document, Paragraph, Section};
    let text = "The quick brown fox jumps over the lazy dog. ".repeat(20);
    let doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), text)]);
    let mut wide = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    let mut narrow_doc = doc.clone();
    narrow_doc.sections = vec![Section {
        columns: 2,
        ..Section::default()
    }];
    let mut narrow =
        LayoutState::from_document(&narrow_doc, PageGeometry::a4(), LayoutOptions::default());
    wide.paginate_all();
    narrow.paginate_all();
    let wide_lines = wide.paragraph_layout(0).expect("wide").lines.len();
    let narrow_lines = narrow.paragraph_layout(0).expect("narrow").lines.len();
    assert!(
        narrow_lines > wide_lines,
        "a column wraps into more lines ({narrow_lines} vs {wide_lines})"
    );
}

#[test]
fn caret_and_hit_test_round_trip_in_the_second_column() {
    let doc = columns_doc(40, 2, 24.0);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let page = &s.pages()[0];
    let right = *page
        .slices
        .iter()
        .find(|sl| sl.col == 1)
        .expect("right column");
    let para = right.para;
    let mut offset = s.paragraphs()[para].text.len() / 2;
    while !s.paragraphs()[para].text.is_char_boundary(offset) {
        offset += 1;
    }
    let caret = s.caret_rect(para, offset, 1.0).expect("caret");
    assert_eq!(caret.page, 0);
    assert!(
        caret.x >= s.geometry().margins.left + right.x - 0.01,
        "the caret is inside the second column"
    );
    let hit = s
        .hit_test(caret.page, caret.x + 0.5, caret.y + caret.height * 0.5)
        .expect("hit");
    assert_eq!(hit.para, para);
    assert!(
        hit.byte_offset.abs_diff(offset) <= 4,
        "{} vs {offset}",
        hit.byte_offset
    );
}

#[test]
fn changing_columns_repaginates_and_changes_the_page_hash() {
    use libertum_model::{BlockId, Document, Paragraph};
    // Short paragraphs fit one line at either width, so two columns simply double the
    // capacity of a page.
    let body = (0..60)
        .map(|i| Paragraph::new(BlockId(i + 1), format!("Paragraph {i}.")))
        .collect();
    let doc = Document::from_paragraphs(body);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    let single_pages = s.page_count();
    let single_hash = s.page_content_hash(0).expect("hash");
    let section = libertum_model::Section {
        columns: 2,
        ..libertum_model::Section::default()
    };
    assert!(
        s.set_sections(vec![section]),
        "a column change re-paginates"
    );
    assert_eq!(
        s.page_count(),
        0,
        "pages are dropped for a full re-pagination"
    );
    s.paginate_all();
    assert_ne!(s.page_content_hash(0), Some(single_hash));
    assert!(
        s.page_count() < single_pages,
        "two columns fit more text per page ({} vs {single_pages})",
        s.page_count()
    );
}

#[test]
fn each_section_uses_its_own_columns() {
    use libertum_model::{BlockId, Document, ParaProps, Paragraph, Section};
    let body = (0..20u64)
        .map(|i| {
            let props = ParaProps {
                section_start: i == 6,
                ..ParaProps::default()
            };
            Paragraph::new(
                BlockId(i + 1),
                format!("Paragraph {i}: the quick brown fox jumps over the lazy dog."),
            )
            .with_props(props)
        })
        .collect();
    let mut doc = Document::from_paragraphs(body);
    doc.sections = vec![
        Section::default(),
        Section {
            columns: 2,
            column_gutter_pt: 24.0,
            ..Section::default()
        },
    ];
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    assert_eq!(s.pages()[0].section, 0);
    let second = s
        .pages()
        .iter()
        .find(|p| p.section == 1)
        .expect("second section");
    assert!(
        second.slices.iter().all(|sl| sl.col == 0),
        "a short section may not reach its second column"
    );
    assert_eq!(s.column_count(0), 1);
    assert_eq!(s.column_count(1), 2);
    assert!(s.column_width(1) < s.column_width(0));
}

// ---------------------------------------------------------------- tables

fn table_state(rows: usize, columns: usize, cell_text: &str, width: f32) -> LayoutState {
    use libertum_model::{Operation, Paragraph};
    let mut doc = libertum_model::Document::from_paragraphs(vec![
        Paragraph::new(libertum_model::BlockId(1), "Before"),
        Paragraph::new(libertum_model::BlockId(2), "After"),
    ]);
    let mut table = doc.new_table(rows, columns, width);
    for row in &mut table.rows {
        for cell in &mut row.cells {
            for p in &mut cell.paragraphs {
                p.insert_text(0, cell_text);
            }
        }
    }
    table.header_rows = 1;
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    let paragraphs: Vec<ParagraphSnapshot> = doc
        .paragraphs()
        .map(|p| ParagraphSnapshot::from_model(p, &doc.styles))
        .collect();
    let mut state = LayoutState::new(PageGeometry::a4(), paragraphs, LayoutOptions::default());
    state.set_tables(table_snapshots(&doc));
    state
}

#[test]
fn table_cells_are_laid_out_side_by_side() {
    let mut state = table_state(1, 2, "cell text", 400.0);
    state.paginate_all();
    let page = &state.pages()[0];
    let draw = page.tables.first().expect("a table row is drawn");
    assert_eq!(draw.cells.len(), 2);
    assert!(draw.cells[1].x > draw.cells[0].x + draw.cells[0].width - 0.01);
    assert!((draw.width - 400.0).abs() < 0.01, "width = {}", draw.width);
    // The two cell paragraphs share a top and sit at different x.
    let slices: Vec<ParaSlice> = page
        .slices
        .iter()
        .filter(|s| s.para >= 1 && s.para <= 2)
        .copied()
        .collect();
    assert_eq!(slices.len(), 2);
    assert!((slices[0].y - slices[1].y).abs() < 0.01);
    assert!(slices[1].x > slices[0].x + 10.0);
}

#[test]
fn a_tall_table_moves_rows_to_the_next_page_and_repeats_headers() {
    let text = "A long cell paragraph that wraps over several lines in a narrow cell. ".repeat(3);
    let mut state = table_state(40, 1, &text, 400.0);
    state.paginate_all();
    assert!(state.page_count() > 1, "the table must span pages");
    // The first page starts with the header row as a normal row; later pages repeat it.
    assert!(state.pages()[0].tables.first().is_some_and(|t| !t.header));
    let repeated = state
        .pages()
        .iter()
        .skip(1)
        .any(|p| p.tables.iter().any(|t| t.header));
    assert!(repeated, "the header row repeats on a continuation page");
    // Rows stay inside the page.
    let height = state.geometry().content_height();
    for page in state.pages() {
        for table in &page.tables {
            assert!(table.y + table.height <= height + 1.0);
        }
    }
}

#[test]
fn hit_test_inside_a_cell_finds_that_cell() {
    let mut state = table_state(1, 2, "left right", 400.0);
    state.paginate_all();
    let page = &state.pages()[0];
    let draw = page.tables.first().expect("table");
    let second = draw.cells[1].clone();
    let hit = state
        .hit_test(
            0,
            state.geometry().margins.left + second.x + 6.0,
            state.geometry().margins.top + draw.y + 6.0,
        )
        .expect("hit");
    let expected = state.tables()[0].rows[0].cells[1].paragraphs[0];
    assert_eq!(hit.para, expected);
}

#[test]
fn from_document_builds_table_snapshots() {
    let doc = {
        let mut doc =
            libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
                libertum_model::BlockId(1),
                "Body",
            )]);
        let table = doc.new_table(2, 2, 300.0);
        doc.apply(libertum_model::Operation::InsertTable { index: 1, table })
            .expect("insert");
        doc
    };
    let state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    assert_eq!(state.tables().len(), 1);
    assert_eq!(state.tables()[0].rows.len(), 2);
    assert_eq!(state.tables()[0].rows[0].cells.len(), 2);
}

#[test]
fn linked_headers_and_footers_follow_the_previous_section() {
    let mut doc = libertum_model::Document::from_paragraphs(vec![
        libertum_model::Paragraph::new(libertum_model::BlockId(1), "First section"),
        libertum_model::Paragraph::new(libertum_model::BlockId(2), "Second section").with_props(
            libertum_model::ParaProps {
                section_start: true,
                ..libertum_model::ParaProps::default()
            },
        ),
    ]);
    let first = libertum_model::Section {
        headers: libertum_model::HeadersFooters::with_default("Inherited header"),
        footers: libertum_model::HeadersFooters::with_default("Inherited footer"),
        ..libertum_model::Section::default()
    };
    let second = libertum_model::Section {
        link_headers_to_previous: true,
        link_footers_to_previous: true,
        headers: libertum_model::HeadersFooters::with_default("Ignored"),
        ..libertum_model::Section::default()
    };
    doc.sections = vec![first, second];
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    assert!(state.page_count() >= 2);
    let header = state
        .furniture_on(1, libertum_model::HfKind::Header)
        .expect("linked header");
    assert_eq!(header.1.text, "Inherited header");
    let footer = state
        .furniture_on(1, libertum_model::HfKind::Footer)
        .expect("linked footer");
    assert_eq!(footer.1.text, "Inherited footer");
}

#[test]
fn sections_can_have_different_page_geometries() {
    let mut doc = libertum_model::Document::from_paragraphs(vec![
        libertum_model::Paragraph::new(libertum_model::BlockId(1), "Portrait section"),
        libertum_model::Paragraph::new(libertum_model::BlockId(2), "Landscape section").with_props(
            libertum_model::ParaProps {
                section_start: true,
                ..libertum_model::ParaProps::default()
            },
        ),
    ]);
    let portrait = libertum_model::Section::default();
    let landscape = libertum_model::Section {
        page: libertum_model::PageSetup {
            paper: libertum_model::PaperSize::A4,
            orientation: libertum_model::Orientation::Landscape,
            margins: libertum_model::Margins::uniform(72.0),
        },
        ..libertum_model::Section::default()
    };
    doc.sections = vec![portrait, landscape];
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    assert!(state.page_count() >= 2, "each section starts a page");
    assert!((state.page_geometry(0).width_pt - 595.276).abs() < 0.1);
    assert!((state.page_geometry(1).width_pt - 841.89).abs() < 0.1);
    assert!((state.page_geometry(1).height_pt - 595.276).abs() < 0.1);
}

#[test]
fn footnotes_are_collected_at_the_page_bottom_and_numbered() {
    let mut paragraphs = Vec::new();
    for i in 0..30 {
        let mut paragraph =
            libertum_model::Paragraph::new(libertum_model::BlockId(i + 1), format!("Line {i}"));
        if i == 0 {
            paragraph.set_notes(vec![
                libertum_model::Note {
                    kind: libertum_model::NoteKind::Footnote,
                    offset: 0,
                    text: "A note at the foot".into(),
                },
                libertum_model::Note {
                    kind: libertum_model::NoteKind::Footnote,
                    offset: 2,
                    text: "Another note".into(),
                },
            ]);
        }
        paragraphs.push(paragraph);
    }
    let doc = libertum_model::Document::from_paragraphs(paragraphs);
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    assert_eq!(state.pages()[0].notes.len(), 2);
    assert_eq!(state.pages()[0].notes[0].number, 1);
    assert_eq!(state.pages()[0].notes[1].number, 2);
    assert!(state.pages()[0].notes[0].text.contains("A note"));
    let decorations = state.page_decorations(0);
    assert!(
        decorations
            .iter()
            .any(|d| matches!(d, Decoration::Text { text, .. } if text.contains("A note")))
    );
    assert!(
        decorations
            .iter()
            .any(|d| matches!(d, Decoration::Rect { .. }))
    );
}

#[test]
fn endnotes_get_their_own_page_at_the_document_end() {
    let mut paragraph = libertum_model::Paragraph::new(libertum_model::BlockId(1), "Body");
    paragraph.set_notes(vec![libertum_model::Note {
        kind: libertum_model::NoteKind::Endnote,
        offset: 0,
        text: "End note".into(),
    }]);
    let doc = libertum_model::Document::from_paragraphs(vec![paragraph]);
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    let last = state.pages().last().expect("a page");
    assert!(last.endnotes);
    assert_eq!(last.notes.len(), 1);
    assert!(
        state
            .page_decorations(state.page_count() - 1)
            .iter()
            .any(|d| matches!(d, Decoration::Text { text, .. } if text == "Endnotes"))
    );
}

fn image_doc(
    text: &str,
    offset: usize,
    px: (u32, u32),
    size_pt: (f32, f32),
) -> libertum_model::Document {
    use libertum_model::{BlockId, Document, InlineImage, Media, Paragraph, test_png_header};
    let media = std::sync::Arc::new(Media::new(test_png_header(px.0, px.1)).expect("png header"));
    let mut p = Paragraph::new(BlockId(1), text);
    p.set_images(vec![InlineImage {
        offset,
        media,
        width_pt: size_pt.0,
        height_pt: size_pt.1,
        alt: String::new(),
        name: String::new(),
    }]);
    Document::from_paragraphs(vec![p, Paragraph::new(BlockId(2), "After the picture")])
}

fn images_on(s: &LayoutState, page: usize) -> Vec<(f32, f32, f32, f32)> {
    s.page_decorations(page)
        .into_iter()
        .filter_map(|d| match d {
            Decoration::Image {
                x,
                y,
                width,
                height,
                ..
            } => Some((x, y, width, height)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_inline_image_takes_room_in_its_line_and_is_drawn_there() {
    if !fcx_has_fonts() {
        return;
    }
    let geo = PageGeometry::a4();
    let plain = image_doc("Text before and text after", 12, (10, 10), (100.0, 80.0));
    let mut without = LayoutState::new(
        geo,
        vec![ParagraphSnapshot::new(1, "Text before and text after")],
        LayoutOptions::default(),
    );
    without.paginate_all();
    let mut s = LayoutState::from_document(&plain, geo, LayoutOptions::default());
    s.paginate_all();
    let images = images_on(&s, 0);
    assert_eq!(images.len(), 1, "one image decoration");
    let (x, y, w, h) = images[0];
    assert!(
        (w - 100.0).abs() < 0.01 && (h - 80.0).abs() < 0.01,
        "{w} x {h}"
    );
    assert!(
        x > geo.margins.left + 10.0,
        "after the text before it: x = {x}"
    );
    assert!(y >= geo.margins.top - 0.01, "inside the body: y = {y}");
    // The line grew to hold the image.
    let with_height = s.paragraph_layout(0).expect("layout").height;
    let without_height = without.paragraph_layout(0).expect("layout").height;
    assert!(
        with_height > without_height + 50.0,
        "{with_height} vs {without_height}"
    );
    // The image is one of the page's items, so its strip repaints when it changes.
    let before: Vec<u64> = s.page_items(0).iter().map(|i| i.key).collect();
    let moved = image_doc("Text before and text after", 12, (10, 10), (120.0, 80.0));
    let mut t = LayoutState::from_document(&moved, geo, LayoutOptions::default());
    t.paginate_all();
    let after: Vec<u64> = t.page_items(0).iter().map(|i| i.key).collect();
    assert_ne!(before, after);
}

#[test]
fn an_oversized_image_is_scaled_to_the_column_and_the_page() {
    if !fcx_has_fonts() {
        return;
    }
    let geo = PageGeometry::a4();
    // Wider than the column: scaled down in proportion.
    let wide = image_doc("", 0, (10, 10), (2_000.0, 1_000.0));
    let mut s = LayoutState::from_document(&wide, geo, LayoutOptions::default());
    s.paginate_all();
    let (_, _, w, h) = images_on(&s, 0)[0];
    assert!(w <= geo.content_width() + 0.01, "width {w}");
    assert!((h / w - 0.5).abs() < 0.001, "aspect kept: {w} x {h}");
    // Taller than the page body: scaled so its line fits on one page.
    let tall = image_doc("", 0, (10, 10), (100.0, 3_000.0));
    let mut s = LayoutState::from_document(&tall, geo, LayoutOptions::default());
    s.paginate_all();
    let (_, y, w, h) = images_on(&s, 0)[0];
    assert!(h < geo.content_height(), "height {h}");
    assert!(y + h <= geo.margins.top + geo.content_height() + 0.01);
    assert!((w / h - 1.0 / 30.0).abs() < 0.001, "aspect kept: {w} x {h}");
}

#[test]
fn an_image_in_an_empty_paragraph_still_gets_a_line() {
    if !fcx_has_fonts() {
        return;
    }
    let doc = image_doc("", 0, (10, 10), (60.0, 40.0));
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    assert_eq!(images_on(&s, 0).len(), 1);
    let layout = s.paragraph_layout(0).expect("layout");
    assert!(layout.height >= 40.0, "height {}", layout.height);
    assert!(
        s.caret_rect(0, 0, 1.0).is_some(),
        "the caret still has a place"
    );
}

#[test]
fn a_table_style_fills_rows_in_the_layout_snapshot() {
    use libertum_model::{Operation, TableStyle};
    let mut doc = libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
        libertum_model::BlockId(1),
        "Before",
    )]);
    let mut table = doc.new_table(4, 2, 300.0);
    let (style, borders) = TableStyle::builtin("Banded Rows").expect("built-in");
    table.apply_style(Some(style.clone()), borders);
    table.rows[1].cells[0].fill = Some([1, 2, 3, 255]);
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    let snapshot = &table_snapshots(&doc)[0];
    assert_eq!(snapshot.rows[0].cells[0].fill, style.header_fill);
    assert_eq!(
        snapshot.rows[1].cells[0].fill,
        Some([1, 2, 3, 255]),
        "own fill wins"
    );
    assert_eq!(snapshot.rows[1].cells[1].fill, None);
    assert_eq!(snapshot.rows[2].cells[1].fill, style.band_fill);
    assert_eq!(snapshot.rows[3].cells[0].fill, None);
}

/// A document with `filler` body paragraphs, then a one-row, two-column table whose first
/// cell holds `words` words, then a closing paragraph.
fn split_row_state(filler: usize, words: usize, keep_together: bool) -> LayoutState {
    use libertum_model::{BlockId, Document, Operation, Paragraph};
    let mut paragraphs: Vec<Paragraph> = (0..filler)
        .map(|i| Paragraph::new(BlockId(i as u64 + 1), "Filler line of body text."))
        .collect();
    paragraphs.push(Paragraph::new(BlockId(9_000), "After the table"));
    let mut doc = Document::from_paragraphs(paragraphs);
    let mut table = doc.new_table(1, 2, 400.0);
    let text = "word ".repeat(words);
    table.rows[0].cells[0].paragraphs[0].insert_text(0, &text);
    table.rows[0].cells[1].paragraphs[0].insert_text(0, "short");
    table.rows[0].keep_together = keep_together;
    doc.apply(Operation::InsertTable {
        index: filler,
        table,
    })
    .expect("insert table");
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    state
}

/// Every line of paragraph `para` across all pages, as (page, line) pairs.
fn lines_of(state: &LayoutState, para: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for (page, pg) in state.pages().iter().enumerate() {
        for s in pg.slices.iter().filter(|s| s.para == para) {
            for line in s.line_start..s.line_end {
                out.push((page, line));
            }
        }
    }
    out
}

#[test]
fn a_row_that_may_split_breaks_across_pages_at_a_line() {
    if !fcx_has_fonts() {
        return;
    }
    // 30 filler lines leave a few lines of room; the cell needs far more than that.
    let filler = 30;
    let state = split_row_state(filler, 400, false);
    let cell = filler; // the first cell's paragraph follows the filler paragraphs
    let total = state.paragraph_layout(cell).expect("layout").lines.len();
    let lines = lines_of(&state, cell);
    assert_eq!(lines.len(), total, "every line is placed exactly once");
    let mut seen: Vec<usize> = lines.iter().map(|(_, line)| *line).collect();
    seen.dedup();
    assert_eq!(seen, (0..total).collect::<Vec<_>>(), "in order, no repeats");
    let pages: Vec<usize> = lines.iter().map(|(page, _)| *page).collect();
    assert_eq!(pages.first(), Some(&0), "the row starts on the first page");
    assert!(pages.last() > Some(&0), "and continues on the next");
    // Both pieces are drawn and neither runs off its page.
    for page in 0..2 {
        let pg = &state.pages()[page];
        let draw = pg.tables.first().expect("a piece of the row");
        assert!(
            draw.y + draw.height <= pg.geometry.content_height() + 0.5,
            "page {page}: piece ends at {}",
            draw.y + draw.height
        );
    }
    // The piece on the first page is closed with a line, and the next page opens with one.
    assert!(state.pages()[0].tables[0].bottom);
    assert!(state.pages()[1].tables[0].cells[0].top);
    // The caret follows the text onto the second page, and a click there finds it again.
    let late = state.paragraphs()[cell].text.len() - 3;
    let caret = state.caret_rect(cell, late, 1.0).expect("caret");
    assert!(caret.page >= 1, "caret page {}", caret.page);
    let hit = state
        .hit_test(caret.page, caret.x + 0.5, caret.y + caret.height / 2.0)
        .expect("hit");
    assert_eq!(hit.para, cell);
    assert!(
        hit.byte_offset.abs_diff(late) <= 1,
        "{} vs {late}",
        hit.byte_offset
    );
    // The paragraph after the table still follows it.
    let after = state.paragraphs().len() - 1;
    assert!(!lines_of(&state, after).is_empty());
}

#[test]
fn a_row_kept_together_moves_whole_unless_it_is_taller_than_a_page() {
    if !fcx_has_fonts() {
        return;
    }
    // Short enough for one page: the whole row goes to the next page.
    let filler = 30;
    let state = split_row_state(filler, 150, true);
    let pages: Vec<usize> = lines_of(&state, filler).iter().map(|(p, _)| *p).collect();
    assert!(pages.iter().all(|p| *p == 1), "all on page 2: {pages:?}");
    assert!(state.pages()[0].tables.is_empty());
    // Taller than a page: it has to break, kept together or not.
    let state = split_row_state(0, 3_000, true);
    let total = state.paragraph_layout(0).expect("layout").lines.len();
    let lines = lines_of(&state, 0);
    assert_eq!(lines.len(), total);
    let last_page = lines.last().map(|(p, _)| *p).expect("lines");
    assert!(last_page >= 1, "spread over {} pages", last_page + 1);
    for pg in state.pages() {
        for draw in &pg.tables {
            assert!(draw.y + draw.height <= pg.geometry.content_height() + 0.5);
        }
    }
}

#[test]
fn an_edit_in_a_split_row_repaginates_like_a_fresh_layout() {
    if !fcx_has_fonts() {
        return;
    }
    let filler = 30;
    let mut state = split_row_state(filler, 400, false);
    let mut edited = state.paragraphs()[filler].clone();
    edited
        .text
        .push_str(" and a few more words at the very end of the cell");
    edited.version += 1;
    state.replace_paragraph(filler, edited.clone());
    let mut fresh = LayoutState::new(
        PageGeometry::a4(),
        state.paragraphs().to_vec(),
        LayoutOptions::default(),
    );
    fresh.set_tables(state.tables().to_vec());
    fresh.paginate_all();
    assert_eq!(state.page_count(), fresh.page_count());
    for (a, b) in state.pages().iter().zip(fresh.pages()) {
        assert_eq!(a.start, b.start);
        assert_eq!(a.slices, b.slices);
        assert_eq!(a.tables, b.tables);
    }
}

#[test]
fn a_vertically_merged_cell_has_no_text_and_no_line_above_it() {
    if !fcx_has_fonts() {
        return;
    }
    use libertum_model::{BlockId, Document, Operation, Paragraph};
    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Before")]);
    let mut table = doc.new_table(2, 2, 300.0);
    let red = [200, 10, 10, 255];
    table.rows[0].cells[0].paragraphs[0].insert_text(0, "Tall cell");
    table.rows[0].cells[0].fill = Some(red);
    table.rows[1].cells[0].merged_into_above = true;
    let hidden = table.rows[1].cells[0].paragraphs[0].id;
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    let hidden = doc.index_of(hidden).expect("index");
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    assert!(
        lines_of(&state, hidden).is_empty(),
        "a covered cell lays out no text"
    );
    let page = &state.pages()[0];
    let (first, second) = (&page.tables[0], &page.tables[1]);
    assert!(first.cells[0].top && first.cells[1].top);
    assert!(!second.cells[0].top, "no line between the merged cells");
    assert!(second.cells[1].top, "the cell beside it keeps its line");
    assert_eq!(
        second.cells[0].fill,
        Some(red),
        "painted like the cell it continues"
    );
    assert!(second.bottom && !first.bottom);
}

#[test]
fn a_table_without_inner_lines_draws_only_its_outline() {
    use libertum_model::{BlockId, Document, Operation, Paragraph, TableBorders};
    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Before")]);
    let mut table = doc.new_table(3, 2, 300.0);
    table.borders = TableBorders {
        inner: false,
        ..TableBorders::default()
    };
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    let mut state = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    state.paginate_all();
    let draws = &state.pages()[0].tables;
    assert_eq!(draws.len(), 3);
    assert!(draws[0].cells.iter().all(|c| c.top), "top of the table");
    assert!(draws[1].cells.iter().all(|c| !c.top), "no row lines inside");
    assert!(draws.iter().all(|d| d.cells.iter().all(|c| !c.left)));
    assert!(draws.iter().all(|d| d.sides));
    assert!(!draws[0].bottom && !draws[1].bottom && draws[2].bottom);
}

#[test]
fn a_pageless_view_draws_no_page_numbers_or_headers() {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![para(1, "Manuscript")],
        LayoutOptions::default(),
    );
    let mut section = Section {
        page_numbers: Some(PageNumberPos::TOP_RIGHT),
        // A header that reaches further down than the top margin, so it takes room from the
        // body in paged view and none in a pageless one.
        header_distance_pt: 90.0,
        ..Section::default()
    };
    with_default(HfKind::Header, "Running head", &mut section);
    s.set_sections(vec![section]);
    s.paginate_all();
    let decorations = s.page_decorations(0);
    assert!(
        decorations
            .iter()
            .any(|d| matches!(d, Decoration::Text { text, .. } if text == "Running head")),
        "the header is drawn in paged view"
    );
    let paged_top = s.pages()[0].geometry.margins.top;

    assert!(s.set_pageless(true), "turning pageless on re-paginates");
    s.paginate_all();
    assert!(s.is_pageless());
    let decorations = s.page_decorations(0);
    assert!(
        !decorations
            .iter()
            .any(|d| matches!(d, Decoration::Text { .. })),
        "no page number and no header in a pageless view"
    );
    assert!(
        s.pages()[0].geometry.margins.top < paged_top,
        "the header reserves no room in a pageless view"
    );

    // Back to pages: the header and its room return.
    assert!(s.set_pageless(false));
    s.paginate_all();
    assert!((s.pages()[0].geometry.margins.top - paged_top).abs() < 0.01);
    assert!(
        s.page_decorations(0)
            .iter()
            .any(|d| matches!(d, Decoration::Text { text, .. } if text == "Running head"))
    );
}

#[test]
fn the_caret_is_as_tall_as_the_text_not_the_line() {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![para(1, "Caret height")],
        LayoutOptions::default(),
    );
    s.paginate_all();
    let caret = s.caret_rect(0, 0, 1.0).expect("a caret");
    let layout = s.paragraph_layout(0).expect("layout");
    let line = &layout.lines[0];
    // 12 pt Cambria at the default line spacing: the line box is about 16-17 pt, the glyph
    // box about 13-14 pt. The caret follows the text, or it looks oversized.
    assert!(
        (9.0..15.0).contains(&caret.height),
        "caret height {} is not the text's",
        caret.height
    );
    assert!(
        caret.height < line.height,
        "the caret ({} pt) should be shorter than the line box ({} pt)",
        caret.height,
        line.height
    );
}

#[test]
fn a_watermark_is_sized_to_the_page_and_centred_on_it() {
    use libertum_model::{BlockId, Document, Paragraph, Watermark};
    if !fcx_has_fonts() {
        return;
    }
    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Body")]);
    let geo = PageGeometry::a4();
    let mut plain = LayoutState::from_document(&doc, geo, LayoutOptions::default());
    plain.paginate_all();
    assert!(plain.page_watermark(0).is_none(), "no watermark by default");
    let plain_hash = plain.page_content_hash(0);

    doc.sections[0].watermark = Watermark::new("DRAFT", true);
    let mut s = LayoutState::from_document(&doc, geo, LayoutOptions::default());
    s.paginate_all();
    let w = s.page_watermark(0).expect("a watermark");
    assert_eq!(w.text, "DRAFT");
    assert!((w.centre_x - geo.width_pt / 2.0).abs() < 0.01);
    assert!((w.centre_y - geo.height_pt / 2.0).abs() < 0.01);
    assert!(
        w.angle_deg < -45.0 && w.angle_deg > -60.0,
        "rises along the diagonal: {}",
        w.angle_deg
    );
    let diagonal = (geo.width_pt.powi(2) + geo.height_pt.powi(2)).sqrt();
    let width = w.layout.layout.width();
    assert!(
        width > diagonal * 0.4 && width < diagonal * 0.8,
        "spans most of it: {width}"
    );
    assert_ne!(s.page_content_hash(0), plain_hash, "the page repaints");
    assert!(
        s.page_items(0)
            .iter()
            .any(|i| i.top <= 0.0 && i.bottom >= geo.height_pt - 0.01)
    );

    doc.sections[0].watermark = Watermark::new("CONFIDENTIAL COPY", false);
    let mut h = LayoutState::from_document(&doc, geo, LayoutOptions::default());
    h.paginate_all();
    let w = h.page_watermark(0).expect("a watermark");
    assert_eq!(w.angle_deg, 0.0);
    assert!(w.layout.layout.width() <= geo.width_pt * 0.85);
    assert!(
        Watermark::new("   ", true).is_none(),
        "blank text is no watermark"
    );
    assert_eq!(
        Watermark::new("a\nb", true).map(|w| w.text),
        Some("a b".to_owned())
    );
}
