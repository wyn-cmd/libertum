//! Character formatting and the paragraph style sheet (PLAN §4.1, style resolution).
//!
//! Formatting is resolved as **document defaults → paragraph style chain → direct
//! formatting**. Direct formatting on a run is a partial [`CharFormat`]: only the properties
//! set on that text, everything else inherited. Keeping "set here" apart from "inherited from
//! the style" is what lets DOCX `w:rPr` and ODT automatic styles round-trip, and lets a style
//! change reach every paragraph that uses it.

use std::sync::Arc;

use crate::paragraph::Align;

/// CSS / OpenType weight of regular text.
pub const REGULAR: u16 = 400;
/// CSS / OpenType weight of bold text. Weights of 600 and above count as bold (DOCX `w:b`).
pub const BOLD: u16 = 700;

/// Partial character formatting: `None` inherits from the paragraph style.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CharFormat {
    pub family: Option<Arc<str>>,
    pub size_pt: Option<f32>,
    /// 100 to 900.
    pub weight: Option<u16>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
    /// Straight (non-premultiplied) RGBA.
    pub colour: Option<[u8; 4]>,
    /// `Some(None)` explicitly removes an inherited highlight.
    pub highlight: Option<Option<[u8; 4]>>,
    /// Superscript or subscript (DOCX `w:vertAlign`, ODT `style:text-position`).
    pub vertical: Option<VerticalAlign>,
    /// Extra space after each character, points; negative tightens (DOCX `w:spacing`, ODT
    /// `fo:letter-spacing`).
    pub letter_spacing_pt: Option<f32>,
    /// Ligature use (DOCX `w14:ligatures`, ODT `fo:font-feature-settings`).
    pub ligatures: Option<Ligatures>,
}

/// Which typographic ligatures the font may use.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Ligatures {
    /// The font's standard ligatures (fi, fl), as Word and Pages do by default.
    #[default]
    Standard,
    /// No ligatures.
    None,
    /// Standard and discretionary ligatures (ct, st and the like, where the font has them).
    All,
}

/// Superscript and subscript.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum VerticalAlign {
    #[default]
    Baseline,
    Superscript,
    Subscript,
}

impl VerticalAlign {
    /// Size of super- and subscript text relative to the surrounding text (Word and
    /// LibreOffice both use about two thirds).
    pub const SCALE: f32 = 0.65;

    /// Baseline rise as a fraction of the surrounding text's size (negative lowers).
    pub fn rise(self) -> f32 {
        match self {
            VerticalAlign::Baseline => 0.0,
            VerticalAlign::Superscript => 0.33,
            VerticalAlign::Subscript => -0.14,
        }
    }
}

impl CharFormat {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `over` wins wherever it sets a property.
    pub fn overlay(&self, over: &CharFormat) -> CharFormat {
        CharFormat {
            family: over.family.clone().or_else(|| self.family.clone()),
            size_pt: over.size_pt.or(self.size_pt),
            weight: over.weight.or(self.weight),
            italic: over.italic.or(self.italic),
            underline: over.underline.or(self.underline),
            strikethrough: over.strikethrough.or(self.strikethrough),
            colour: over.colour.or(self.colour),
            highlight: over.highlight.or(self.highlight),
            vertical: over.vertical.or(self.vertical),
            letter_spacing_pt: over.letter_spacing_pt.or(self.letter_spacing_pt),
            ligatures: over.ligatures.or(self.ligatures),
        }
    }

    pub fn bold() -> Self {
        Self {
            weight: Some(BOLD),
            ..Self::default()
        }
    }

    pub fn sized(size_pt: f32) -> Self {
        Self {
            size_pt: Some(size_pt),
            ..Self::default()
        }
    }
}

/// Fully resolved character formatting: what layout draws.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedChar {
    pub family: Arc<str>,
    pub size_pt: f32,
    pub weight: u16,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub colour: [u8; 4],
    pub highlight: Option<[u8; 4]>,
    pub vertical: VerticalAlign,
    pub letter_spacing_pt: f32,
    pub ligatures: Ligatures,
}

impl ResolvedChar {
    pub const BLACK: [u8; 4] = [0, 0, 0, 255];

    /// Applies the properties `f` sets.
    pub fn apply(&self, f: &CharFormat) -> ResolvedChar {
        ResolvedChar {
            family: f.family.clone().unwrap_or_else(|| self.family.clone()),
            size_pt: f.size_pt.unwrap_or(self.size_pt),
            weight: f.weight.unwrap_or(self.weight),
            italic: f.italic.unwrap_or(self.italic),
            underline: f.underline.unwrap_or(self.underline),
            strikethrough: f.strikethrough.unwrap_or(self.strikethrough),
            colour: f.colour.unwrap_or(self.colour),
            highlight: f.highlight.unwrap_or(self.highlight),
            vertical: f.vertical.unwrap_or(self.vertical),
            letter_spacing_pt: f.letter_spacing_pt.unwrap_or(self.letter_spacing_pt),
            ligatures: f.ligatures.unwrap_or(self.ligatures),
        }
    }

    pub fn is_bold(&self) -> bool {
        self.weight >= 600
    }
}

/// Line spacing of a paragraph (DOCX `w:spacing w:line/w:lineRule`, ODT `fo:line-height`,
/// `style:line-height-at-least`, `style:line-spacing`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LineSpacing {
    /// A multiple of the font's own line height (1.0 is single spacing).
    Multiple(f32),
    /// Lines at least this tall, points.
    AtLeast(f32),
    /// Lines exactly this tall, points.
    Exactly(f32),
    /// Single spacing plus this much extra space between lines, points.
    Leading(f32),
}

impl Default for LineSpacing {
    fn default() -> Self {
        LineSpacing::Multiple(1.0)
    }
}

/// Which sides of a paragraph's border box are drawn.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(default)]
pub struct BorderSides {
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
    pub left: bool,
}

impl BorderSides {
    pub const NONE: Self = Self {
        top: false,
        right: false,
        bottom: false,
        left: false,
    };
    pub const ALL: Self = Self {
        top: true,
        right: true,
        bottom: true,
        left: true,
    };

    pub fn any(self) -> bool {
        self.top || self.right || self.bottom || self.left
    }
}

/// A paragraph's border box (DOCX `w:pBdr`, ODT `fo:border-*`): one line per chosen side,
/// drawn outside the text with [`space_pt`](Self::space_pt) between text and line.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Borders {
    pub sides: BorderSides,
    /// Line thickness, points.
    pub width_pt: f32,
    /// Straight RGBA line colour.
    pub colour: [u8; 4],
    /// Gap between the text and the line, points (DOCX `w:space`, ODT `fo:padding`).
    pub space_pt: f32,
}

impl Default for Borders {
    fn default() -> Self {
        Self {
            sides: BorderSides::NONE,
            width_pt: 1.0,
            colour: [0, 0, 0, 255],
            space_pt: 1.0,
        }
    }
}

impl Borders {
    /// No line on any side: how direct formatting removes a style's border.
    pub const fn none() -> Self {
        Self {
            sides: BorderSides::NONE,
            width_pt: 0.0,
            colour: [0, 0, 0, 255],
            space_pt: 0.0,
        }
    }

    /// A line of `width_pt` on all four sides, black, with Word's 1 pt gap.
    pub fn line(width_pt: f32) -> Self {
        Self {
            sides: BorderSides::ALL,
            width_pt,
            ..Self::default()
        }
    }

    /// True when something would be drawn.
    pub fn is_visible(self) -> bool {
        self.sides.any() && self.width_pt > 0.0
    }
}

/// How a tab stop aligns the text after the tab.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum TabStopKind {
    /// The text starts at the stop (the default stops are all left).
    #[default]
    Left,
    /// The text is centred on the stop.
    Centre,
    /// The text ends at the stop.
    Right,
    /// The first '.' after the tab sits on the stop.
    Decimal,
}

/// One explicit tab stop, measured from the left margin (DOCX `w:tab`, ODT `style:tab-stop`).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TabStop {
    pub position_pt: f32,
    pub kind: TabStopKind,
}

impl TabStop {
    pub const fn left(position_pt: f32) -> Self {
        Self {
            position_pt,
            kind: TabStopKind::Left,
        }
    }
    pub const fn right(position_pt: f32) -> Self {
        Self {
            position_pt,
            kind: TabStopKind::Right,
        }
    }
    pub const fn centre(position_pt: f32) -> Self {
        Self {
            position_pt,
            kind: TabStopKind::Centre,
        }
    }
    pub const fn decimal(position_pt: f32) -> Self {
        Self {
            position_pt,
            kind: TabStopKind::Decimal,
        }
    }
}

/// Upper bound on explicit stops per paragraph. DOCX allows more; an importer can keep the
/// first eight and preserve the rest as unsupported XML (Phase 3).
pub const MAX_TAB_STOPS: usize = 8;

/// A paragraph's explicit tab stops, kept sorted by position. Copyable and small, because
/// [`ParaFormat`] is copied with every paragraph operation.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TabStops {
    stops: [TabStop; MAX_TAB_STOPS],
    len: u8,
}

impl Default for TabStops {
    fn default() -> Self {
        Self::new()
    }
}

impl TabStops {
    pub const fn new() -> Self {
        Self {
            stops: [TabStop {
                position_pt: 0.0,
                kind: TabStopKind::Left,
            }; MAX_TAB_STOPS],
            len: 0,
        }
    }

    /// Adds a stop, keeping the list sorted. Returns false when full or when a stop already
    /// sits at that position.
    pub fn push(&mut self, stop: TabStop) -> bool {
        let len = usize::from(self.len);
        if len >= MAX_TAB_STOPS
            || self.stops[..len]
                .iter()
                .any(|s| s.position_pt == stop.position_pt)
        {
            return false;
        }
        let at = self.stops[..len]
            .iter()
            .position(|s| s.position_pt > stop.position_pt)
            .unwrap_or(len);
        self.stops.copy_within(at..len, at + 1);
        self.stops[at] = stop;
        self.len += 1;
        true
    }

    pub fn as_slice(&self) -> &[TabStop] {
        &self.stops[..usize::from(self.len)]
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TabStop> {
        self.as_slice().iter()
    }
}

/// Partial paragraph formatting: `None` inherits from the paragraph style. Lengths are
/// points. Indents are measured from the margins; a negative first-line indent hangs.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ParaFormat {
    pub align: Option<Align>,
    pub indent_left_pt: Option<f32>,
    pub indent_right_pt: Option<f32>,
    /// Relative to the left indent (DOCX `w:firstLine` / negative `w:hanging`).
    pub indent_first_pt: Option<f32>,
    pub space_before_pt: Option<f32>,
    pub space_after_pt: Option<f32>,
    pub line_spacing: Option<LineSpacing>,
    /// Keep on the same page as the next paragraph (DOCX `w:keepNext`).
    pub keep_with_next: Option<bool>,
    /// Do not split the paragraph across pages (DOCX `w:keepLines`).
    pub keep_together: Option<bool>,
    /// Never leave a single line of the paragraph alone at the top or bottom of a page
    /// (DOCX `w:widowControl`, ODT `fo:widows` / `fo:orphans` of 2).
    pub widow_control: Option<bool>,
    /// Hyphenate words at line ends (document setting DOCX `w:autoHyphenation`, per
    /// paragraph `w:suppressAutoHyphens`; ODT `fo:hyphenate`).
    pub hyphenate: Option<bool>,
    /// Border box (`Some(Borders::none())` clears a style's border).
    pub borders: Option<Borders>,
    /// Explicit tab stops (`Some(TabStops::new())` clears a style's stops).
    pub tab_stops: Option<TabStops>,
}

impl ParaFormat {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `over` wins wherever it sets a property.
    pub fn overlay(&self, over: &ParaFormat) -> ParaFormat {
        ParaFormat {
            align: over.align.or(self.align),
            indent_left_pt: over.indent_left_pt.or(self.indent_left_pt),
            indent_right_pt: over.indent_right_pt.or(self.indent_right_pt),
            indent_first_pt: over.indent_first_pt.or(self.indent_first_pt),
            space_before_pt: over.space_before_pt.or(self.space_before_pt),
            space_after_pt: over.space_after_pt.or(self.space_after_pt),
            line_spacing: over.line_spacing.or(self.line_spacing),
            keep_with_next: over.keep_with_next.or(self.keep_with_next),
            keep_together: over.keep_together.or(self.keep_together),
            widow_control: over.widow_control.or(self.widow_control),
            hyphenate: over.hyphenate.or(self.hyphenate),
            borders: over.borders.or(self.borders),
            tab_stops: over.tab_stops.or(self.tab_stops),
        }
    }
}

/// Fully resolved paragraph formatting: what layout and pagination use.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ResolvedPara {
    pub align: Align,
    pub indent_left_pt: f32,
    pub indent_right_pt: f32,
    pub indent_first_pt: f32,
    pub space_before_pt: f32,
    pub space_after_pt: f32,
    pub line_spacing: LineSpacing,
    pub keep_with_next: bool,
    pub keep_together: bool,
    pub widow_control: bool,
    pub hyphenate: bool,
    /// Border box; `None` draws nothing.
    pub borders: Option<Borders>,
    /// Explicit tab stops; `None` uses the default stops (every half inch).
    pub tab_stops: Option<TabStops>,
}

impl Default for ResolvedPara {
    fn default() -> Self {
        Self {
            align: Align::Start,
            indent_left_pt: 0.0,
            indent_right_pt: 0.0,
            indent_first_pt: 0.0,
            space_before_pt: 0.0,
            space_after_pt: 6.0,
            line_spacing: LineSpacing::Multiple(1.0),
            keep_with_next: false,
            keep_together: false,
            widow_control: true,
            hyphenate: false,
            borders: None,
            tab_stops: None,
        }
    }
}

impl ResolvedPara {
    /// Applies the properties `f` sets.
    pub fn apply(&self, f: &ParaFormat) -> ResolvedPara {
        ResolvedPara {
            align: f.align.unwrap_or(self.align),
            indent_left_pt: f.indent_left_pt.unwrap_or(self.indent_left_pt),
            indent_right_pt: f.indent_right_pt.unwrap_or(self.indent_right_pt),
            indent_first_pt: f.indent_first_pt.unwrap_or(self.indent_first_pt),
            space_before_pt: f.space_before_pt.unwrap_or(self.space_before_pt),
            space_after_pt: f.space_after_pt.unwrap_or(self.space_after_pt),
            line_spacing: f.line_spacing.unwrap_or(self.line_spacing),
            keep_with_next: f.keep_with_next.unwrap_or(self.keep_with_next),
            keep_together: f.keep_together.unwrap_or(self.keep_together),
            widow_control: f.widow_control.unwrap_or(self.widow_control),
            hyphenate: f.hyphenate.unwrap_or(self.hyphenate),
            // A box whose line is invisible is "no border" for layout; the paragraph keeps
            // `Some(Borders::none())` in its direct formatting for DOCX `w:val="none"`.
            borders: f.borders.or(self.borders).filter(|b| b.is_visible()),
            tab_stops: f.tab_stops.or(self.tab_stops).filter(|s| !s.is_empty()),
        }
    }
}

/// Index of a paragraph style in the [`StyleSheet`].
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct StyleId(pub u32);

impl StyleId {
    pub const BODY: StyleId = StyleId(0);
    pub const TITLE: StyleId = StyleId(1);
    pub const HEADING_1: StyleId = StyleId(2);
    pub const HEADING_2: StyleId = StyleId(3);
    pub const HEADING_3: StyleId = StyleId(4);
    pub const QUOTE: StyleId = StyleId(5);
}

/// A named paragraph style. Its character formatting applies to every paragraph using it,
/// under any direct formatting on the text.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParagraphStyle {
    /// The style's name in files (DOCX `w:name`, ODT `style:display-name`).
    pub name: String,
    /// Inherits from this style (DOCX `w:basedOn`, ODT `style:parent-style-name`).
    pub base: Option<StyleId>,
    pub char: CharFormat,
    pub para: ParaFormat,
    /// Style of the paragraph Return starts after this one (DOCX `w:next`, ODT
    /// `style:next-style-name`); `None` keeps this style.
    pub next: Option<StyleId>,
    /// Heading level for the table of contents (1 = chapter; DOCX `w:outlineLvl`, ODT
    /// `style:default-outline-level`).
    pub outline_level: Option<u8>,
}

/// Document defaults plus the paragraph styles.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct StyleSheet {
    /// Document default character formatting (DOCX `w:docDefaults`).
    pub defaults: ResolvedChar,
    /// Document default paragraph formatting (DOCX `w:pPrDefault`).
    pub para_defaults: ResolvedPara,
    pub paragraph: Vec<ParagraphStyle>,
}

impl Default for StyleSheet {
    fn default() -> Self {
        Self::with_defaults("Cambria", 11.0)
    }
}

/// Deepest `base` chain followed; deeper (or cyclic) chains stop there instead of looping.
const MAX_STYLE_DEPTH: usize = 16;

impl StyleSheet {
    /// The built-in styles (Body, Title, Heading 1 to 3, Quote) over the given default font.
    pub fn with_defaults(family: &str, size_pt: f32) -> Self {
        let style = |name: &str, base: Option<StyleId>, char: CharFormat| ParagraphStyle {
            name: name.to_owned(),
            base,
            char,
            para: ParaFormat::default(),
            next: None,
            outline_level: None,
        };
        let body = Some(StyleId::BODY);
        let bold_sized = |size| CharFormat {
            size_pt: Some(size),
            weight: Some(BOLD),
            ..CharFormat::default()
        };
        // Headings: space above, kept with the text they introduce, followed by body text.
        let heading = |name: &str, size: f32, level: u8, before: f32| ParagraphStyle {
            para: ParaFormat {
                space_before_pt: Some(before),
                keep_with_next: Some(true),
                keep_together: Some(true),
                ..ParaFormat::default()
            },
            next: body,
            outline_level: Some(level),
            ..style(name, body, bold_sized(size))
        };
        Self {
            defaults: ResolvedChar {
                family: Arc::from(family),
                size_pt,
                weight: REGULAR,
                italic: false,
                underline: false,
                strikethrough: false,
                colour: ResolvedChar::BLACK,
                highlight: None,
                vertical: VerticalAlign::Baseline,
                letter_spacing_pt: 0.0,
                ligatures: Ligatures::Standard,
            },
            para_defaults: ResolvedPara::default(),
            paragraph: vec![
                style("Body", None, CharFormat::default()),
                ParagraphStyle {
                    para: ParaFormat {
                        align: Some(Align::Centre),
                        space_after_pt: Some(size_pt),
                        keep_with_next: Some(true),
                        ..ParaFormat::default()
                    },
                    next: body,
                    ..style("Title", body, bold_sized(size_pt * 2.4))
                },
                heading("Heading 1", size_pt * 1.6, 1, size_pt * 1.5),
                heading("Heading 2", size_pt * 1.3, 2, size_pt),
                heading("Heading 3", size_pt * 1.1, 3, size_pt * 0.75),
                ParagraphStyle {
                    para: ParaFormat {
                        indent_left_pt: Some(36.0),
                        indent_right_pt: Some(36.0),
                        ..ParaFormat::default()
                    },
                    ..style(
                        "Quote",
                        body,
                        CharFormat {
                            italic: Some(true),
                            ..CharFormat::default()
                        },
                    )
                },
            ],
        }
    }

    pub fn get(&self, id: StyleId) -> Option<&ParagraphStyle> {
        self.paragraph.get(id.0 as usize)
    }

    /// The styles from `id` up its `base` chain (nearest first), bounded against cycles.
    fn chain(&self, id: StyleId) -> Vec<&ParagraphStyle> {
        let mut chain = Vec::new();
        let mut at = Some(id);
        while let Some(s) = at.and_then(|i| self.get(i)) {
            if chain.len() >= MAX_STYLE_DEPTH {
                break;
            }
            chain.push(s);
            at = s.base;
        }
        chain
    }

    /// The paragraph formatting of style `id`: the defaults, then each style from the root
    /// of its `base` chain down to `id`.
    pub fn resolve_para(&self, id: StyleId) -> ResolvedPara {
        self.chain(id)
            .iter()
            .rev()
            .fold(self.para_defaults, |acc, s| acc.apply(&s.para))
    }

    /// A paragraph's resolved formatting: its style's, then its direct formatting.
    /// List items take their indents from the list unless the paragraph sets its own: every
    /// line of text at the level's indent, the marker in the gap to its left (as Word lays
    /// out a number, a tab, then the text over a hanging indent).
    pub fn resolve_props(&self, props: &crate::ParaProps) -> ResolvedPara {
        let mut r = self.resolve_para(props.style);
        if let Some(item) = &props.list {
            r.indent_left_pt = item.indent_left_pt();
            r.indent_first_pt = 0.0;
        }
        r.apply(&props.format)
    }

    /// Style for the paragraph Return starts after a paragraph of style `id`.
    pub fn next_style(&self, id: StyleId) -> StyleId {
        self.get(id).and_then(|s| s.next).unwrap_or(id)
    }

    /// Heading level of style `id` (inherited through `base`).
    pub fn outline_level(&self, id: StyleId) -> Option<u8> {
        self.chain(id).iter().find_map(|s| s.outline_level)
    }

    /// True if style `id` is `ancestor` or inherits from it.
    pub fn inherits_from(&self, id: StyleId, ancestor: StyleId) -> bool {
        let mut at = Some(id);
        for _ in 0..MAX_STYLE_DEPTH {
            match at {
                Some(i) if i == ancestor => return true,
                Some(i) => at = self.get(i).and_then(|s| s.base),
                None => return false,
            }
        }
        false
    }

    /// The character formatting of paragraph style `id`: the defaults, then each style from
    /// the root of its `base` chain down to `id`. Unknown ids resolve to the defaults.
    pub fn resolve_char(&self, id: StyleId) -> ResolvedChar {
        self.chain(id)
            .iter()
            .rev()
            .fold(self.defaults.clone(), |acc, s| acc.apply(&s.char))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_and_apply_take_the_set_properties() {
        let base = CharFormat {
            weight: Some(BOLD),
            size_pt: Some(18.0),
            ..CharFormat::default()
        };
        let direct = CharFormat {
            weight: Some(REGULAR),
            italic: Some(true),
            ..CharFormat::default()
        };
        let merged = base.overlay(&direct);
        assert_eq!(merged.weight, Some(REGULAR));
        assert_eq!(merged.size_pt, Some(18.0));
        assert_eq!(merged.italic, Some(true));
        let r = StyleSheet::default().defaults.apply(&merged);
        assert!(!r.is_bold() && r.italic && r.size_pt == 18.0);
    }

    #[test]
    fn styles_resolve_through_their_base_chain() {
        let sheet = StyleSheet::with_defaults("Georgia", 12.0);
        let body = sheet.resolve_char(StyleId::BODY);
        assert_eq!(&*body.family, "Georgia");
        assert_eq!(body.size_pt, 12.0);
        let h1 = sheet.resolve_char(StyleId::HEADING_1);
        assert!(h1.is_bold());
        assert!(h1.size_pt > body.size_pt);
        assert_eq!(&*h1.family, "Georgia", "inherited from the defaults");
        assert!(sheet.resolve_char(StyleId::QUOTE).italic);
        assert_eq!(
            sheet.resolve_char(StyleId(99)),
            sheet.defaults,
            "unknown id"
        );
    }

    #[test]
    fn paragraph_formatting_resolves_through_styles() {
        let sheet = StyleSheet::with_defaults("Georgia", 12.0);
        let body = sheet.resolve_para(StyleId::BODY);
        assert_eq!(body.align, Align::Start);
        assert!(body.widow_control && !body.keep_with_next);
        let h1 = sheet.resolve_para(StyleId::HEADING_1);
        assert!(h1.keep_with_next && h1.space_before_pt > 0.0);
        assert_eq!(
            h1.space_after_pt, body.space_after_pt,
            "inherited from Body"
        );
        assert_eq!(sheet.next_style(StyleId::HEADING_1), StyleId::BODY);
        assert_eq!(sheet.next_style(StyleId::BODY), StyleId::BODY);
        assert_eq!(sheet.outline_level(StyleId::HEADING_2), Some(2));
        assert_eq!(sheet.outline_level(StyleId::QUOTE), None);
        assert!(sheet.inherits_from(StyleId::QUOTE, StyleId::BODY));
        assert!(!sheet.inherits_from(StyleId::BODY, StyleId::QUOTE));
        let direct = ParaFormat {
            align: Some(Align::Justify),
            ..ParaFormat::default()
        };
        let r = h1.apply(&direct);
        assert_eq!(r.align, Align::Justify);
        assert!(r.keep_with_next);
    }

    #[test]
    fn a_cyclic_base_chain_terminates() {
        let mut sheet = StyleSheet::default();
        sheet.paragraph[0].base = Some(StyleId(1));
        sheet.paragraph[1].base = Some(StyleId(0));
        let _ = sheet.resolve_char(StyleId(0));
        let _ = sheet.resolve_para(StyleId(0));
        let _ = sheet.outline_level(StyleId(0));
        assert!(!sheet.inherits_from(StyleId(0), StyleId(7)));
    }

    #[test]
    fn tab_stops_resolve_from_the_style_and_clear_with_direct_formatting() {
        let mut stops = TabStops::new();
        assert!(stops.push(TabStop::right(144.0)));
        assert!(stops.push(TabStop::left(36.0)));
        assert_eq!(
            stops
                .as_slice()
                .iter()
                .map(|s| s.position_pt)
                .collect::<Vec<_>>(),
            vec![36.0, 144.0],
            "stops stay sorted"
        );
        assert!(
            !stops.push(TabStop::left(36.0)),
            "duplicate position refused"
        );

        let mut sheet = StyleSheet::with_defaults("Georgia", 12.0);
        sheet.paragraph[StyleId::BODY.0 as usize].para.tab_stops = Some(stops);
        let resolved = sheet.resolve_props(&crate::ParaProps::default());
        assert_eq!(resolved.tab_stops.map(|s| s.len()), Some(2));

        let cleared = crate::ParaProps {
            format: ParaFormat {
                tab_stops: Some(TabStops::new()),
                ..ParaFormat::default()
            },
            ..crate::ParaProps::default()
        };
        assert!(
            sheet.resolve_props(&cleared).tab_stops.is_none(),
            "an empty direct set clears the style's stops"
        );
    }

    #[test]
    fn borders_resolve_from_the_style_and_clear_with_direct_formatting() {
        let mut sheet = StyleSheet::with_defaults("Georgia", 12.0);
        sheet.paragraph[StyleId::BODY.0 as usize].para.borders = Some(Borders::line(1.0));
        let props = crate::ParaProps::default();
        let resolved = sheet.resolve_props(&props);
        assert_eq!(resolved.borders.map(|b| b.width_pt), Some(1.0));
        assert_eq!(
            resolved.borders.map(|b| b.sides),
            Some(BorderSides::ALL),
            "the style's box is inherited"
        );

        let cleared = crate::ParaProps {
            format: ParaFormat {
                borders: Some(Borders::none()),
                ..ParaFormat::default()
            },
            ..props
        };
        let resolved = sheet.resolve_props(&cleared);
        assert!(
            resolved.borders.is_none(),
            "explicit None clears the style's border rather than inheriting it"
        );

        let thick = crate::ParaProps {
            format: ParaFormat {
                borders: Some(Borders::line(2.0)),
                ..ParaFormat::default()
            },
            ..props
        };
        assert_eq!(
            sheet.resolve_props(&thick).borders.map(|b| b.width_pt),
            Some(2.0)
        );
    }
}
