//! The toolbar / inspector state model (`AppState`).
//!
//! Nothing here is bound to a document yet. Every value that will later reflect the selection
//! is wrapped in [`Mixed`], so "the selection spans two fonts" can be represented.

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use libertum_model::{StyleId, VerticalAlign};

use serde::{Deserialize, Serialize};

use crate::strings as s;
use crate::widgets::stepper::StepSpec;

/// A control value that is either uniform across the selection or indeterminate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Mixed` is constructed once controls bind to a selection (Phase 2)
pub enum Mixed<T> {
    Value(T),
    Mixed,
}

impl<T: Copy> Mixed<T> {
    /// The uniform value, or `None` when the selection is mixed.
    pub fn value(self) -> Option<T> {
        match self {
            Self::Value(v) => Some(v),
            Self::Mixed => None,
        }
    }
}

impl<T> Mixed<T> {
    pub fn is_mixed(&self) -> bool {
        matches!(self, Self::Mixed)
    }

    /// The uniform value by reference (for values that are not `Copy`).
    pub fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Value(v) => Some(v),
            Self::Mixed => None,
        }
    }
}

impl<T> From<T> for Mixed<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}

impl Mixed<bool> {
    /// Clicking a toggle: mixed becomes on, otherwise flip.
    pub fn toggled(self) -> Self {
        Self::Value(!matches!(self, Self::Value(true)))
    }
}

/// Declares a fieldless enum with a display label and an `ALL` list for pick lists.
macro_rules! choice_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $label:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            #[allow(dead_code)]
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self { $(Self::$variant => $label),+ })
            }
        }
    };
}

/// Which inspector the sidebar shows. Persisted in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Inspector {
    #[default]
    Format,
    Document,
    Headers,
    Notes,
    Bookmarks,
}

choice_enum!(FormatPane {
    Style => s::PANE_STYLE,
    Layout => s::PANE_LAYOUT,
    More => s::PANE_MORE,
});

/// An AO3 tag category in a popup. Iced's `pick_list` needs `Display`, which the export
/// crate's own enum cannot implement here, and the labels belong in `strings.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TagCategoryChoice(pub libertum_export::TagCategory);

impl TagCategoryChoice {
    pub const ALL: &'static [Self] = &[
        Self(libertum_export::TagCategory::Rating),
        Self(libertum_export::TagCategory::Warning),
        Self(libertum_export::TagCategory::Fandom),
        Self(libertum_export::TagCategory::Relationship),
        Self(libertum_export::TagCategory::Character),
        Self(libertum_export::TagCategory::Freeform),
    ];
}

impl fmt::Display for TagCategoryChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use libertum_export::TagCategory;
        f.write_str(match self.0 {
            TagCategory::Rating => s::TAG_CATEGORY_RATING,
            TagCategory::Warning => s::TAG_CATEGORY_WARNING,
            TagCategory::Fandom => s::TAG_CATEGORY_FANDOM,
            TagCategory::Relationship => s::TAG_CATEGORY_RELATIONSHIP,
            TagCategory::Character => s::TAG_CATEGORY_CHARACTER,
            TagCategory::Freeform => s::TAG_CATEGORY_FREEFORM,
        })
    }
}
choice_enum!(DocumentPane {
    Document => s::PANE_DOCUMENT,
    Section => s::PANE_SECTION,
});

/// Toolbar dropdown menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Menu {
    View,
    Zoom,
    Insert,
    /// Save a copy in another format, or copy an AO3 chapter.
    Export,
}

/// Which swatch popover is expanded in the Style pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SwatchTarget {
    Character,
    Highlight,
}

choice_enum!(ParagraphStyle {
    Title => s::STYLE_TITLE,
    Heading1 => s::STYLE_H1,
    Heading2 => s::STYLE_H2,
    Heading3 => s::STYLE_H3,
    Heading4 => s::STYLE_H4,
    Body => s::STYLE_BODY,
    Quote => s::STYLE_QUOTE,
});

impl ParagraphStyle {
    /// The built-in style in the document's style sheet.
    pub fn id(self) -> StyleId {
        match self {
            Self::Title => StyleId::TITLE,
            Self::Heading1 => StyleId::HEADING_1,
            Self::Heading2 => StyleId::HEADING_2,
            Self::Heading3 => StyleId::HEADING_3,
            Self::Heading4 => StyleId::HEADING_4,
            Self::Body => StyleId::BODY,
            Self::Quote => StyleId::QUOTE,
        }
    }

    pub fn from_id(id: StyleId) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.id() == id)
    }
}

choice_enum!(Baseline {
    Default => s::BASELINE_DEFAULT,
    Superscript => s::BASELINE_SUPERSCRIPT,
    Subscript => s::BASELINE_SUBSCRIPT,
});

impl Baseline {
    pub fn vertical(self) -> VerticalAlign {
        match self {
            Self::Default => VerticalAlign::Baseline,
            Self::Superscript => VerticalAlign::Superscript,
            Self::Subscript => VerticalAlign::Subscript,
        }
    }

    pub fn from_vertical(v: VerticalAlign) -> Self {
        match v {
            VerticalAlign::Baseline => Self::Default,
            VerticalAlign::Superscript => Self::Superscript,
            VerticalAlign::Subscript => Self::Subscript,
        }
    }
}

/// A font family name, as listed in the font popup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontName(pub Arc<str>);

impl fmt::Display for FontName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

choice_enum!(FontWeight {
    Light => s::FONT_WEIGHT_LIGHT,
    Regular => s::FONT_WEIGHT_REGULAR,
    Medium => s::FONT_WEIGHT_MEDIUM,
    Semibold => s::FONT_WEIGHT_SEMIBOLD,
    Bold => s::FONT_WEIGHT_BOLD,
});

impl Swatch {
    /// The document colour for text in this swatch ("Black" is true black on the page).
    pub fn text_rgba(self) -> [u8; 4] {
        match self {
            Swatch::Black => [0, 0, 0, 255],
            Swatch::Grey => [0x8e, 0x8e, 0x93, 255],
            Swatch::Red => [0xff, 0x3b, 0x30, 255],
            Swatch::Orange => [0xff, 0x95, 0x00, 255],
            Swatch::Yellow => [0xff, 0xd6, 0x0a, 255],
            Swatch::Green => [0x34, 0xc7, 0x59, 255],
            Swatch::Blue => [0x00, 0x7a, 0xff, 255],
            Swatch::Purple => [0xaf, 0x52, 0xde, 255],
        }
    }

    /// The highlight colour: the swatch mixed halfway to white, so text stays readable.
    pub fn highlight_rgba(self) -> [u8; 4] {
        let [r, g, b, _] = self.text_rgba();
        let lift = |c: u8| ((u16::from(c) + 255) / 2) as u8;
        [lift(r), lift(g), lift(b), 255]
    }

    pub fn from_text_rgba(rgba: [u8; 4]) -> Option<Swatch> {
        Self::ALL.iter().copied().find(|s| s.text_rgba() == rgba)
    }

    pub fn from_highlight_rgba(rgba: [u8; 4]) -> Option<Swatch> {
        Self::ALL
            .iter()
            .copied()
            .find(|s| s.highlight_rgba() == rgba)
    }
}

impl FontWeight {
    /// CSS / OpenType weight.
    pub fn value(self) -> u16 {
        match self {
            FontWeight::Light => 300,
            FontWeight::Regular => 400,
            FontWeight::Medium => 500,
            FontWeight::Semibold => 600,
            FontWeight::Bold => 700,
        }
    }

    /// The nearest named weight.
    pub fn nearest(weight: u16) -> FontWeight {
        match weight {
            0..=349 => FontWeight::Light,
            350..=449 => FontWeight::Regular,
            450..=549 => FontWeight::Medium,
            550..=649 => FontWeight::Semibold,
            _ => FontWeight::Bold,
        }
    }
}

choice_enum!(Swatch {
    Black => s::SWATCH_BLACK,
    Grey => s::SWATCH_GREY,
    Red => s::SWATCH_RED,
    Orange => s::SWATCH_ORANGE,
    Yellow => s::SWATCH_YELLOW,
    Green => s::SWATCH_GREEN,
    Blue => s::SWATCH_BLUE,
    Purple => s::SWATCH_PURPLE,
});

choice_enum!(Alignment {
    Left => s::ALIGN_LEFT,
    Centre => s::ALIGN_CENTRE,
    Right => s::ALIGN_RIGHT,
    Justify => s::ALIGN_JUSTIFY,
});

choice_enum!(ListKind {
    None => s::LIST_NONE,
    Bulleted => s::LIST_BULLETED,
    Numbered => s::LIST_NUMBERED,
    Lettered => s::LIST_LETTERED,
    Harvard => s::LIST_HARVARD,
});

impl ListKind {
    pub fn model(self) -> Option<libertum_model::ListStyle> {
        use libertum_model::ListStyle;
        match self {
            Self::None => None,
            Self::Bulleted => Some(ListStyle::Bullet),
            Self::Numbered => Some(ListStyle::Numbered),
            Self::Lettered => Some(ListStyle::Lettered),
            Self::Harvard => Some(ListStyle::Harvard),
        }
    }

    pub fn from_model(style: Option<libertum_model::ListStyle>) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.model() == style)
            .unwrap_or(Self::None)
    }
}

impl Ligature {
    pub fn model(self) -> libertum_model::Ligatures {
        match self {
            Self::Default => libertum_model::Ligatures::Standard,
            Self::None => libertum_model::Ligatures::None,
            Self::All => libertum_model::Ligatures::All,
        }
    }

    pub fn from_model(l: libertum_model::Ligatures) -> Self {
        match l {
            libertum_model::Ligatures::Standard => Self::Default,
            libertum_model::Ligatures::None => Self::None,
            libertum_model::Ligatures::All => Self::All,
        }
    }
}

choice_enum!(LineSpacingMode {
    Lines => s::SPACING_LINES,
    AtLeast => s::SPACING_AT_LEAST,
    Exactly => s::SPACING_EXACTLY,
    Between => s::SPACING_BETWEEN,
});

choice_enum!(Ligature {
    Default => s::LIGATURE_DEFAULT,
    None => s::LIGATURE_NONE,
    All => s::LIGATURE_ALL,
});

choice_enum!(BorderStyle {
    None => s::BORDER_NONE,
    Thin => s::BORDER_THIN,
    Medium => s::BORDER_MEDIUM,
    Thick => s::BORDER_THICK,
});

impl BorderStyle {
    /// The border box this preset applies: black lines on all four sides, or none.
    pub fn borders(self) -> libertum_model::Borders {
        match self {
            Self::None => libertum_model::Borders::none(),
            Self::Thin => libertum_model::Borders::line(0.5),
            Self::Medium => libertum_model::Borders::line(1.0),
            Self::Thick => libertum_model::Borders::line(2.0),
        }
    }

    /// The line width this preset draws; zero for none.
    pub fn width(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Thin => 0.5,
            Self::Medium => 1.0,
            Self::Thick => 2.0,
        }
    }

    /// Nearest preset for a resolved border box (including no box at all).
    pub fn from_borders(borders: Option<libertum_model::Borders>) -> Self {
        let Some(borders) = borders.filter(|b| b.is_visible()) else {
            return Self::None;
        };
        if borders.width_pt <= 0.75 {
            Self::Thin
        } else if borders.width_pt <= 1.5 {
            Self::Medium
        } else {
            Self::Thick
        }
    }
}

choice_enum!(PageSize {
    A4 => s::PAGE_SIZE_A4,
    Letter => s::PAGE_SIZE_LETTER,
    A5 => s::PAGE_SIZE_A5,
    Legal => s::PAGE_SIZE_LEGAL,
});

impl PageSize {
    /// Portrait size in millimetres (width, height).
    pub fn millimetres(self) -> (f32, f32) {
        match self {
            Self::A4 => (210.0, 297.0),
            Self::Letter => (215.9, 279.4),
            Self::A5 => (148.0, 210.0),
            Self::Legal => (215.9, 355.6),
        }
    }

    pub fn model(self) -> libertum_model::PaperSize {
        use libertum_model::PaperSize as Paper;
        match self {
            Self::A4 => Paper::A4,
            Self::Letter => Paper::Letter,
            Self::A5 => Paper::A5,
            Self::Legal => Paper::Legal,
        }
    }

    pub fn from_model(paper: libertum_model::PaperSize) -> Self {
        use libertum_model::PaperSize as Paper;
        match paper {
            Paper::A4 => Self::A4,
            Paper::Letter => Self::Letter,
            Paper::A5 => Self::A5,
            Paper::Legal => Self::Legal,
        }
    }
}

choice_enum!(Orientation {
    Portrait => s::ORIENT_PORTRAIT,
    Landscape => s::ORIENT_LANDSCAPE,
});

impl Orientation {
    pub fn model(self) -> libertum_model::Orientation {
        use libertum_model::Orientation as Model;
        match self {
            Self::Portrait => Model::Portrait,
            Self::Landscape => Model::Landscape,
        }
    }

    pub fn from_model(orientation: libertum_model::Orientation) -> Self {
        use libertum_model::Orientation as Model;
        match orientation {
            Model::Portrait => Self::Portrait,
            Model::Landscape => Self::Landscape,
        }
    }
}

choice_enum!(MarginSide {
    Top => s::MARGIN_TOP,
    Bottom => s::MARGIN_BOTTOM,
    Left => s::MARGIN_LEFT,
    Right => s::MARGIN_RIGHT,
});

/// Measurement unit for lengths shown in the inspector. Lengths are stored in centimetres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Units {
    #[default]
    Centimetres,
    Inches,
}

impl Units {
    /// Centimetres per displayed unit.
    pub fn cm_per_unit(self) -> f32 {
        match self {
            Self::Centimetres => 1.0,
            Self::Inches => 2.54,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Centimetres => s::UNIT_CM,
            Self::Inches => s::UNIT_IN,
        }
    }

    /// Guess from a POSIX-style locale name such as `en_US.UTF-8`. The United States, Liberia
    /// and Myanmar use imperial measures; everything else (and anything unrecognised) is
    /// metric, which is the default.
    pub fn from_locale(locale: &str) -> Self {
        let region = locale
            .split(['.', '@'])
            .next()
            .and_then(|l| l.split(['_', '-']).nth(1))
            .unwrap_or("");
        if ["US", "LR", "MM"]
            .iter()
            .any(|r| r.eq_ignore_ascii_case(region))
        {
            Self::Inches
        } else {
            Self::Centimetres
        }
    }

    /// Units from the environment (`LIBERTUM_UNITS=cm|in`, then the usual locale variables).
    pub fn detect() -> Self {
        match std::env::var("LIBERTUM_UNITS").ok().as_deref() {
            Some("in") => return Self::Inches,
            Some("cm") => return Self::Centimetres,
            _ => {}
        }
        for var in ["LC_MEASUREMENT", "LC_ALL", "LANG"] {
            if let Ok(value) = std::env::var(var)
                && !value.is_empty()
            {
                return Self::from_locale(&value);
            }
        }
        Self::Centimetres
    }
}

/// Points (font size, paragraph spacing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Points(pub u16);

/// Line spacing multiple, in hundredths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineSpacing(pub u16);

/// A margin or distance, in hundredths of a centimetre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Margin(pub u16);

impl Margin {
    /// Points, for the document model (which stores lengths in points).
    pub fn to_points(self) -> f32 {
        f32::from(self.0) / 100.0 * 72.0 / 2.54
    }

    /// Hundredths of a centimetre, from points, rounded and clamped to the stepper's range.
    pub fn from_points(points: f32) -> Self {
        Self(
            (points * 2.54 / 72.0 * 100.0)
                .round()
                .clamp(0.0, f32::from(u16::MAX)) as u16,
        )
    }
}

/// Fonts listed before a document has been opened (the font popup then lists every family
/// installed on the machine, from the layout engine's font collection).
pub const FONT_FAMILIES: &[&str] = &[
    "Arial",
    "Calibri",
    "Cambria",
    "Courier New",
    "Georgia",
    "Times New Roman",
    "Verdana",
];

#[derive(Debug, Clone, PartialEq)]
pub struct FormatState {
    pub paragraph_style: Mixed<ParagraphStyle>,
    /// The selection's paragraph formatting differs from its style ("Update" is enabled).
    pub style_differs: bool,
    pub font_family: Mixed<FontName>,
    pub font_weight: Mixed<FontWeight>,
    pub font_size: Mixed<Points>,
    pub bold: Mixed<bool>,
    pub italic: Mixed<bool>,
    pub underline: Mixed<bool>,
    pub strikethrough: Mixed<bool>,
    pub colour: Mixed<Swatch>,
    pub highlight: Mixed<Option<Swatch>>,
    pub baseline: Mixed<Baseline>,
    /// Character spacing, tenths of a point.
    pub char_spacing: Mixed<i16>,
    pub alignment: Mixed<Alignment>,
    pub line_spacing_mode: Mixed<LineSpacingMode>,
    /// Line spacing in "Lines" mode (a multiple).
    pub line_spacing: Mixed<LineSpacing>,
    /// Line spacing in the "At least", "Exactly" and "Between" modes, points.
    pub line_spacing_pt: Mixed<Points>,
    pub space_before: Mixed<Points>,
    pub space_after: Mixed<Points>,
    pub list_kind: Mixed<ListKind>,
    // Layout pane (centimetres).
    pub columns: Mixed<u16>,
    pub gutter_cm: Mixed<f32>,
    pub indent_first_cm: Mixed<f32>,
    pub indent_left_cm: Mixed<f32>,
    pub indent_right_cm: Mixed<f32>,
    // More pane.
    pub keep_lines_together: Mixed<bool>,
    pub keep_with_next: Mixed<bool>,
    pub page_break_before: Mixed<bool>,
    pub widow_orphan: Mixed<bool>,
    pub remove_hyphenation: Mixed<bool>,
    pub ligatures: Mixed<Ligature>,
    /// Border box around the selected paragraphs (Layout pane).
    pub border_style: Mixed<BorderStyle>,
    /// Which sides the border style draws, and its colour.
    pub border_sides: libertum_model::BorderSides,
    pub border_colour: [u8; 4],
}

impl Default for FormatState {
    fn default() -> Self {
        Self {
            paragraph_style: Mixed::Value(ParagraphStyle::Body),
            style_differs: false,
            font_family: Mixed::Value(FontName(Arc::from("Times New Roman"))),
            font_weight: Mixed::Value(FontWeight::Regular),
            font_size: Mixed::Value(Points(12)),
            bold: Mixed::Value(false),
            italic: Mixed::Value(false),
            underline: Mixed::Value(false),
            strikethrough: Mixed::Value(false),
            colour: Mixed::Value(Swatch::Black),
            highlight: Mixed::Value(None),
            baseline: Mixed::Value(Baseline::Default),
            char_spacing: Mixed::Value(0),
            alignment: Mixed::Value(Alignment::Left),
            line_spacing_mode: Mixed::Value(LineSpacingMode::Lines),
            line_spacing: Mixed::Value(LineSpacing(100)),
            line_spacing_pt: Mixed::Value(Points(14)),
            space_before: Mixed::Value(Points(0)),
            space_after: Mixed::Value(Points(8)),
            list_kind: Mixed::Value(ListKind::None),
            columns: Mixed::Value(1),
            gutter_cm: Mixed::Value(0.64),
            indent_first_cm: Mixed::Value(0.0),
            indent_left_cm: Mixed::Value(0.0),
            indent_right_cm: Mixed::Value(0.0),
            keep_lines_together: Mixed::Value(false),
            keep_with_next: Mixed::Value(false),
            page_break_before: Mixed::Value(false),
            widow_orphan: Mixed::Value(true),
            remove_hyphenation: Mixed::Value(false),
            ligatures: Mixed::Value(Ligature::Default),
            border_style: Mixed::Value(BorderStyle::None),
            border_sides: libertum_model::BorderSides::ALL,
            border_colour: [0, 0, 0, 255],
        }
    }
}

choice_enum!(NumberingVertical {
    Top => s::NUMBERING_TOP,
    Bottom => s::NUMBERING_BOTTOM,
});

choice_enum!(NumberingHorizontal {
    Left => s::POSITION_LEFT,
    Centre => s::POSITION_CENTRE,
    Right => s::POSITION_RIGHT,
});

impl NumberingVertical {
    pub fn model(self) -> libertum_model::PageNumberVertical {
        use libertum_model::PageNumberVertical as Model;
        match self {
            Self::Top => Model::Top,
            Self::Bottom => Model::Bottom,
        }
    }

    pub fn from_model(vertical: libertum_model::PageNumberVertical) -> Self {
        use libertum_model::PageNumberVertical as Model;
        match vertical {
            Model::Top => Self::Top,
            Model::Bottom => Self::Bottom,
        }
    }
}

impl NumberingHorizontal {
    pub fn model(self) -> libertum_model::PageNumberHorizontal {
        use libertum_model::PageNumberHorizontal as Model;
        match self {
            Self::Left => Model::Left,
            Self::Centre => Model::Centre,
            Self::Right => Model::Right,
        }
    }

    pub fn from_model(horizontal: libertum_model::PageNumberHorizontal) -> Self {
        use libertum_model::PageNumberHorizontal as Model;
        match horizontal {
            Model::Left => Self::Left,
            Model::Centre => Self::Centre,
            Model::Right => Self::Right,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentState {
    pub page_size: Mixed<PageSize>,
    pub orientation: Mixed<Orientation>,
    pub margin_top: Mixed<Margin>,
    pub margin_bottom: Mixed<Margin>,
    pub margin_left: Mixed<Margin>,
    pub margin_right: Mixed<Margin>,
    pub header: Mixed<bool>,
    pub footer: Mixed<bool>,
    pub header_distance: Mixed<Margin>,
    pub footer_distance: Mixed<Margin>,
    /// Text of the caret section's headers and footers, by variant (empty when switched off).
    pub header_text: String,
    pub footer_text: String,
    pub first_header_text: String,
    pub first_footer_text: String,
    pub even_header_text: String,
    pub even_footer_text: String,
    /// The section's first page and even pages have their own header and footer.
    pub different_first_page: Mixed<bool>,
    pub different_even_odd: Mixed<bool>,
    pub hyphenation: Mixed<bool>,
    pub ligatures: Mixed<bool>,
    /// The watermark text as typed in the Document pane (applied on Return or Apply).
    pub watermark_text: String,
    pub watermark_diagonal: bool,
    /// The document has a watermark.
    pub watermark_on: bool,
    // Section pane (page numbering, which applies to the whole document).
    /// The number written on the start page.
    pub numbering_start: Option<u16>,
    /// The physical page where numbering starts (1-based).
    pub numbering_start_page: Option<u16>,
    pub numbering_on: Mixed<bool>,
    pub numbering_vertical: Mixed<NumberingVertical>,
    pub numbering_horizontal: Mixed<NumberingHorizontal>,
    pub numbering_mirror: Mixed<bool>,
    pub hide_on_first_page: Mixed<bool>,
    /// Use the previous section's headers and footers (Word "Link to Previous").
    pub link_headers: Mixed<bool>,
    pub link_footers: Mixed<bool>,
    /// The caret paragraph's footnote text and 1-based number (empty/None without one).
    pub footnote_text: String,
    pub footnote_number: Option<u32>,
    pub columns: Mixed<u16>,
}

impl Default for DocumentState {
    fn default() -> Self {
        Self {
            page_size: Mixed::Value(PageSize::A4),
            orientation: Mixed::Value(Orientation::Portrait),
            margin_top: Mixed::Value(Margin(254)),
            margin_bottom: Mixed::Value(Margin(254)),
            margin_left: Mixed::Value(Margin(254)),
            margin_right: Mixed::Value(Margin(254)),
            header: Mixed::Value(false),
            footer: Mixed::Value(false),
            header_distance: Mixed::Value(Margin(127)),
            footer_distance: Mixed::Value(Margin(127)),
            header_text: String::new(),
            footer_text: String::new(),
            first_header_text: String::new(),
            first_footer_text: String::new(),
            even_header_text: String::new(),
            even_footer_text: String::new(),
            different_first_page: Mixed::Value(false),
            different_even_odd: Mixed::Value(false),
            hyphenation: Mixed::Value(false),
            ligatures: Mixed::Value(true),
            watermark_text: String::new(),
            watermark_diagonal: true,
            watermark_on: false,
            numbering_start: None,
            numbering_start_page: None,
            numbering_on: Mixed::Value(false),
            numbering_vertical: Mixed::Value(NumberingVertical::Bottom),
            numbering_horizontal: Mixed::Value(NumberingHorizontal::Centre),
            numbering_mirror: Mixed::Value(false),
            hide_on_first_page: Mixed::Value(false),
            link_headers: Mixed::Value(false),
            link_footers: Mixed::Value(false),
            footnote_text: String::new(),
            footnote_number: None,
            columns: Mixed::Value(1),
        }
    }
}

impl DocumentState {
    pub fn set_margin(&mut self, side: MarginSide, margin: Margin) {
        let slot = match side {
            MarginSide::Top => &mut self.margin_top,
            MarginSide::Bottom => &mut self.margin_bottom,
            MarginSide::Left => &mut self.margin_left,
            MarginSide::Right => &mut self.margin_right,
        };
        *slot = Mixed::Value(margin);
    }

    pub fn margin(&self, side: MarginSide) -> Mixed<Margin> {
        match side {
            MarginSide::Top => self.margin_top,
            MarginSide::Bottom => self.margin_bottom,
            MarginSide::Left => self.margin_left,
            MarginSide::Right => self.margin_right,
        }
    }

    /// Page size in millimetres after applying the orientation.
    pub fn page_millimetres(&self) -> Option<(f32, f32)> {
        let (w, h) = self.page_size.value()?.millimetres();
        Some(match self.orientation.value()? {
            Orientation::Portrait => (w, h),
            Orientation::Landscape => (h, w),
        })
    }
}

/// Zoom levels offered in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Zoom {
    Percent(u16),
    FitWidth,
    FitPage,
}

impl Zoom {
    pub const PRESETS: &'static [u16] = &[50, 75, 100, 125, 150, 200];
}

impl fmt::Display for Zoom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Percent(p) => f.write_str(&s::zoom_percent(*p)),
            Self::FitWidth => f.write_str(s::ZOOM_FIT_WIDTH),
            Self::FitPage => f.write_str(s::ZOOM_FIT_PAGE),
        }
    }
}

/// A numeric inspector field edited through a stepper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    FontSize,
    /// Character spacing, points.
    CharSpacing,
    LineSpacing,
    /// Line spacing in points (the "At least", "Exactly" and "Between" modes).
    LineSpacingPoints,
    SpaceBefore,
    SpaceAfter,
    IndentFirst,
    IndentLeft,
    IndentRight,
    Columns,
    Gutter,
    Margin(MarginSide),
    HeaderDistance,
    FooterDistance,
    NumberingStart,
    NumberingStartPage,
}

impl Field {
    /// Range and step in the displayed unit.
    pub fn spec(self, units: Units) -> StepSpec {
        let per = units.cm_per_unit();
        let length = |min_cm: f32, max_cm: f32| StepSpec {
            min: min_cm / per,
            max: max_cm / per,
            step: if per > 1.0 { 0.05 } else { 0.1 },
            decimals: 2,
        };
        match self {
            Self::FontSize => StepSpec::new(1.0, 400.0, 1.0, 0),
            Self::CharSpacing => StepSpec::new(-10.0, 50.0, 0.5, 1),
            Self::LineSpacing => StepSpec::new(0.5, 10.0, 0.05, 2),
            Self::LineSpacingPoints => StepSpec::new(0.0, 400.0, 1.0, 0),
            Self::SpaceBefore | Self::SpaceAfter => StepSpec::new(0.0, 200.0, 1.0, 0),
            Self::IndentFirst => length(-10.0, 20.0),
            Self::IndentLeft | Self::IndentRight => length(0.0, 20.0),
            Self::Gutter => length(0.0, 5.0),
            Self::Margin(_) => length(0.0, 10.0),
            Self::HeaderDistance | Self::FooterDistance => length(0.0, 5.0),
            Self::Columns => StepSpec::new(1.0, 12.0, 1.0, 0),
            Self::NumberingStart => StepSpec::new(0.0, 9999.0, 1.0, 0),
            Self::NumberingStartPage => StepSpec::new(1.0, 9999.0, 1.0, 0),
        }
    }

    pub fn unit_label(self, units: Units) -> &'static str {
        match self {
            Self::FontSize
            | Self::SpaceBefore
            | Self::SpaceAfter
            | Self::CharSpacing
            | Self::LineSpacingPoints => s::UNIT_PT,
            Self::LineSpacing => "",
            Self::Columns | Self::NumberingStart | Self::NumberingStartPage => "",
            _ => units.label(),
        }
    }

    /// Current value in the displayed unit; `None` when the selection is mixed.
    pub fn get(self, state: &AppState) -> Option<f32> {
        let f = &state.format;
        let d = &state.document;
        let per = state.units.cm_per_unit();
        let hundredths = |m: Mixed<Margin>| m.value().map(|m| f32::from(m.0) / 100.0 / per);
        let cm = |m: Mixed<f32>| m.value().map(|v| v / per);
        match self {
            Self::FontSize => f.font_size.value().map(|p| f32::from(p.0)),
            Self::CharSpacing => f.char_spacing.value().map(|t| f32::from(t) / 10.0),
            Self::LineSpacing => f.line_spacing.value().map(|l| f32::from(l.0) / 100.0),
            Self::LineSpacingPoints => f.line_spacing_pt.value().map(|p| f32::from(p.0)),
            Self::SpaceBefore => f.space_before.value().map(|p| f32::from(p.0)),
            Self::SpaceAfter => f.space_after.value().map(|p| f32::from(p.0)),
            Self::IndentFirst => cm(f.indent_first_cm),
            Self::IndentLeft => cm(f.indent_left_cm),
            Self::IndentRight => cm(f.indent_right_cm),
            Self::Columns => f.columns.value().map(f32::from),
            Self::Gutter => cm(f.gutter_cm),
            Self::Margin(side) => hundredths(d.margin(side)),
            Self::HeaderDistance => hundredths(d.header_distance),
            Self::FooterDistance => hundredths(d.footer_distance),
            Self::NumberingStart => Some(f32::from(d.numbering_start.unwrap_or(1))),
            Self::NumberingStartPage => Some(f32::from(d.numbering_start_page.unwrap_or(1))),
        }
    }

    /// Store a value given in the displayed unit, clamped to the field's range.
    pub fn set(self, state: &mut AppState, value: f32) {
        let value = self.spec(state.units).clamp(value);
        let per = state.units.cm_per_unit();
        let hundredths = |v: f32| Margin((v * per * 100.0).round().max(0.0) as u16);
        let cm = |v: f32| Mixed::Value(((v * per) * 100.0).round() / 100.0);
        let f = &mut state.format;
        let d = &mut state.document;
        match self {
            Self::FontSize => f.font_size = Mixed::Value(Points(value.round() as u16)),
            Self::CharSpacing => f.char_spacing = Mixed::Value((value * 10.0).round() as i16),
            Self::LineSpacingPoints => {
                f.line_spacing_pt = Mixed::Value(Points(value.round() as u16));
            }
            Self::LineSpacing => {
                f.line_spacing = Mixed::Value(LineSpacing((value * 100.0).round() as u16));
            }
            Self::SpaceBefore => f.space_before = Mixed::Value(Points(value.round() as u16)),
            Self::SpaceAfter => f.space_after = Mixed::Value(Points(value.round() as u16)),
            Self::IndentFirst => f.indent_first_cm = cm(value),
            Self::IndentLeft => f.indent_left_cm = cm(value),
            Self::IndentRight => f.indent_right_cm = cm(value),
            Self::Columns => f.columns = Mixed::Value(value.round() as u16),
            Self::Gutter => f.gutter_cm = cm(value),
            Self::Margin(side) => d.set_margin(side, hundredths(value)),
            Self::HeaderDistance => d.header_distance = Mixed::Value(hundredths(value)),
            Self::FooterDistance => d.footer_distance = Mixed::Value(hundredths(value)),
            Self::NumberingStart => d.numbering_start = Some(value.round() as u16),
            Self::NumberingStartPage => {
                d.numbering_start_page = Some(value.round().max(1.0) as u16)
            }
        }
    }

    /// Step by `step_cm` centimetres (indent buttons), clamped to the field's range.
    pub fn nudge_by(self, state: &mut AppState, direction: i8, step_cm: f32) {
        let current = self.get(state).unwrap_or(0.0);
        let delta = f32::from(direction.signum()) * step_cm / state.units.cm_per_unit();
        self.set(state, current + delta);
    }

    /// Step by one increment (`direction` is +1 or -1). A mixed value starts from the minimum.
    pub fn nudge(self, state: &mut AppState, direction: i8) {
        let spec = self.spec(state.units);
        let next = match self.get(state) {
            Some(v) => spec.nudge(v, direction),
            None => spec.min,
        };
        self.set(state, next);
    }
}

choice_enum!(ViewAction {
    Inspector => s::VIEW_HIDE_INSPECTOR,
    WordCount => s::VIEW_WORD_COUNT,
    Booklet => s::VIEW_BOOKLET,
    Pageless => s::VIEW_PAGELESS,
});

choice_enum!(InsertAction {
    PageBreak => s::INSERT_PAGE_BREAK,
    SectionBreak => s::INSERT_SECTION_BREAK,
    Table => s::INSERT_TABLE,
    Image => s::INSERT_IMAGE,
    Footnote => s::INSERT_FOOTNOTE,
    Endnote => s::INSERT_ENDNOTE,
    TableOfContents => s::INSERT_TOC,
    PageNumber => s::INSERT_PAGE_NUMBER,
});

// Commands in the Table toolbar menu.
choice_enum!(TableAction {
    InsertTable => s::TABLE_INSERT,
    InsertRowAbove => s::TABLE_ROW_ABOVE,
    InsertRowBelow => s::TABLE_ROW_BELOW,
    InsertColumnLeft => s::TABLE_COLUMN_LEFT,
    InsertColumnRight => s::TABLE_COLUMN_RIGHT,
    DeleteRow => s::TABLE_DELETE_ROW,
    DeleteColumn => s::TABLE_DELETE_COLUMN,
    DeleteTable => s::TABLE_DELETE,
    MergeRight => s::TABLE_MERGE_RIGHT,
    MergeBelow => s::TABLE_MERGE_BELOW,
    SplitCell => s::TABLE_SPLIT_CELL,
    ColumnNarrower => s::TABLE_COLUMN_NARROWER,
    ColumnWider => s::TABLE_COLUMN_WIDER,
    EqualiseColumns => s::TABLE_COLUMN_EQUALISE,
});

/// The Find & Replace bar: its query, options and the match Replace acts on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FindState {
    pub open: bool,
    pub query: String,
    pub replace: String,
    pub match_case: bool,
    pub whole_word: bool,
    /// The match found last; Replace replaces it.
    pub last: Option<libertum_editor::SearchMatch>,
    /// How many times the query occurs in the document and which occurrence `last` is
    /// (1-based), shown in the bar as "3 of 17".
    pub count: Option<(Option<usize>, usize)>,
    /// Short feedback in the bar: "No matches", "Replaced 3 places".
    pub notice: Option<String>,
}

/// One inline image of the caret's paragraph, for the Format inspector's Image group.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageEntry {
    pub block: libertum_model::BlockId,
    pub index: usize,
    pub width_pt: f32,
    pub alt: String,
}

/// Duration of the inspector slide animation.
pub const SIDEBAR_ANIMATION_SECS: f32 = 0.15;

/// The state behind the toolbar, inspector and word-count pill.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub inspector: Inspector,
    pub format_pane: FormatPane,
    pub document_pane: DocumentPane,
    /// Fraction of the sidebar width currently shown, 0.0 to 1.0 (animated).
    pub sidebar_open: f32,
    /// When the current slide started and from which fraction; `None` when settled.
    pub sidebar_anim: Option<(Instant, f32)>,
    pub show_word_count: bool,
    pub open_menu: Option<Menu>,
    pub open_swatch: Option<SwatchTarget>,
    /// The Export menu's chapter list is open beside it.
    pub ao3_open: bool,
    /// Chapters of the open document, computed when the list is opened.
    pub ao3_chapters: Vec<libertum_export::Chapter>,
    /// The caret is in a table cell (the Table menu's row/column commands apply).
    pub in_table: bool,
    /// The window fills the screen.
    pub fullscreen: bool,
    /// The page thumbnails column is shown left of the pages.
    pub thumbnails: bool,
    /// What the Table menu's merge and split commands can do at the caret.
    pub can_merge_right: bool,
    pub can_merge_below: bool,
    pub can_split_cell: bool,
    /// Name of the caret table's style, if it has one.
    pub table_style: Option<String>,
    /// Width of the grid column the caret's cell starts at, points.
    pub column_width_pt: Option<f32>,
    /// The open document is a Markdown file: controls Markdown cannot store are hidden.
    pub markdown: bool,
    /// Inline images of the caret's paragraph.
    pub images: Vec<ImageEntry>,
    /// The Find & Replace bar.
    pub find: FindState,
    /// Text being typed into a stepper, shown instead of the formatted value.
    pub editing: Option<(Field, String)>,
    pub units: Units,
    pub zoom: Zoom,
    /// Text being typed into the toolbar's zoom stepper, if any.
    pub zoom_edit: Option<String>,
    /// Booklet view: page 1 alone, then pages 2-3, 4-5... side by side.
    pub booklet: bool,
    /// Pageless view: one endless column instead of pages.
    pub pageless: bool,
    pub format: FormatState,
    /// The font popup's list: the machine's installed families once a document is open.
    pub fonts: Vec<FontName>,
    /// The advanced font options (baseline, character spacing) are shown.
    pub font_advanced: bool,
    pub document: DocumentState,
    // Placeholder document facts for the word-count pill.
    pub page_count: usize,
    pub current_page: usize,
    pub word_count: usize,
    /// Words in the current selection, when there is one.
    pub selection_words: Option<usize>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            sidebar_width: crate::settings::DEFAULT_SIDEBAR_WIDTH,
            inspector: Inspector::Format,
            format_pane: FormatPane::Style,
            document_pane: DocumentPane::Document,
            sidebar_open: 1.0,
            sidebar_anim: None,
            show_word_count: true,
            open_menu: None,
            open_swatch: None,
            ao3_open: false,
            ao3_chapters: Vec::new(),
            in_table: false,
            fullscreen: false,
            thumbnails: false,
            can_merge_right: false,
            can_merge_below: false,
            can_split_cell: false,
            table_style: None,
            column_width_pt: None,
            find: FindState::default(),
            markdown: false,
            images: Vec::new(),
            editing: None,
            units: Units::Centimetres,
            zoom: Zoom::Percent(100),
            zoom_edit: None,
            booklet: false,
            pageless: false,
            format: FormatState::default(),
            fonts: FONT_FAMILIES
                .iter()
                .map(|f| FontName(Arc::from(*f)))
                .collect(),
            font_advanced: false,
            document: DocumentState::default(),
            page_count: 5,
            current_page: 1,
            word_count: 0,
            selection_words: None,
        }
    }
}

impl AppState {
    /// Sidebar toggle button / Ctrl+Alt+I: shows or hides the sidebar, keeping the content.
    pub fn toggle_sidebar(&mut self) {
        self.set_sidebar_visible(!self.sidebar_visible);
    }

    /// Toolbar Format / Document buttons: the active one hides the sidebar, the other one
    /// switches its content (showing the sidebar if it was hidden).
    pub fn toggle_inspector(&mut self, kind: Inspector) {
        if self.sidebar_visible && self.inspector == kind {
            self.set_sidebar_visible(false);
        } else {
            self.inspector = kind;
            self.set_sidebar_visible(true);
        }
    }

    pub fn set_sidebar_visible(&mut self, visible: bool) {
        if self.sidebar_visible != visible {
            self.sidebar_visible = visible;
            self.sidebar_anim = Some((Instant::now(), self.sidebar_open));
        }
    }

    /// Skip the animation (startup, tests).
    pub fn settle_sidebar(&mut self) {
        self.sidebar_anim = None;
        self.sidebar_open = if self.sidebar_visible { 1.0 } else { 0.0 };
    }

    /// Advance the slide animation; returns `true` while it is still running.
    pub fn tick_sidebar(&mut self, now: Instant) -> bool {
        let Some((start, from)) = self.sidebar_anim else {
            return false;
        };
        let target = if self.sidebar_visible { 1.0 } else { 0.0 };
        let t = (now.saturating_duration_since(start).as_secs_f32() / SIDEBAR_ANIMATION_SECS)
            .clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        self.sidebar_open = from + (target - from) * eased;
        if t >= 1.0 {
            self.settle_sidebar();
            false
        } else {
            true
        }
    }

    pub fn sidebar_animating(&self) -> bool {
        self.sidebar_anim.is_some()
    }

    /// Width the sidebar currently occupies, in logical pixels.
    pub fn sidebar_shown_width(&self) -> f32 {
        self.sidebar_width * self.sidebar_open
    }

    /// Type into a stepper: the text is kept as typed and any parseable value applies live.
    pub fn edit_field(&mut self, field: Field, text: String) {
        if let Some(v) = crate::widgets::stepper::parse(&text) {
            field.set(self, v);
        }
        self.editing = Some((field, text));
    }

    pub fn toggle_menu(&mut self, menu: Menu) {
        let closing = self.open_menu == Some(menu);
        self.open_menu = if closing { None } else { Some(menu) };
        if closing || self.open_menu != Some(Menu::Export) {
            self.ao3_open = false;
        }
    }

    pub fn toggle_swatch(&mut self, target: SwatchTarget) {
        self.open_swatch = if self.open_swatch == Some(target) {
            None
        } else {
            Some(target)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_toggle_turns_on() {
        assert_eq!(Mixed::<bool>::Mixed.toggled(), Mixed::Value(true));
        assert_eq!(Mixed::Value(true).toggled(), Mixed::Value(false));
        assert_eq!(Mixed::Value(false).toggled(), Mixed::Value(true));
    }

    #[test]
    fn mixed_has_no_value() {
        assert_eq!(Mixed::<Points>::Mixed.value(), None);
        assert_eq!(Mixed::Value(Points(12)).value(), Some(Points(12)));
    }

    #[test]
    fn active_inspector_button_hides_the_sidebar() {
        let mut state = AppState::default();
        assert!(state.sidebar_visible && state.inspector == Inspector::Format);
        state.toggle_inspector(Inspector::Format);
        assert!(!state.sidebar_visible);
        // The content is remembered while hidden; the same button shows it again.
        state.toggle_inspector(Inspector::Format);
        assert!(state.sidebar_visible && state.inspector == Inspector::Format);
    }

    #[test]
    fn other_inspector_button_switches_content() {
        let mut state = AppState::default();
        state.toggle_inspector(Inspector::Document);
        assert!(state.sidebar_visible);
        assert_eq!(state.inspector, Inspector::Document);
        state.toggle_inspector(Inspector::Format);
        assert!(state.sidebar_visible);
        assert_eq!(state.inspector, Inspector::Format);
    }

    #[test]
    fn hidden_sidebar_opens_on_the_chosen_inspector() {
        let mut state = AppState::default();
        state.toggle_sidebar();
        assert!(!state.sidebar_visible);
        state.toggle_inspector(Inspector::Document);
        assert!(state.sidebar_visible);
        assert_eq!(state.inspector, Inspector::Document);
    }

    #[test]
    fn sidebar_animation_runs_to_the_target() {
        let mut state = AppState::default();
        state.toggle_sidebar();
        assert!(state.sidebar_animating());
        let start = state.sidebar_anim.map(|(t, _)| t).expect("animating");
        assert!(state.tick_sidebar(start + std::time::Duration::from_millis(50)));
        assert!(state.sidebar_open > 0.0 && state.sidebar_open < 1.0);
        assert!(!state.tick_sidebar(start + std::time::Duration::from_millis(400)));
        assert_eq!(state.sidebar_open, 0.0);
        assert!(!state.sidebar_animating());
    }

    #[test]
    fn rapid_toggle_reverses_from_the_current_fraction() {
        let mut state = AppState::default();
        state.toggle_sidebar();
        let start = state.sidebar_anim.map(|(t, _)| t).expect("animating");
        state.tick_sidebar(start + std::time::Duration::from_millis(60));
        let mid = state.sidebar_open;
        state.toggle_sidebar();
        let (_, from) = state.sidebar_anim.expect("animating");
        assert_eq!(from, mid);
        assert!(state.sidebar_visible);
    }

    #[test]
    fn stepper_fields_clamp() {
        let mut state = AppState::default();
        Field::FontSize.set(&mut state, 5000.0);
        assert_eq!(state.format.font_size, Mixed::Value(Points(400)));
        Field::FontSize.set(&mut state, -3.0);
        assert_eq!(state.format.font_size, Mixed::Value(Points(1)));
        Field::Margin(MarginSide::Top).set(&mut state, 99.0);
        assert_eq!(state.document.margin_top, Mixed::Value(Margin(1000)));
    }

    #[test]
    fn nudge_steps_and_stops_at_the_limits() {
        let mut state = AppState::default();
        Field::FontSize.nudge(&mut state, 1);
        assert_eq!(state.format.font_size, Mixed::Value(Points(13)));
        Field::Columns.set(&mut state, 12.0);
        Field::Columns.nudge(&mut state, 1);
        assert_eq!(state.format.columns, Mixed::Value(12));
        Field::LineSpacing.nudge(&mut state, 1);
        assert_eq!(state.format.line_spacing, Mixed::Value(LineSpacing(105)));
    }

    #[test]
    fn mixed_field_starts_from_its_minimum() {
        let mut state = AppState::default();
        state.format.font_size = Mixed::Mixed;
        assert_eq!(Field::FontSize.get(&state), None);
        Field::FontSize.nudge(&mut state, 1);
        assert_eq!(state.format.font_size, Mixed::Value(Points(1)));
    }

    #[test]
    fn lengths_convert_between_centimetres_and_inches() {
        let mut state = AppState {
            units: Units::Inches,
            ..AppState::default()
        };
        let top = Field::Margin(MarginSide::Top);
        assert!((top.get(&state).expect("value") - 1.0).abs() < 0.01);
        top.set(&mut state, 0.5);
        assert_eq!(state.document.margin_top, Mixed::Value(Margin(127)));
        assert_eq!(top.unit_label(state.units), "in");
    }

    #[test]
    fn typing_applies_live_and_keeps_the_text() {
        let mut state = AppState::default();
        state.edit_field(Field::FontSize, "18".into());
        assert_eq!(state.format.font_size, Mixed::Value(Points(18)));
        assert_eq!(state.editing, Some((Field::FontSize, "18".into())));
        // Unparseable text leaves the value alone but stays visible while typing.
        state.edit_field(Field::FontSize, "1x".into());
        assert_eq!(state.format.font_size, Mixed::Value(Points(18)));
        state.edit_field(Field::FontSize, "999".into());
        assert_eq!(state.format.font_size, Mixed::Value(Points(400)));
    }

    #[test]
    fn numbering_starts_at_one_when_enabled() {
        let mut state = AppState::default();
        assert_eq!(state.document.numbering_start, None);
        Field::NumberingStart.nudge(&mut state, 1);
        assert_eq!(state.document.numbering_start, Some(2));
    }

    #[test]
    fn units_follow_the_locale() {
        assert_eq!(Units::from_locale("en_US.UTF-8"), Units::Inches);
        assert_eq!(Units::from_locale("en-US"), Units::Inches);
        assert_eq!(Units::from_locale("en_SG.UTF-8"), Units::Centimetres);
        assert_eq!(Units::from_locale("C"), Units::Centimetres);
        assert_eq!(Units::from_locale(""), Units::Centimetres);
    }

    #[test]
    fn landscape_swaps_the_page_dimensions() {
        let mut d = DocumentState::default();
        assert_eq!(d.page_millimetres(), Some((210.0, 297.0)));
        d.orientation = Mixed::Value(Orientation::Landscape);
        assert_eq!(d.page_millimetres(), Some((297.0, 210.0)));
    }

    #[test]
    fn menus_toggle() {
        let mut state = AppState::default();
        state.toggle_menu(Menu::Zoom);
        assert_eq!(state.open_menu, Some(Menu::Zoom));
        state.toggle_menu(Menu::Insert);
        assert_eq!(state.open_menu, Some(Menu::Insert));
        state.toggle_menu(Menu::Insert);
        assert_eq!(state.open_menu, None);
    }
}
