//! What the page renderer and the PDF writer share about vertical text.
//!
//! Vertical text is laid out on the page turned a quarter turn (`PageGeometry::transposed`),
//! so a run of Latin letters is already sideways once the page is turned back. Ideographs,
//! kana, Hangul, East Asian punctuation and numbers must stand upright, so each glyph is told
//! apart by the character it stands for.

use libertum_layout::Brush;
use libertum_layout::parley::layout::Run;

/// How a glyph is set on a vertical page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stand {
    /// Turned a quarter turn with the page: Latin and Cyrillic text.
    Sideways,
    /// Upright in its cell: an East Asian character.
    Upright,
    /// Upright, and part of a number: it was given room after it to make its cell about an em
    /// down the column (`NUMBER_CELL_SPACING_EM`), which is not part of the glyph's own width.
    Number,
    /// One of the two digits of a number set horizontally in a single cell (tate-chu-yoko);
    /// 0 for the first digit and 1 for the second.
    Tcy(u8),
}

impl Stand {
    pub(crate) fn upright(self) -> bool {
        self != Stand::Sideways
    }

    /// The room added after the glyph at `font_size` points, which centring must leave out.
    pub(crate) fn extra_advance(self, font_size: f32) -> f32 {
        match self {
            Stand::Number => libertum_layout::script::NUMBER_CELL_SPACING_EM * font_size,
            _ => 0.0,
        }
    }
}

/// For each glyph of `run`, in drawing order, how it is set on a vertical page: East Asian
/// characters upright, numbers (each digit in its own cell down the column) upright unless they
/// belong to a Latin word, everything else sideways. A glyph for several characters (a
/// ligature) goes by the first.
pub(crate) fn upright_glyphs(paragraph_text: &str, run: &Run<'_, Brush>) -> Vec<Stand> {
    let mut stands = Vec::new();
    for cluster in run.clusters() {
        let range = cluster.text_range();
        let first = paragraph_text
            .get(range.clone())
            .and_then(|text| text.chars().next());
        let stand = if first.is_some_and(libertum_layout::script::is_upright_in_vertical) {
            Stand::Upright
        } else if let Some(pair) = libertum_layout::script::tcy_pair_at(paragraph_text, range.start)
        {
            Stand::Tcy(u8::from(range.start != pair.start))
        } else if libertum_layout::script::number_stands_upright(paragraph_text, range.start) {
            Stand::Number
        } else {
            Stand::Sideways
        };
        stands.extend(cluster.glyphs().map(|_| stand));
    }
    stands
}

/// The cell an upright glyph is set in, in layout points along the line.
pub(crate) struct UprightCell {
    /// Where the cell starts along the line (the layout x of its first glyph).
    pub start: f32,
    /// How long the cell is down the column.
    pub len: f32,
    /// How wide what is drawn in it is across the column: a glyph's own width, or both digits
    /// of a number set in one cell.
    pub width: f32,
    /// How far across the column this glyph is from the left edge of that width.
    pub offset: f32,
}

/// The cell for the glyph `index` of `positions`, `stand` telling how it is set. The two digits
/// of a number set in one cell share it, side by side; every other glyph has a cell of its own
/// advance, and a digit's own width leaves out the room added after it.
pub(crate) fn upright_cell(
    stand: Stand,
    positions: &[libertum_layout::parley::Glyph],
    index: usize,
    font_size: f32,
) -> UprightCell {
    let glyph = &positions[index];
    match stand {
        Stand::Tcy(0) => {
            let width = glyph.advance + positions.get(index + 1).map_or(0.0, |next| next.advance);
            UprightCell {
                start: glyph.x,
                len: width,
                width,
                offset: 0.0,
            }
        }
        Stand::Tcy(_) => {
            let first = &positions[index.saturating_sub(1)];
            let width = first.advance + glyph.advance;
            UprightCell {
                start: first.x,
                len: width,
                width,
                offset: first.advance,
            }
        }
        _ => UprightCell {
            start: glyph.x,
            len: glyph.advance,
            width: (glyph.advance - stand.extra_advance(font_size)).max(0.0),
            offset: 0.0,
        },
    }
}

/// Where an upright glyph's origin goes, for a glyph whose layout-page position is
/// (`x_layout` for the start of its `cell`, `y_layout` for its baseline). The em box (`ascent`
/// above the baseline, `descent` below) is centred in the cell down the column, and what is
/// drawn there is centred across the column. Returns the origin's position down the sheet (the
/// band's shift not yet taken off) and across it from the left, both in points;
/// `sheet_width` is the sheet's width.
pub(crate) fn upright_origin(
    x_layout: f32,
    y_layout: f32,
    cell: &UprightCell,
    ascent: f32,
    descent: f32,
    sheet_width: f32,
) -> (f32, f32) {
    let centre_across = y_layout - (ascent - descent) * 0.5;
    (
        x_layout + cell.len * 0.5 + (ascent - descent) * 0.5,
        sheet_width - centre_across - cell.width * 0.5 + cell.offset,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_upright_glyph_is_centred_on_its_column_and_hangs_from_its_cell() {
        // A 10 pt glyph on a baseline 200 pt from the layout page's top, 100 pt along the
        // line, with a 0.88 / 0.12 em box, on a sheet 595 pt wide.
        let cell = UprightCell {
            start: 100.0,
            len: 10.0,
            width: 10.0,
            offset: 0.0,
        };
        let (cell, across) = upright_origin(100.0, 200.0, &cell, 8.8, 1.2, 595.0);
        // The cell starts at 100 pt down the sheet; the origin is the ascent below that.
        assert!((cell - 108.8).abs() < 1e-4);
        // The column's centre is where the em box's middle is: 3.8 pt above the baseline, so
        // 3.8 pt off the baseline's own distance, turned: 595 - 196.2 = 398.8 from the left,
        // and the glyph's left edge is half its advance before that.
        assert!((across - (398.8 - 5.0)).abs() < 1e-3, "{across}");
    }
}

#[cfg(test)]
mod cell_tests {
    use super::*;
    use libertum_layout::parley::Glyph;

    fn glyph(x: f32, advance: f32) -> Glyph {
        Glyph {
            id: 1,
            x,
            y: 0.0,
            advance,
        }
    }

    #[test]
    fn two_digits_of_a_number_share_one_cell_side_by_side() {
        // Two 5.5 pt digits starting 40 pt along the line.
        let positions = [glyph(40.0, 5.5), glyph(45.5, 5.5)];
        let first = upright_cell(Stand::Tcy(0), &positions, 0, 11.0);
        let second = upright_cell(Stand::Tcy(1), &positions, 1, 11.0);
        for cell in [&first, &second] {
            assert_eq!((cell.start, cell.len, cell.width), (40.0, 11.0, 11.0));
        }
        assert_eq!((first.offset, second.offset), (0.0, 5.5));
        // Same place down the column, 5.5 pt apart across it.
        let (along_a, across_a) = upright_origin(40.0, 0.0, &first, 8.8, 1.2, 100.0);
        let (along_b, across_b) = upright_origin(40.0, 0.0, &second, 8.8, 1.2, 100.0);
        assert_eq!(along_a, along_b);
        assert!((across_b - across_a - 5.5).abs() < 1e-4);
    }

    #[test]
    fn a_stacked_digit_is_centred_in_a_cell_of_its_width_plus_the_room_after_it() {
        // A 5.5 pt digit given 0.45 em (4.95 pt) after it at 11 pt.
        let positions = [glyph(10.0, 5.5 + 4.95)];
        let cell = upright_cell(Stand::Number, &positions, 0, 11.0);
        assert!((cell.len - 10.45).abs() < 1e-4);
        assert!(
            (cell.width - 5.5).abs() < 1e-4,
            "the room after it is not part of its width"
        );
        // The em box is centred in the cell down the column, not hung from its start: a
        // digit's cell is shorter than its em box, and hanging it would push it into the next.
        let (along, _) = upright_origin(10.0, 0.0, &cell, 9.5, 2.5, 100.0);
        assert!((along - (10.0 + 10.45 * 0.5 + 3.5)).abs() < 1e-4);
    }

    #[test]
    fn an_ideograph_still_hangs_from_the_start_of_its_cell() {
        // A 11 pt ideograph with a 0.88 / 0.12 em box fills its cell exactly.
        let positions = [glyph(10.0, 11.0)];
        let cell = upright_cell(Stand::Upright, &positions, 0, 11.0);
        let (along, _) = upright_origin(10.0, 0.0, &cell, 9.68, 1.32, 100.0);
        assert!((along - (10.0 + 9.68)).abs() < 1e-4, "{along}");
    }
}
