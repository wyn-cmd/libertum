use libertum_layout::spike::generate_document;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
use libertum_render::{PageRenderer, render_page};

fn state() -> LayoutState {
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        generate_document(3, 5),
        LayoutOptions::default(),
    );
    s.paginate_all();
    s
}

fn has_fonts(s: &LayoutState) -> bool {
    s.font_report().family_count > 0
}

/// Whether the first paragraph's text actually has glyphs: a machine with no CJK font shapes
/// these tests' Japanese text to nothing, so there is no ink to measure.
fn has_glyphs(s: &LayoutState) -> bool {
    let Some(layout) = s.paragraph_layout(0) else {
        return false;
    };
    layout.layout.lines().any(|line| {
        line.items().any(|item| {
            matches!(
                item,
                parley::PositionedLayoutItem::GlyphRun(run)
                    if run.positioned_glyphs().any(|glyph| glyph.id != 0)
            )
        })
    })
}

#[test]
fn renders_ink_inside_margins_only() {
    let s = state();
    if !has_fonts(&s) {
        return;
    }
    let pm = render_page(&s, 0, 1.0).expect("page 0");
    assert_eq!((pm.width(), pm.height()), (596, 842));
    let px = |x: u32, y: u32| {
        let i = ((y * pm.width() + x) * 4) as usize;
        [pm.data()[i], pm.data()[i + 1], pm.data()[i + 2]]
    };
    // Margin corner stays white.
    assert_eq!(px(5, 5), [255, 255, 255]);
    // Some dark ink exists in the content area, and the (blue) heading has a blue-ish pixel.
    let mut dark = 0;
    let mut blue = 0;
    for y in 72..400 {
        for x in 72..524 {
            let [r, g, b] = px(x, y);
            if r < 100 && g < 100 && b < 100 {
                dark += 1;
            }
            if u32::from(b) > u32::from(r) + 40 {
                blue += 1;
            }
        }
    }
    assert!(dark > 500, "dark = {dark}");
    assert!(blue > 50, "blue = {blue}");
}

#[test]
fn scale_changes_size_and_out_of_range_is_none() {
    let s = state();
    let pm = render_page(&s, 0, 2.0).expect("page 0");
    assert_eq!((pm.width(), pm.height()), (1191, 1684));
    assert!(render_page(&s, 9_999, 1.0).is_none());
}

#[test]
fn page_cache_hits_until_content_changes() {
    let mut s = state();
    let mut r = PageRenderer::default();
    let a = r.cached_page(&s, 1, 1.0).expect("page");
    let b = r.cached_page(&s, 1, 1.0).expect("page");
    assert!(
        std::sync::Arc::ptr_eq(&a, &b),
        "second call should hit the cache"
    );
    let para = s.pages()[1].slices[0].para;
    s.edit_paragraph(para, |p| p.insert_char(0, 'Z'));
    let c = r.cached_page(&s, 1, 1.0).expect("page");
    assert!(
        !std::sync::Arc::ptr_eq(&a, &c),
        "edited page must re-render"
    );
    // A different scale is a different cache entry.
    let d = r.cached_page(&s, 1, 2.0).expect("page");
    assert!(!std::sync::Arc::ptr_eq(&c, &d));
}

#[test]
fn header_and_footer_text_leave_ink() {
    use libertum_layout::ParagraphSnapshot;
    use libertum_model::{HeadersFooters, Section};

    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![ParagraphSnapshot::new(1, "Body text")],
        LayoutOptions::default(),
    );
    let section = Section {
        headers: HeadersFooters::with_default("Running head"),
        footers: HeadersFooters::with_default("Page {page}"),
        ..Section::default()
    };
    s.set_sections(vec![section]);
    s.paginate_all();
    if !has_fonts(&s) {
        return;
    }
    let pm = render_page(&s, 0, 1.0).expect("page 0");
    let ink = |x0: u32, y0: u32, x1: u32, y1: u32| {
        (y0..y1).any(|y| {
            (x0..x1).any(|x| {
                let i = ((y * pm.width() + x) * 4) as usize;
                pm.data()[i] < 128
            })
        })
    };
    // Above the top margin: the header, from the left margin rightwards.
    assert!(ink(72, 28, 400, 60), "header ink missing");
    // Below the bottom margin: the footer's page field.
    assert!(ink(72, 785, 400, 820), "footer ink missing");
    // The body's own text area is still where it was.
    assert!(ink(72, 72, 400, 100), "body ink missing");
}

#[test]
fn page_numbers_and_toc_leaders_leave_ink() {
    use libertum_layout::ParagraphSnapshot;
    use libertum_model::{PageNumberPos, Section};

    let mut entry = ParagraphSnapshot::new(2, "Chapter One");
    entry.toc_target = Some(3);
    let mut chapter = ParagraphSnapshot::new(3, "Chapter One");
    chapter.section_start = true;
    let mut s = LayoutState::new(
        PageGeometry::a4(),
        vec![ParagraphSnapshot::new(1, "Contents"), entry, chapter],
        LayoutOptions::default(),
    );
    s.set_sections(vec![Section::default(), Section::default()]);
    // Whole-document numbering starts on the chapter page (physical page 2).
    s.set_numbering(libertum_model::PageNumbering {
        show: true,
        position: PageNumberPos::BOTTOM_CENTRE,
        start_page: 2,
        start_at: 1,
        ..libertum_model::PageNumbering::default()
    });
    s.paginate_all();
    if !has_fonts(&s) {
        return;
    }
    let ink = |page: usize, x0: u32, y0: u32, x1: u32, y1: u32| {
        let pm = render_page(&s, page, 1.0).expect("page");
        (y0..y1).any(|y| {
            (x0..x1).any(|x| {
                let i = ((y * pm.width() + x) * 4) as usize;
                pm.data()[i] < 128
            })
        })
    };
    // Footer of the chapter page (bottom margin, middle third): the page number.
    assert!(ink(1, 200, 770, 400, 842), "page number missing");
    // The contents page has no number of its own.
    assert!(
        !ink(0, 200, 770, 400, 842),
        "unexpected number on the contents page"
    );
    // Between the entry text and the right margin: the leader and the entry's number.
    assert!(ink(0, 300, 72, 523, 120), "TOC leader and number missing");
}

#[test]
fn a_table_draws_its_cell_fill_and_borders() {
    use libertum_layout::LayoutState;
    use libertum_model::{BlockId, Document, Operation, Paragraph};

    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Before")]);
    let mut table = doc.new_table(1, 1, 300.0);
    table.rows[0].cells[0].fill = Some([220, 30, 30, 255]);
    table.rows[0].cells[0].paragraphs[0].insert_text(0, "inside");
    doc.apply(Operation::InsertTable { index: 1, table })
        .expect("insert table");
    let mut st = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    st.paginate_all();

    let pm = render_page(&st, 0, 1.0).expect("page 0");
    let px = |x: u32, y: u32| {
        let i = ((y * pm.width() + x) * 4) as usize;
        [pm.data()[i], pm.data()[i + 1], pm.data()[i + 2]]
    };
    let draw = st.pages()[0].tables.first().expect("table draw");
    let left = (st.geometry().margins.left + draw.x) as u32;
    let top = (st.geometry().margins.top + draw.y) as u32;
    // The cell fill is red inside the table.
    let [r, g, b] = px(left + 6, top + 6);
    assert!(r > 170 && g < 90 && b < 90, "fill was {r},{g},{b}");
    // The top border is dark.
    let [r, g, b] = px(left + 40, top);
    assert!(r < 120 && g < 120 && b < 120, "border was {r},{g},{b}");
}

fn image_state(media: std::sync::Arc<libertum_model::Media>, size_pt: (f32, f32)) -> LayoutState {
    use libertum_model::{BlockId, Document, InlineImage, Paragraph};
    let mut p = Paragraph::new(BlockId(1), "");
    p.set_images(vec![InlineImage {
        offset: 0,
        media,
        width_pt: size_pt.0,
        height_pt: size_pt.1,
        alt: String::new(),
        name: String::new(),
    }]);
    let doc = Document::from_paragraphs(vec![p]);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    s
}

fn pixel(pm: &libertum_render::Pixmap, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * pm.width() + x) * 4) as usize;
    [pm.data()[i], pm.data()[i + 1], pm.data()[i + 2]]
}

#[test]
fn an_inline_image_is_drawn_where_layout_put_it_at_any_scale() {
    // A solid red 40 x 20 px image shown at 200 x 100 pt (enlarged) and at 10 x 5 pt (shrunk).
    let red =
        libertum_formats_common::media::png_from_rgba(40, 20, [200u8, 30, 30, 255].repeat(40 * 20))
            .expect("png");
    for (size, scale) in [
        ((200.0, 100.0), 1.0f32),
        ((200.0, 100.0), 2.0),
        ((10.0, 5.0), 1.0),
    ] {
        let s = image_state(red.clone(), size);
        if !has_fonts(&s) {
            return;
        }
        let (x, y, w, h) = s
            .page_decorations(0)
            .into_iter()
            .find_map(|d| match d {
                libertum_layout::Decoration::Image {
                    x,
                    y,
                    width,
                    height,
                    ..
                } => Some((x, y, width, height)),
                _ => None,
            })
            .expect("image decoration");
        let mut renderer = PageRenderer::default();
        let pm = renderer.render_page(&s, 0, scale).expect("page");
        let centre = pixel(
            &pm,
            ((x + w / 2.0) * scale) as u32,
            ((y + h / 2.0) * scale) as u32,
        );
        assert!(
            centre[0] > 180 && centre[1] < 60 && centre[2] < 60,
            "centre of the image is red at {size:?} x{scale}: {centre:?}"
        );
        // Just outside the image the page is still white.
        let outside = pixel(
            &pm,
            ((x + w + 6.0) * scale) as u32,
            ((y + h / 2.0) * scale) as u32,
        );
        assert_eq!(outside, [255, 255, 255]);
        // Rendering again reuses the decode.
        renderer.render_page(&s, 0, scale).expect("page");
        assert_eq!(renderer.stats().image_decodes, 1);
        // A strip that does not reach the image never decodes it.
        let mut fresh = PageRenderer::default();
        fresh.render_band(&s, 0, scale, 0, 8).expect("band");
        assert_eq!(fresh.stats().image_decodes, 0);
    }
}

#[test]
fn a_damaged_image_draws_a_placeholder_instead_of_failing() {
    let media = std::sync::Arc::new(
        libertum_model::Media::new(libertum_model::test_png_header(20, 20)).expect("header"),
    );
    let s = image_state(media, (100.0, 100.0));
    if !has_fonts(&s) {
        return;
    }
    let pm = render_page(&s, 0, 1.0).expect("page still renders");
    let grey = pixel(&pm, 72 + 50, 72 + 50);
    assert_eq!(grey, [214, 214, 214]);
}

#[test]
fn a_large_image_is_downsampled_when_decoded() {
    let big = libertum_formats_common::media::png_from_rgba(
        3000,
        300,
        [10u8, 120, 10, 255].repeat(3000 * 300),
    )
    .expect("png");
    let decoded = libertum_render::images::decode(&big).expect("decodes");
    assert_eq!(decoded.width, libertum_render::images::MAX_DECODED_SIDE);
    assert!(
        decoded.height <= 205 && decoded.height >= 204,
        "{}",
        decoded.height
    );
}

#[test]
fn a_watermark_is_drawn_faintly_behind_the_page() {
    use libertum_model::{BlockId, Document, Paragraph, Watermark};
    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "")]);
    for diagonal in [true, false] {
        doc.sections[0].watermark = Watermark::new("WATERMARK", diagonal);
        let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
        s.paginate_all();
        if !has_fonts(&s) {
            return;
        }
        for scale in [1.0f32, 2.5] {
            let mut renderer = PageRenderer::default();
            let pm = renderer.render_page(&s, 0, scale).expect("page");
            // Pixels near the middle are grey: light, never black, never coloured.
            let (cx, cy) = (pm.width() / 2, pm.height() / 2);
            let mut grey = 0;
            for y in cy.saturating_sub(40)..cy + 40 {
                for x in cx.saturating_sub(200)..(cx + 200).min(pm.width()) {
                    let [r, g, b] = pixel(&pm, x, y);
                    if r < 250 {
                        grey += 1;
                        assert!(r > 150, "faint, not dark: {r} at {scale}");
                        assert!(r.abs_diff(g) <= 2 && g.abs_diff(b) <= 2, "grey");
                    }
                }
            }
            assert!(
                grey > 50,
                "the watermark shows (diagonal {diagonal}, x{scale}): {grey}"
            );
            // The corners stay white.
            assert_eq!(pixel(&pm, 2, 2), [255, 255, 255]);
            assert_eq!(pixel(&pm, pm.width() - 3, pm.height() - 3), [255, 255, 255]);
            // A strip has the same pixels as the whole page at that height.
            let band = renderer
                .render_band(&s, 0, scale, cy - 10, 20)
                .expect("band");
            for x in (0..pm.width()).step_by(7) {
                assert_eq!(pixel(&band, x, 10), pixel(&pm, x, cy));
            }
        }
    }
}

/// Distinct colours on a page: hard edges leave only the ink colours and the paper.
fn distinct_colours(pm: &libertum_render::Pixmap) -> usize {
    let mut seen = std::collections::HashSet::new();
    for p in pm.data().as_chunks::<4>().0 {
        seen.insert([p[0], p[1], p[2]]);
    }
    seen.len()
}

#[test]
fn turning_antialiasing_off_gives_hard_edges_and_a_separate_cache_entry() {
    let s = state();
    if !has_fonts(&s) {
        return;
    }
    let mut r = PageRenderer::default();
    let smooth = r.cached_page(&s, 0, 1.0).expect("page");
    let smooth_colours = distinct_colours(&smooth);
    assert!(
        smooth_colours > 30,
        "antialiased text has many shades: {smooth_colours}"
    );
    r.set_antialias(false);
    let hard = r.cached_page(&s, 0, 1.0).expect("page");
    assert!(
        !std::sync::Arc::ptr_eq(&smooth, &hard),
        "the cached smooth page must not be served"
    );
    let hard_colours = distinct_colours(&hard);
    assert!(
        hard_colours <= 8,
        "hard edges leave only paper and ink colours: {hard_colours}"
    );
    assert!(
        hard.data().as_chunks::<4>().0.iter().any(|p| p[0] < 100),
        "text is still drawn"
    );
    r.set_antialias(true);
    let again = r.cached_page(&s, 0, 1.0).expect("page");
    assert!(
        std::sync::Arc::ptr_eq(&smooth, &again),
        "switching back reuses the smooth page"
    );
}

#[test]
fn a_page_colour_fills_the_page_margins_included() {
    use libertum_model::{BlockId, Document, Paragraph};
    let mut doc = Document::from_paragraphs(vec![Paragraph::new(BlockId(1), "Body")]);
    doc.sections[0].page_colour = Some([0xF4, 0xB6, 0xB6, 0xFF]);
    let mut s = LayoutState::from_document(&doc, PageGeometry::a4(), LayoutOptions::default());
    s.paginate_all();
    assert_eq!(s.page_colour(0), Some([0xF4, 0xB6, 0xB6, 0xFF]));
    let pm = render_page(&s, 0, 1.0).expect("page 0");
    let i = ((5 * pm.width() + 5) * 4) as usize;
    assert_eq!(&pm.data()[i..i + 3], &[0xF4, 0xB6, 0xB6]);
}

fn vertical_state(text: &str) -> (LayoutState, PageGeometry) {
    let mut document =
        libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
            libertum_model::BlockId(1),
            text,
        )]);
    for section in &mut document.sections {
        section.vertical = true;
        section.language = Some("ja-JP".into());
    }
    let sheet = PageGeometry::from_setup(&document.page_setup());
    let mut state = LayoutState::from_document(&document, sheet, LayoutOptions::default());
    state.paginate_all();
    (state, sheet)
}

/// The bounding box of the non-white pixels: (left, top, right, bottom).
fn ink_box(pm: &tiny_skia::Pixmap) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = (pm.width(), pm.height());
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if pm.data()[i] < 200 {
                let b = found.get_or_insert((x, y, x, y));
                b.0 = b.0.min(x);
                b.1 = b.1.min(y);
                b.2 = b.2.max(x);
                b.3 = b.3.max(y);
            }
        }
    }
    found
}

#[test]
fn vertical_text_is_painted_on_the_real_sheet_from_the_top_right() {
    let (state, sheet) = vertical_state("縦書きの日本語です。");
    if !has_glyphs(&state) {
        return;
    }
    let scale = 1.0;
    let pm = libertum_render::render_page(&state, 0, scale).expect("page");
    // The sheet is the real page, portrait, not the turned layout page.
    assert_eq!(pm.width(), sheet.width_pt.ceil() as u32);
    assert_eq!(pm.height(), sheet.height_pt.ceil() as u32);
    let (left, top, right, bottom) = ink_box(&pm).expect("some ink");
    // One short line: a single column at the right margin, starting at the top margin and
    // running down, much taller than it is wide.
    let margin = 72.0;
    assert!(
        right as f32 > sheet.width_pt - margin - 30.0,
        "right {right}"
    );
    assert!(
        right as f32 <= sheet.width_pt - margin + 6.0,
        "right {right}"
    );
    assert!((top as f32 - margin).abs() < 12.0, "top {top}");
    assert!(
        bottom - top > 4 * (right - left),
        "{left} {top} {right} {bottom}"
    );
    // Bands cut across the columns and still agree with the whole page.
    let h = pm.height();
    let band = libertum_render::PageRenderer::default()
        .render_band(&state, 0, scale, h / 4, h / 4)
        .expect("band");
    let (from, rows) = (h / 4, h / 4);
    let whole_rows =
        &pm.data()[(from * pm.width() * 4) as usize..((from + rows) * pm.width() * 4) as usize];
    assert_eq!(
        band.data(),
        whole_rows,
        "a band is the same rows of the page"
    );
}

#[test]
fn horizontal_and_vertical_pages_of_the_same_text_differ_and_both_have_ink() {
    let text = "縦書きの日本語です。";
    let (vertical, _) = vertical_state(text);
    if !has_glyphs(&vertical) {
        return;
    }
    let mut document =
        libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
            libertum_model::BlockId(1),
            text,
        )]);
    for section in &mut document.sections {
        section.language = Some("ja-JP".into());
    }
    let sheet = PageGeometry::from_setup(&document.page_setup());
    let mut horizontal = LayoutState::from_document(&document, sheet, LayoutOptions::default());
    horizontal.paginate_all();
    let h = ink_box(&libertum_render::render_page(&horizontal, 0, 1.0).unwrap()).unwrap();
    let v = ink_box(&libertum_render::render_page(&vertical, 0, 1.0).unwrap()).unwrap();
    // Horizontal: a wide line at the top left. Vertical: a tall column at the top right.
    assert!(h.2 - h.0 > h.3 - h.1);
    assert!(v.3 - v.1 > v.2 - v.0);
    assert!(h.0 < v.0, "horizontal text starts left of vertical text");
}

#[test]
fn a_digit_stands_upright_in_vertical_text_where_a_letter_goes_sideways() {
    let (digit, _) = vertical_state("1");
    let (letter, _) = vertical_state("l");
    if !has_fonts(&digit) {
        return;
    }
    let scale = 4.0;
    let ink = |state: &LayoutState| {
        let pm = libertum_render::render_page(state, 0, scale).expect("page");
        let (l, t, r, b) = ink_box(&pm).expect("some ink");
        (r - l + 1, b - t + 1)
    };
    let (dw, dh) = ink(&digit);
    let (lw, lh) = ink(&letter);
    // The figure 1 is a tall mark, and upright it is taller than it is wide...
    assert!(dh as f32 > dw as f32 * 1.3, "upright digit {dw}x{dh}");
    // ...while a letter l, a tall mark too, is turned on its side with the column.
    assert!(lw as f32 > lh as f32 * 1.3, "sideways letter {lw}x{lh}");
}

#[test]
fn a_number_stacks_down_the_column_a_digit_to_a_cell() {
    let (state, _) = vertical_state("2026");
    if !has_fonts(&state) {
        return;
    }
    let pm = libertum_render::render_page(&state, 0, 3.0).expect("page");
    let (l, t, r, b) = ink_box(&pm).expect("some ink");
    // Four upright digits one under another: a narrow column, several times taller than wide.
    assert!((b - t) as f32 > (r - l) as f32 * 2.5, "{l} {t} {r} {b}");
}

/// A vertical document with one 120 x 40 pt red picture and no text.
fn vertical_image_state() -> LayoutState {
    use libertum_model::{BlockId, Document, InlineImage, Paragraph};
    let red =
        libertum_formats_common::media::png_from_rgba(60, 20, [200u8, 30, 30, 255].repeat(60 * 20))
            .expect("png");
    let mut p = Paragraph::new(BlockId(1), "");
    p.set_images(vec![InlineImage {
        offset: 0,
        media: red,
        width_pt: 120.0,
        height_pt: 40.0,
        alt: String::new(),
        name: String::new(),
    }]);
    let mut doc = Document::from_paragraphs(vec![p]);
    for section in &mut doc.sections {
        section.vertical = true;
    }
    let sheet = PageGeometry::from_setup(&doc.page_setup());
    let mut s = LayoutState::from_document(&doc, sheet, LayoutOptions::default());
    s.paginate_all();
    s
}

#[test]
fn a_picture_on_a_vertical_page_keeps_its_own_shape_upright() {
    let state = vertical_image_state();
    if !has_fonts(&state) {
        return;
    }
    let pm = render_page(&state, 0, 1.0).expect("page");
    // The red box of the picture.
    let (w, h) = (pm.width(), pm.height());
    let mut found: Option<(u32, u32, u32, u32)> = None;
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = pixel(&pm, x, y);
            if r > 150 && g < 80 && b < 80 {
                let bb = found.get_or_insert((x, y, x, y));
                bb.0 = bb.0.min(x);
                bb.1 = bb.1.min(y);
                bb.2 = bb.2.max(x);
                bb.3 = bb.3.max(y);
            }
        }
    }
    let (l, t, r, b) = found.expect("the picture is drawn");
    // 120 pt wide and 40 pt tall on the sheet, not turned on its side (40 x 120).
    assert!((r - l + 1).abs_diff(120) <= 2, "width {}", r - l + 1);
    assert!((b - t + 1).abs_diff(40) <= 2, "height {}", b - t + 1);
    // And it is in the layout's turned box: at the right margin, from the top margin.
    assert!(r as f32 > 595.0 - 72.0 - 125.0, "right edge {r}");
}

#[test]
fn digits_of_a_number_do_not_overlap_down_the_column() {
    let (one, _) = vertical_state("2");
    let (four, _) = vertical_state("2026");
    if !has_fonts(&one) {
        return;
    }
    let height = |state: &LayoutState| {
        let pm = libertum_render::render_page(state, 0, 4.0).expect("page");
        let (_, t, _, b) = ink_box(&pm).expect("some ink");
        (b - t + 1) as f32
    };
    // Four digits are four cells long, each about an em: without room between them they would
    // overlap and be barely taller than one (they are taller than they are wide).
    assert!(
        height(&four) > height(&one) * 3.3,
        "{} {}",
        height(&four),
        height(&one)
    );
}

fn vertical_furniture_state() -> (LayoutState, PageGeometry) {
    let mut document =
        libertum_model::Document::from_paragraphs(vec![libertum_model::Paragraph::new(
            libertum_model::BlockId(1),
            "縦書き",
        )]);
    for section in &mut document.sections {
        section.vertical = true;
        section.headers = libertum_model::HeadersFooters::with_default("HEADER TEXT");
        section.footers = libertum_model::HeadersFooters::with_default("FOOTER TEXT");
    }
    let sheet = PageGeometry::from_setup(&document.page_setup());
    let mut state = LayoutState::from_document(&document, sheet, LayoutOptions::default());
    state.paginate_all();
    (state, sheet)
}

#[test]
fn a_vertical_pages_header_and_footer_are_upright_at_the_top_and_bottom_of_the_sheet() {
    let (state, sheet) = vertical_furniture_state();
    if !has_fonts(&state) {
        return;
    }
    let pm = libertum_render::render_page(&state, 0, 1.0).expect("page");
    let (w, h) = (pm.width(), pm.height());
    // The ink in the top fifth and in the bottom fifth of the sheet, apart from the column.
    let band_box = |from: u32, to: u32| {
        let mut found: Option<(u32, u32, u32, u32)> = None;
        for y in from..to {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                if pm.data()[i] < 200 {
                    let b = found.get_or_insert((x, y, x, y));
                    b.0 = b.0.min(x);
                    b.1 = b.1.min(y);
                    b.2 = b.2.max(x);
                    b.3 = b.3.max(y);
                }
            }
        }
        found
    };
    let header = band_box(0, 60).expect("a header in the top of the sheet");
    let footer = band_box(h - 70, h).expect("a footer at the bottom of the sheet");
    // Horizontal text: a line much wider than it is tall, starting at the left margin.
    for (name, (l, t, r, b)) in [("header", header), ("footer", footer)] {
        assert!(
            r - l > 4 * (b - t),
            "{name} is a horizontal line: {l} {t} {r} {b}"
        );
        assert!(
            (l as f32 - sheet.margins.left).abs() < 6.0,
            "{name} starts at the margin: {l}"
        );
    }
}

fn ink_size(state: &LayoutState, scale: f32) -> (f32, f32) {
    let pm = libertum_render::render_page(state, 0, scale).expect("page");
    let (l, t, r, b) = ink_box(&pm).expect("some ink");
    ((r - l + 1) as f32, (b - t + 1) as f32)
}

#[test]
fn a_two_digit_number_is_set_in_one_cell_and_a_longer_one_is_stacked() {
    let (one, _) = vertical_state("1");
    let (pair, _) = vertical_state("11");
    let (three, _) = vertical_state("111");
    if !has_fonts(&one) {
        return;
    }
    let (w1, h1) = ink_size(&one, 4.0);
    let (w2, h2) = ink_size(&pair, 4.0);
    let (w3, h3) = ink_size(&three, 4.0);
    // 11: both digits in one cell, so no taller than a single digit but about twice as wide.
    assert!(h2 < h1 * 1.25, "pair is one cell tall: {h2} vs {h1}");
    assert!(w2 > w1 * 1.5, "pair is two digits wide: {w2} vs {w1}");
    // 111: a digit to a cell, down the column.
    assert!(h3 > h1 * 2.5, "three are stacked: {h3} vs {h1}");
    assert!(w3 < w1 * 1.3, "stacked digits are one wide: {w3} vs {w1}");
}

#[test]
fn a_decimal_point_sits_in_its_own_cell_between_the_digits() {
    let (decimal, _) = vertical_state("3.1");
    let (plain, _) = vertical_state("31");
    if !has_fonts(&decimal) {
        return;
    }
    // 3.1 is three cells down the column (3, the point, 1); 31 is one cell (a pair).
    let (_, h_decimal) = ink_size(&decimal, 4.0);
    let (_, h_plain) = ink_size(&plain, 4.0);
    assert!(h_decimal > h_plain * 2.2, "{h_decimal} vs {h_plain}");
}

fn vertical_picture_beside_text() -> LayoutState {
    use libertum_model::{BlockId, Document, InlineImage, Paragraph};
    let red =
        libertum_formats_common::media::png_from_rgba(60, 20, [200u8, 30, 30, 255].repeat(60 * 20))
            .expect("png");
    let mut p = Paragraph::new(BlockId(1), "あ\u{3042}い");
    p.set_images(vec![InlineImage {
        offset: 3,
        media: red,
        width_pt: 60.0,
        height_pt: 30.0,
        alt: String::new(),
        name: String::new(),
    }]);
    let mut doc = Document::from_paragraphs(vec![p]);
    for section in &mut doc.sections {
        section.vertical = true;
        section.language = Some("ja-JP".into());
    }
    let sheet = PageGeometry::from_setup(&doc.page_setup());
    let mut s = LayoutState::from_document(&doc, sheet, LayoutOptions::default());
    s.paginate_all();
    s
}

#[test]
fn a_picture_in_a_column_is_centred_on_the_text_not_hung_from_its_left() {
    let state = vertical_picture_beside_text();
    if !has_glyphs(&state) {
        return;
    }
    let pm = libertum_render::render_page(&state, 0, 2.0).expect("page");
    let (w, h) = (pm.width(), pm.height());
    let (mut red, mut dark): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = pixel(&pm, x, y);
            if r > 150 && g < 80 && b < 80 {
                red.push(x);
            } else if r < 100 && g < 100 && b < 100 {
                dark.push(x);
            }
        }
    }
    let middle = |xs: &[u32]| (*xs.iter().min().unwrap() + *xs.iter().max().unwrap()) as f32 / 2.0;
    // Centres across the column, in device pixels at 2 px a point. The 60 pt wide picture and the
    // 11 pt text share a centre line (within 3 pt), where hung from the left it would be 24 pt off.
    let gap = (middle(&red) - middle(&dark)).abs() / 2.0;
    assert!(gap < 3.0, "picture and text centres are {gap} pt apart");
}
