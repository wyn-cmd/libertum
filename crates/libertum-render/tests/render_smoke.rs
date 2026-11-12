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
    s.set_sections(vec![
        Section::default(),
        Section {
            page_numbers: Some(PageNumberPos::BOTTOM_CENTRE),
            first_page_number: Some(1),
            ..Section::default()
        },
    ]);
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
