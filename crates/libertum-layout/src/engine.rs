//! Font discovery, parley paragraph layout and the layout caches.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parley::editing::Cursor;
use parley::layout::Affinity;
use parley::style::{
    FontFamily, FontFamilyName, FontFeatures, FontStyle, FontWeight, GenericFamily, OverflowWrap,
};
use parley::{
    Alignment, AlignmentOptions, FontContext, IndentOptions, InlineBox, InlineBoxKind, Layout,
    LayoutContext, LineBreakContext, LineHeight, PositionedLayoutItem, StyleProperty,
};

use libertum_model::{
    Align, Ligatures, LineSpacing, MAX_TAB_STOPS, TabStopKind, TabStops, VerticalAlign,
};

use crate::spike::{SpanStyle, SpikeParagraph};

/// Brush carried through parley for each span: its straight RGBA colour, its highlight if
/// any (the renderer paints that behind the glyphs), and how far superscript or subscript
/// glyphs are raised or lowered from the line's baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Brush {
    pub color: [u8; 4],
    pub highlight: Option<[u8; 4]>,
    /// Baseline rise in 1/64 pt (positive raises; see [`Brush::rise_pt`]).
    pub rise_64: i32,
}

impl Brush {
    /// Baseline rise in points: subtract it from a glyph's y to draw it.
    pub fn rise_pt(&self) -> f32 {
        self.rise_64 as f32 / 64.0
    }
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            color: [0, 0, 0, 255],
            highlight: None,
            rise_64: 0,
        }
    }
}

/// Approximate single line height of a font as a multiple of its size. Used where a line
/// spacing rule needs a height before shaping ("at least", "between"); a fixed estimate,
/// since real fonts range from about 1.15 to 1.3.
const SINGLE_LINE_EM: f32 = 1.2;

/// Shortest word that is hyphenated, in characters (Word's default zone is similar).
const MIN_HYPHENATED_WORD: usize = 5;
/// Room kept at the end of each line of a hyphenated paragraph for the hyphen, as a
/// fraction of the font size (a hyphen is about a third of an em in most text faces).
const HYPHEN_RESERVE_EM: f32 = 0.35;

/// Hyphenation points of `text`: the character indices a line may break before, inside
/// words, at their syllables (English patterns; the model has no language yet).
fn hyphenation_points(text: &str) -> Vec<usize> {
    let mut points = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && chars[i].1.is_alphabetic() {
            i += 1;
        }
        if i - start < MIN_HYPHENATED_WORD {
            continue;
        }
        let from = chars[start].0;
        let to = chars.get(i).map_or(text.len(), |c| c.0);
        let mut at = start;
        let syllables: Vec<&str> =
            hypher::hyphenate(&text[from..to], hypher::Lang::English).collect();
        for s in syllables.iter().take(syllables.len().saturating_sub(1)) {
            at += s.chars().count();
            points.push(at);
        }
    }
    points
}

/// OpenType feature settings for a ligature choice (`None` for the font's defaults).
fn ligature_features(l: Ligatures) -> Option<FontFeatures<'static>> {
    match l {
        Ligatures::Standard => None,
        Ligatures::None => Some(FontFeatures::from("\"liga\" 0, \"clig\" 0")),
        Ligatures::All => Some(FontFeatures::from("\"dlig\" 1")),
    }
}

/// The size and brush of a span, applying superscript or subscript.
fn sized_brush(style: &SpanStyle) -> (f32, Brush) {
    let (size, rise) = match style.vertical {
        VerticalAlign::Baseline => (style.font_size, 0.0),
        v => (
            style.font_size * VerticalAlign::SCALE,
            style.font_size * v.rise(),
        ),
    };
    let brush = Brush {
        color: style.color,
        highlight: style.highlight,
        rise_64: (rise * 64.0).round() as i32,
    };
    (size, brush)
}

/// Tunables for the layout engine.
#[derive(Clone, Debug)]
pub struct LayoutOptions {
    /// Preferred family names, tried in order; the first one installed on this machine is
    /// used, followed by the generic serif family and then the generic sans-serif family.
    pub font_preference: Vec<String>,
    /// Size of text not covered by any span, in points.
    pub base_size_pt: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Maximum number of full parley layouts kept (glyph data is the heavy part). Compact
    /// per-paragraph line metrics for *every* paragraph are kept regardless.
    pub max_cached_layouts: usize,
    /// Tallest an inline image is drawn, points; taller ones are scaled down in proportion
    /// so a line always fits on a page. The pagination state sets this from the page size.
    pub max_image_height_pt: f32,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            font_preference: [
                "Cambria",
                "Georgia",
                "Times New Roman",
                "Liberation Serif",
                "DejaVu Serif",
                "Noto Serif",
                "Times",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
            base_size_pt: 11.0,
            line_height: 1.2,
            max_cached_layouts: 4_000,
            max_image_height_pt: 600.0,
        }
    }
}

/// A tab's own glyph is made this small so it adds no visible advance; the inline box supplies
/// the tab's width. The tab character stays in the text, so byte offsets need no mapping.
const TAB_GLYPH_PT: f32 = 0.01;
/// Default tab stops every half inch, as Word and Pages.
const DEFAULT_TAB_PT: f32 = 36.0;
/// Passes allowed to settle the tab box widths.
const MAX_TAB_PASSES: usize = 6;
/// A width change below this, points, counts as settled.
const TAB_EPSILON_PT: f32 = 0.01;

/// The paragraph's text frame: (left indent, right indent, break width). Hyphenated lines
/// reserve room for the hyphen and a TOC entry for its page number, in both the layout pass
/// and the tab solver.
fn frame(para: &SpikeParagraph, base: f32, width: f32) -> (f32, f32, f32) {
    let min_width = width * 0.25;
    let left = para.indent_left_pt.clamp(0.0, width - min_width);
    let right = para
        .indent_right_pt
        .clamp(0.0, (width - left - min_width).max(0.0));
    let mut break_width = width - left - right;
    if para.hyphenate {
        break_width -= base * HYPHEN_RESERVE_EM;
    }
    if para.toc_target.is_some() {
        break_width = (break_width - TOC_NUMBER_RESERVE_PT).max(break_width * 0.5);
    }
    (left, right, break_width)
}

fn tab_positions(text: &str) -> Vec<usize> {
    text.match_indices('\t').map(|(at, _)| at).collect()
}

/// x coordinate of byte offset `at` in a laid-out paragraph.
fn cursor_x(layout: &Layout<Brush>, at: usize) -> f32 {
    let cursor = Cursor::from_byte_index(layout, at, Affinity::Downstream);
    cursor.geometry(layout, f32::MAX / 8.0).x0 as f32
}

fn line_of(layout: &Layout<Brush>, at: usize) -> Option<usize> {
    layout
        .lines()
        .enumerate()
        .find(|(_, line)| {
            let range = line.text_range();
            at >= range.start && at <= range.end
        })
        .map(|(i, _)| i)
}

/// (line, x, width) of every inline box, by box id.
fn positioned_boxes(layout: &Layout<Brush>) -> Vec<Option<(usize, f32, f32)>> {
    let mut out: Vec<Option<(usize, f32, f32)>> = Vec::new();
    for (line_index, line) in layout.lines().enumerate() {
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(b) = item {
                let id = b.id as usize;
                if out.len() <= id {
                    out.resize(id + 1, None);
                }
                out[id] = Some((line_index, b.x, b.width));
            }
        }
    }
    out
}

/// The first tab stop after `after` (measured from the left margin), within `limit`. Explicit
/// stops replace the default stops up to the last one, as Word's custom stops do; beyond that,
/// default stops every half inch apply again.
fn next_stop(para: &SpikeParagraph, after: f32, limit: f32) -> Option<(f32, TabStopKind)> {
    let stops = para.tab_stops.as_ref().map_or(&[][..], TabStops::as_slice);
    if let Some(stop) = stops
        .iter()
        .filter(|s| s.position_pt > after + 1e-3)
        .min_by(|a, b| a.position_pt.total_cmp(&b.position_pt))
    {
        return (stop.position_pt <= limit).then_some((stop.position_pt, stop.kind));
    }
    let floor = stops.last().map_or(after, |s| after.max(s.position_pt));
    let default = ((floor / DEFAULT_TAB_PT).floor() + 1.0) * DEFAULT_TAB_PT;
    (default <= limit).then_some((default, TabStopKind::Left))
}

/// Widths for every tab, from where its box currently sits. Left tabs advance to the next
/// stop; right, centre and decimal tabs align the text after the tab to it, which needs the
/// width of that text (same line only). A stop past the right margin is ignored, which keeps
/// the pass stable; Word would move the tab's text to the next line instead.
fn solve_tab_widths(
    para: &SpikeParagraph,
    layout: &Layout<Brush>,
    tabs: &[usize],
    previous: &[f32],
    limit: f32,
    left: f32,
) -> Vec<f32> {
    let placed = positioned_boxes(layout);
    let mut out = previous.to_vec();
    for (i, at) in tabs.iter().enumerate() {
        let Some(Some((line, x, width))) = placed.get(i).copied() else {
            continue;
        };
        // Stops are measured from the left margin; x is relative to the paragraph's left
        // indent, and a first-line indent is already part of x.
        let x_rel = left + x;
        let end = tabs.get(i + 1).copied().unwrap_or(para.text.len());
        let same_line = line_of(layout, end) == Some(line);
        let suffix = if same_line {
            (cursor_x(layout, end) - x - width).max(0.0)
        } else {
            0.0
        };
        let dot = para
            .text
            .get(*at + 1..end)
            .and_then(|s| s.find('.'))
            .map(|offset| at + 1 + offset);
        let prefix = if same_line {
            dot.map_or(suffix, |dot| (cursor_x(layout, dot) - x - width).max(0.0))
        } else {
            0.0
        };
        let mut after = x_rel;
        let mut chosen = None;
        for _ in 0..MAX_TAB_STOPS {
            let Some((position, kind)) = next_stop(para, after + 1e-3, limit) else {
                break;
            };
            let anchor = match kind {
                TabStopKind::Left => 0.0,
                TabStopKind::Centre => suffix / 2.0,
                TabStopKind::Right => suffix,
                TabStopKind::Decimal => prefix,
            };
            let width = position - x_rel - anchor;
            if width >= -TAB_EPSILON_PT {
                chosen = Some(width.max(0.0));
                break;
            }
            // The text does not fit before this stop: try the next one.
            after = position;
        }
        out[i] = chosen.unwrap_or(0.0);
    }
    out
}

/// An inline image placed by layout, relative to the paragraph layout's origin.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedImage {
    /// Index into the snapshot's `images`.
    pub index: usize,
    /// Line the image is on.
    pub line: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub media: Arc<libertum_model::Media>,
}

/// The size an image is drawn at: its own, scaled down in proportion to fit the line width
/// and the height cap.
fn fitted_image_size(width: f32, height: f32, max_width: f32, max_height: f32) -> (f32, f32) {
    let width = if width.is_finite() {
        width.max(1.0)
    } else {
        1.0
    };
    let height = if height.is_finite() {
        height.max(1.0)
    } else {
        1.0
    };
    let scale = (max_width.max(1.0) / width)
        .min(max_height.max(1.0) / height)
        .min(1.0);
    (width * scale, height * scale)
}

/// What the font lookup found. Missing fonts never panic: layout falls back to the generic
/// families, and if the machine has no fonts at all, text simply produces no glyphs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontReport {
    /// Number of font families discovered on this machine.
    pub family_count: usize,
    /// The preferred family that was found, if any.
    pub resolved_family: Option<String>,
    /// True when no preferred family was found (generic fallback in use).
    pub using_fallback: bool,
}

/// Vertical box of one laid-out line, in points relative to the top of its paragraph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineBox {
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
}

/// A fully laid-out paragraph at one content width. Holds the parley layout (shaped glyph
/// runs) used for rendering and hit-testing.
pub struct ParagraphLayout {
    pub layout: Layout<Brush>,
    /// Left indent: the layout's x origin relative to the content area's left edge.
    pub x_offset: f32,
    /// Lines broken inside a word by hyphenation: a hyphen is drawn at their end.
    pub hyphen_lines: Vec<usize>,
    /// Inline images and where they landed.
    pub images: Vec<PlacedImage>,
    /// One entry per line; never empty (an empty paragraph has one synthetic line).
    pub lines: Vec<LineBox>,
    pub height: f32,
    pub width: f32,
    pub version: u64,
}

/// Compact line-height table for pagination. Cheap enough to keep for every paragraph.
#[derive(Clone, Debug, PartialEq)]
pub struct ParaMetrics {
    pub heights: Vec<f32>,
    pub version: u64,
    pub width_bits: u32,
}

/// Counters for tests and benchmarks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EngineStats {
    /// Number of parley layouts built since creation.
    pub layouts_built: u64,
    /// Full layouts currently cached.
    pub cached_layouts: usize,
    /// Compact metric tables currently cached.
    pub cached_metrics: usize,
}

struct FullEntry {
    layout: Arc<ParagraphLayout>,
    tick: u64,
}

type Key = (u64, u32); // (paragraph id, content width bits); version is validated on lookup.

pub(crate) struct Engine {
    fcx: FontContext,
    lcx: LayoutContext<Brush>,
    family: Vec<FontFamilyName<'static>>,
    opts: LayoutOptions,
    report: FontReport,
    full: HashMap<Key, FullEntry>,
    metrics: HashMap<Key, Arc<ParaMetrics>>,
    /// One-line layouts of page furniture text (page numbers), keyed by the text.
    labels: HashMap<String, Arc<ParagraphLayout>>,
    /// One-line layouts of list markers, keyed by the text and its formatting.
    markers: HashMap<String, Arc<ParagraphLayout>>,
    tick: u64,
    layouts_built: u64,
}

/// Room kept free at the right of a table-of-contents entry for its page number and leader.
pub const TOC_NUMBER_RESERVE_PT: f32 = 48.0;
/// Bound on cached furniture labels (page numbers repeat, so this is rarely reached).
const MAX_LABELS: usize = 2_048;

impl Engine {
    pub(crate) fn new(opts: LayoutOptions) -> Self {
        let mut fcx = FontContext::new();
        let family_count = fcx.collection.family_names().count();
        let mut resolved = None;
        for name in &opts.font_preference {
            if fcx.collection.family_id(name).is_some() {
                resolved = Some(name.clone());
                break;
            }
        }
        let mut family = Vec::new();
        if let Some(name) = &resolved {
            family.push(FontFamilyName::Named(Cow::Owned(name.clone())));
        }
        family.push(FontFamilyName::Generic(GenericFamily::Serif));
        family.push(FontFamilyName::Generic(GenericFamily::SansSerif));
        let report = FontReport {
            family_count,
            using_fallback: resolved.is_none(),
            resolved_family: resolved,
        };
        Self {
            fcx,
            lcx: LayoutContext::new(),
            family,
            opts,
            report,
            full: HashMap::new(),
            metrics: HashMap::new(),
            labels: HashMap::new(),
            markers: HashMap::new(),
            tick: 0,
            layouts_built: 0,
        }
    }

    /// Sets the tallest an inline image may be drawn (see [`LayoutOptions`]).
    pub(crate) fn set_max_image_height(&mut self, height_pt: f32) {
        if height_pt.is_finite() && height_pt > 0.0 {
            self.opts.max_image_height_pt = height_pt;
        }
    }

    pub(crate) fn report(&self) -> &FontReport {
        &self.report
    }

    /// Names of the font families installed on this machine, sorted case-insensitively
    /// without duplicates. Vertical-writing aliases ("@MS Gothic" on Windows) and hidden
    /// system families (".SF NS" on macOS) are left out.
    pub(crate) fn family_names(&mut self) -> Vec<Arc<str>> {
        let mut names: Vec<Arc<str>> = self
            .fcx
            .collection
            .family_names()
            .filter(|n| !n.is_empty() && !n.starts_with('@') && !n.starts_with('.'))
            .map(Arc::from)
            .collect();
        names.sort_by(|a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        });
        names.dedup();
        names
    }

    pub(crate) fn stats(&self) -> EngineStats {
        EngineStats {
            layouts_built: self.layouts_built,
            cached_layouts: self.full.len(),
            cached_metrics: self.metrics.len(),
        }
    }

    /// One parley pass for a paragraph. Tabs are inline boxes whose widths are solved by
    /// [`Engine::build`]; the tab character itself is drawn at a near-zero size so it adds no
    /// visible advance but keeps the text's byte offsets unchanged.
    fn layout_once(
        &mut self,
        para: &SpikeParagraph,
        width: f32,
        tabs: &[usize],
        tab_widths: &[f32],
    ) -> (Layout<Brush>, HashSet<usize>) {
        self.layouts_built += 1;
        let text = para.text.as_str();
        let lh = self.opts.line_height;
        // Line height of a span of `size` points, by the paragraph's spacing rule.
        let spacing = para.line_spacing;
        let line_height = move |size: f32| match spacing {
            None => LineHeight::FontSizeRelative(lh),
            Some(LineSpacing::Multiple(m)) => LineHeight::MetricsRelative(m.max(0.1)),
            Some(LineSpacing::Exactly(pt)) => LineHeight::Absolute(pt.max(1.0)),
            Some(LineSpacing::AtLeast(pt)) => LineHeight::Absolute(pt.max(size * SINGLE_LINE_EM)),
            Some(LineSpacing::Leading(pt)) => {
                LineHeight::Absolute((size * SINGLE_LINE_EM + pt).max(1.0))
            }
        };
        // The paragraph style's formatting, or the layout defaults.
        let default_style = SpanStyle::new(self.opts.base_size_pt);
        let base_style = para.base.as_ref().unwrap_or(&default_style);
        let base = base_style.font_size;
        let fallback = self.family.clone();
        let family_for = |name: Option<&Arc<str>>| -> FontFamily<'static> {
            let mut names: Vec<FontFamilyName<'static>> = Vec::with_capacity(fallback.len() + 1);
            if let Some(name) = name {
                names.push(FontFamilyName::Named(Cow::Owned(name.to_string())));
            }
            names.extend(fallback.iter().cloned());
            FontFamily::List(Cow::Owned(names))
        };
        let base_family = family_for(base_style.family.as_ref());
        let (base_size, base_brush) = sized_brush(base_style);
        // Hyphenation: extra break opportunities at syllables. parley's override is called
        // once per adjacent character pair, in order (there are no mandatory breaks inside a
        // paragraph), so a counter maps each call to the character after the pair.
        let points: HashSet<usize> = if para.hyphenate {
            hyphenation_points(text).into_iter().collect()
        } else {
            HashSet::new()
        };
        let pair = AtomicUsize::new(0);
        let points_for_override = points.clone();
        let hyphenate_override = move |_: LineBreakContext| -> Option<bool> {
            let after = pair.fetch_add(1, Ordering::Relaxed) + 1;
            points_for_override.contains(&after).then_some(true)
        };
        let mut b = self.lcx.ranged_builder(&mut self.fcx, text, 1.0, false);
        if !points.is_empty() {
            b.set_line_break_override(Some(&hyphenate_override));
        }
        b.push_default(StyleProperty::FontFamily(base_family));
        b.push_default(StyleProperty::FontSize(base_size));
        b.push_default(StyleProperty::LineHeight(line_height(base)));
        if base_style.letter_spacing != 0.0 {
            b.push_default(StyleProperty::LetterSpacing(base_style.letter_spacing));
        }
        if let Some(features) = ligature_features(base_style.ligatures) {
            b.push_default(StyleProperty::FontFeatures(features));
        }
        b.push_default(StyleProperty::FontWeight(FontWeight::new(f32::from(
            base_style.weight,
        ))));
        if base_style.italic {
            b.push_default(StyleProperty::FontStyle(FontStyle::Italic));
        }
        b.push_default(StyleProperty::Underline(base_style.underline));
        b.push_default(StyleProperty::Strikethrough(base_style.strikethrough));
        b.push_default(StyleProperty::Brush(base_brush));
        // Break over-long unbreakable words (URLs, pasted junk) instead of overflowing the margin.
        b.push_default(StyleProperty::OverflowWrap(OverflowWrap::Anywhere));
        for (range, style) in &para.spans {
            let valid = range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end);
            if !valid {
                continue;
            }
            if style.family != base_style.family {
                b.push(
                    StyleProperty::FontFamily(family_for(style.family.as_ref())),
                    range.clone(),
                );
            }
            let (size, brush) = sized_brush(style);
            b.push(StyleProperty::FontSize(size), range.clone());
            // Super- and subscripts keep the line height of the text around them.
            b.push(
                StyleProperty::LineHeight(line_height(style.font_size)),
                range.clone(),
            );
            if style.ligatures != base_style.ligatures {
                let features = ligature_features(style.ligatures).unwrap_or(FontFeatures::empty());
                b.push(StyleProperty::FontFeatures(features), range.clone());
            }
            if style.letter_spacing != base_style.letter_spacing {
                b.push(
                    StyleProperty::LetterSpacing(style.letter_spacing),
                    range.clone(),
                );
            }
            b.push(
                StyleProperty::FontWeight(FontWeight::new(f32::from(style.weight))),
                range.clone(),
            );
            let slant = if style.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            };
            b.push(StyleProperty::FontStyle(slant), range.clone());
            b.push(StyleProperty::Underline(style.underline), range.clone());
            b.push(
                StyleProperty::Strikethrough(style.strikethrough),
                range.clone(),
            );
            b.push(StyleProperty::Brush(brush), range.clone());
        }
        // Tabs: a nearly invisible tab glyph plus an inline box that supplies the advance to
        // the next stop. Pushed last so they override the run's own size and colour.
        let transparent = Brush {
            color: [0, 0, 0, 0],
            ..base_brush
        };
        for (i, at) in tabs.iter().enumerate() {
            let range = *at..(*at + 1);
            b.push(StyleProperty::FontSize(TAB_GLYPH_PT), range.clone());
            b.push(StyleProperty::Brush(transparent), range.clone());
            b.push(StyleProperty::Underline(false), range.clone());
            b.push(StyleProperty::Strikethrough(false), range);
            b.push_inline_box(InlineBox {
                id: i as u64,
                kind: InlineBoxKind::InFlow,
                index: *at,
                width: tab_widths.get(i).copied().unwrap_or(0.0),
                height: base * 1.2,
                baseline: Some(base * 0.8),
                vertical_align: parley::VerticalAlign::BASELINE,
            });
        }
        // Inline images: a box at the image's offset, sitting on the baseline. Their ids
        // follow the tabs', so the tab solver's indexing is unchanged.
        let (_, _, image_width_limit) = frame(para, base, width);
        for (i, image) in para.images.iter().enumerate() {
            let index = image.offset.min(text.len());
            if !text.is_char_boundary(index) {
                continue;
            }
            let (w, h) = fitted_image_size(
                image.width_pt,
                image.height_pt,
                image_width_limit,
                self.opts.max_image_height_pt,
            );
            b.push_inline_box(InlineBox {
                id: (tabs.len() + i) as u64,
                kind: InlineBoxKind::InFlow,
                index,
                width: w,
                height: h,
                baseline: None,
                vertical_align: parley::VerticalAlign::BASELINE,
            });
        }
        let mut layout: Layout<Brush> = b.build(text);
        // Indents narrow the line; at least a quarter of the content width always remains.
        let min_width = width * 0.25;
        let (left, _right, break_width) = frame(para, base, width);
        // The first-line indent may hang into the left indent, but not past the margin.
        let first = para
            .indent_first_pt
            .clamp(-left, break_width - min_width * 0.5);
        if first != 0.0 {
            layout.set_text_indent(first, IndentOptions::default());
        }
        layout.break_all_lines(Some(break_width));
        let alignment = match para.align {
            Align::Start => Alignment::Start,
            Align::Centre => Alignment::Center,
            Align::End => Alignment::End,
            Align::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
        (layout, points)
    }

    /// Lays out a paragraph, solving tab widths first when it contains `\t`. Each pass lays the
    /// paragraph out with the current widths, reads every tab box's x and computes the width
    /// that reaches the next stop; it stops when no width changes by more than 0.01 pt.
    fn build(&mut self, para: &SpikeParagraph, width: f32) -> ParagraphLayout {
        let text = para.text.as_str();
        let lh = self.opts.line_height;
        let spacing = para.line_spacing;
        let empty_line_h = |size: f32| match spacing {
            None => size * lh,
            Some(LineSpacing::Multiple(m)) => size * SINGLE_LINE_EM * m.max(0.1),
            Some(LineSpacing::Exactly(pt)) => pt.max(1.0),
            Some(LineSpacing::AtLeast(pt)) => pt.max(size * SINGLE_LINE_EM),
            Some(LineSpacing::Leading(pt)) => (size * SINGLE_LINE_EM + pt).max(1.0),
        };
        let default_style = SpanStyle::new(self.opts.base_size_pt);
        let base = para.base.as_ref().unwrap_or(&default_style).font_size;
        let tabs = tab_positions(text);
        let (left, _right, break_width) = frame(para, base, width);
        let mut widths = vec![0.0f32; tabs.len()];
        let (mut layout, points) = self.layout_once(para, width, &tabs, &widths);
        for _ in 0..MAX_TAB_PASSES {
            if tabs.is_empty() {
                break;
            }
            let next = solve_tab_widths(para, &layout, &tabs, &widths, break_width, left);
            let settled = next
                .iter()
                .zip(&widths)
                .all(|(a, b)| (a - b).abs() < TAB_EPSILON_PT);
            if settled {
                break;
            }
            widths = next;
            let (l, p) = self.layout_once(para, width, &tabs, &widths);
            layout = l;
            debug_assert_eq!(p, points);
        }
        let mut lines: Vec<LineBox> = layout
            .lines()
            .map(|l| {
                let m = l.metrics();
                LineBox {
                    top: m.block_min_coord,
                    height: m.block_max_coord - m.block_min_coord,
                    baseline: m.baseline,
                }
            })
            .collect();
        if lines.is_empty() {
            // Empty paragraph: one blank line at the base size.
            let h = empty_line_h(base);
            lines.push(LineBox {
                top: 0.0,
                height: h,
                baseline: h * 0.8,
            });
        }
        let height = lines.last().map_or(0.0, |l| l.top + l.height);
        // Lines that end inside a word, at a hyphenation point, get a hyphen drawn after them.
        let mut hyphen_lines = Vec::new();
        if !points.is_empty() {
            let byte_points: HashSet<usize> = text
                .char_indices()
                .enumerate()
                .filter(|(ci, _)| points.contains(ci))
                .map(|(_, (b, _))| b)
                .collect();
            let n = layout.len();
            for li in 0..n.saturating_sub(1) {
                if let Some(line) = layout.get(li) {
                    if byte_points.contains(&line.text_range().end) {
                        hyphen_lines.push(li);
                    }
                }
            }
        }
        // Where each image landed (box ids after the tabs').
        let mut images = Vec::new();
        if !para.images.is_empty() {
            for (line_index, line) in layout.lines().enumerate() {
                for item in line.items() {
                    let PositionedLayoutItem::InlineBox(b) = item else {
                        continue;
                    };
                    let Some(index) = (b.id as usize).checked_sub(tabs.len()) else {
                        continue;
                    };
                    let Some(image) = para.images.get(index) else {
                        continue;
                    };
                    images.push(PlacedImage {
                        index,
                        line: line_index,
                        x: b.x,
                        y: b.y,
                        width: b.width,
                        height: b.height,
                        media: image.media.clone(),
                    });
                }
            }
        }
        ParagraphLayout {
            layout,
            hyphen_lines,
            images,
            x_offset: left,
            lines,
            height,
            width,
            version: para.version,
        }
    }

    /// Full layout for rendering and hit-testing (cached, LRU-ish by use tick).
    pub(crate) fn full(&mut self, para: &SpikeParagraph, width: f32) -> Arc<ParagraphLayout> {
        self.tick += 1;
        let key = (para.id, width.to_bits());
        if let Some(e) = self.full.get_mut(&key) {
            if e.layout.version == para.version {
                e.tick = self.tick;
                return e.layout.clone();
            }
        }
        let built = Arc::new(self.build(para, width));
        // Refresh the compact metrics from the same layout.
        let m = Arc::new(ParaMetrics {
            heights: built.lines.iter().map(|l| l.height).collect(),
            version: para.version,
            width_bits: width.to_bits(),
        });
        self.metrics.insert(key, m);
        self.full.insert(
            key,
            FullEntry {
                layout: built.clone(),
                tick: self.tick,
            },
        );
        self.evict();
        built
    }

    /// Line heights only (what pagination needs). Cached for every paragraph.
    pub(crate) fn metrics(&mut self, para: &SpikeParagraph, width: f32) -> Arc<ParaMetrics> {
        let key = (para.id, width.to_bits());
        if let Some(m) = self.metrics.get(&key) {
            if m.version == para.version {
                return m.clone();
            }
        }
        self.full(para, width);
        self.metrics.get(&key).cloned().unwrap_or_else(|| {
            Arc::new(ParaMetrics {
                heights: vec![self.opts.base_size_pt * self.opts.line_height],
                version: para.version,
                width_bits: width.to_bits(),
            })
        })
    }

    /// A one-line layout of `text` at the base size (page numbers and other page furniture).
    pub(crate) fn label(&mut self, text: &str) -> Arc<ParagraphLayout> {
        if let Some(l) = self.labels.get(text) {
            return l.clone();
        }
        if self.labels.len() >= MAX_LABELS {
            self.labels.clear();
        }
        let snapshot = SpikeParagraph::new(0, text);
        let built = Arc::new(self.build(&snapshot, f32::MAX / 4.0));
        self.labels.insert(text.to_owned(), built.clone());
        built
    }

    /// A one-line layout of a list marker in the formatting of its paragraph's text.
    pub(crate) fn marker(&mut self, text: &str, style: Option<&SpanStyle>) -> Arc<ParagraphLayout> {
        let style = style.cloned().map(|s| SpanStyle {
            underline: false,
            strikethrough: false,
            highlight: None,
            vertical: VerticalAlign::Baseline,
            ..s
        });
        let key = format!("{text}|{style:?}");
        if let Some(l) = self.markers.get(&key) {
            return l.clone();
        }
        if self.markers.len() >= MAX_LABELS {
            self.markers.clear();
        }
        let mut snapshot = SpikeParagraph::new(0, text);
        snapshot.base = style;
        snapshot.line_spacing = Some(LineSpacing::Multiple(1.0));
        let built = Arc::new(self.build(&snapshot, f32::MAX / 4.0));
        self.markers.insert(key, built.clone());
        built
    }

    /// Drops the least recently used quarter of the full layouts once over the cap.
    fn evict(&mut self) {
        let cap = self.opts.max_cached_layouts.max(1);
        if self.full.len() <= cap {
            return;
        }
        let mut ticks: Vec<u64> = self.full.values().map(|e| e.tick).collect();
        let drop_n = (self.full.len() - cap).max(cap / 4).min(self.full.len());
        let (_, nth, _) = ticks.select_nth_unstable(drop_n - 1);
        let cutoff = *nth;
        self.full.retain(|_, e| e.tick > cutoff);
    }
}
