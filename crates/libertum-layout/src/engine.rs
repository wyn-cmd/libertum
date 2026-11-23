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
    Alignment, AlignmentOptions, FontContext, IndentOptions, InlineBox, InlineBoxKind, Language,
    Layout, LayoutContext, LineBreakContext, LineHeight, PositionedLayoutItem, StyleProperty,
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

/// Where the centre line of a vertical column lies above the baseline, in ems: half the
/// ascent less the descent of an ideographic em box (0.88 and 0.12 in the CJK fonts that matter).
const COLUMN_CENTRE_EM: f32 = 0.38;
/// Ruby text is this fraction of its base's size.
const RUBY_SIZE_RATIO: f32 = 0.5;
/// How far above its baseline a line's text reaches, in ems, which the room made for ruby
/// above it is added to.
const RUBY_BASE_ASCENT_EM: f32 = 0.9;
/// Space between ruby and its base text, as a fraction of the base's size.
const RUBY_GAP_EM: f32 = 0.05;
/// Room kept at the end of each line of a hyphenated paragraph for the hyphen, as a
/// fraction of the font size (a hyphen is about a third of an em in most text faces).
const HYPHEN_RESERVE_EM: f32 = 0.35;

/// Hyphenation points of `text`: the character indices a line may break before, inside
/// words, at their syllables. Latin words use the English patterns and Cyrillic words the
/// Ukrainian ones (see [`crate::script::cyrillic_word_language`]); Chinese,
/// Japanese, Korean, Greek and other scripts are never broken at syllables.
fn hyphenation_points(text: &str, cyrillic: Option<crate::script::Cyrillic>) -> Vec<usize> {
    use crate::script::{Cyrillic, is_cyrillic_letter, is_latin_letter};
    let mut points = Vec::new();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let latin = is_latin_letter(chars[i].1);
        let letter: fn(char) -> bool = if latin {
            is_latin_letter
        } else if is_cyrillic_letter(chars[i].1) {
            is_cyrillic_letter
        } else {
            i += 1;
            continue;
        };
        let start = i;
        while i < chars.len() && letter(chars[i].1) {
            i += 1;
        }
        if i - start < MIN_HYPHENATED_WORD {
            continue;
        }
        let from = chars[start].0;
        let to = chars.get(i).map_or(text.len(), |c| c.0);
        let word = &text[from..to];
        let lang = if latin {
            hypher::Lang::English
        } else {
            match crate::script::cyrillic_word_language(word, cyrillic) {
                Some(Cyrillic::Ukrainian) => hypher::Lang::Ukrainian,
                None => continue,
            }
        };
        let mut at = start;
        let syllables: Vec<&str> = hypher::hyphenate(word, lang).collect();
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
    /// Threads used to shape paragraphs ahead of pagination: 0 picks one per core (at most
    /// [`DEFAULT_LAYOUT_THREADS`]), 1 keeps everything on the calling thread, and an explicit
    /// count is honoured up to [`MAX_LAYOUT_THREADS`].
    pub threads: usize,
}

/// Most threads the engine uses to lay out paragraphs ahead of pagination, however many are asked
/// for.
pub const MAX_LAYOUT_THREADS: usize = 8;

/// Threads used when the count is left to the engine (`threads: 0`). Four: the 500-page demo
/// stops gaining there (full pagination: about 600 ms with one thread, 620 to 650 ms with two
/// or four, 920 to 1,070 ms with eight), and a four-core machine keeps the count it had before
/// the cap (docs/decisions.md, 2026-11-20 and 2026-11-21).
pub const DEFAULT_LAYOUT_THREADS: usize = 4;

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
            threads: 0,
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
    // Hyphenated lines reserve the hyphen's room line by line (see `layout_once`): reserving
    // it for the whole paragraph would leave every other line short of the margin.
    let _ = base;
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

/// Ruby text placed over its base: the reading's own layout, and where it goes in the
/// paragraph's coordinates (its left edge and top, so the app only adds the paragraph's origin).
#[derive(Clone)]
pub struct PlacedRuby {
    /// Index into the snapshot's `ruby`.
    pub index: usize,
    /// Line of the paragraph the base starts on.
    pub line: usize,
    pub x: f32,
    pub y: f32,
    pub layout: Arc<ParagraphLayout>,
}

/// What a ruby needs while the paragraph is laid out: its base range, extra spacing to give
/// the base so it is as wide as the reading, and the height of the room above the line.
#[derive(Clone)]
struct RubySpec {
    range: std::ops::Range<usize>,
    spacing: f32,
    box_height: f32,
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
    /// The paragraph's source text: cluster byte ranges index into this. The PDF exporter
    /// needs it to map shaped glyph ids back to Unicode for copying and searching.
    pub text: String,
    /// Left indent: the layout's x origin relative to the content area's left edge.
    pub x_offset: f32,
    /// Lines broken inside a word by hyphenation: a hyphen is drawn at their end.
    pub hyphen_lines: Vec<usize>,
    /// Inline images and where they landed.
    pub images: Vec<PlacedImage>,
    /// Ruby over its base text.
    pub ruby: Vec<PlacedRuby>,
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
        ruby: &[RubySpec],
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
        let points: HashSet<usize> = if para.hyphenate && !para.upright {
            hyphenation_points(text, para.cyrillic)
                .into_iter()
                .collect()
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
        // Chinese, Japanese and Korean text needs its language for the right regional glyphs
        // (the same Han character is drawn differently in each) and punctuation.
        for (range, locale) in crate::script::cjk_locale_ranges(text, para.locale) {
            if let Ok(language) = Language::parse(locale) {
                b.push(StyleProperty::Locale(Some(language)), range.clone());
            }
            // Vertical text: the font's vertical forms of punctuation, small kana and the like.
            if para.vertical && !para.upright {
                b.push(
                    StyleProperty::FontFeatures(FontFeatures::from("\"vert\" 1")),
                    range,
                );
            }
        }
        // Numbers stand upright in vertical text, one digit to a cell down the column. A digit
        // is taller than it is wide, so each is given room to make its cell about an em.
        if para.vertical && !para.upright {
            for range in crate::script::upright_number_ranges(text) {
                // By the size of the text the number is in: its own span's, else the base.
                let size_pt = para
                    .spans
                    .iter()
                    .rev()
                    .find(|(span, _)| span.start <= range.start && range.start < span.end)
                    .map_or(base, |(_, style)| style.font_size);
                b.push(
                    StyleProperty::LetterSpacing(crate::script::NUMBER_CELL_SPACING_EM * size_pt),
                    range,
                );
            }
        }
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
                // On a vertical page the picture is centred on the column's centre line, which
                // is about 0.38 em above the baseline (half the CJK em box's ascent less its
                // descent), instead of standing on the baseline, which is the column's left.
                baseline: (para.vertical && !para.upright)
                    .then_some(h * 0.5 + COLUMN_CENTRE_EM * base),
                vertical_align: parley::VerticalAlign::BASELINE,
            });
        }
        // Ruby: extra spacing on a base shorter than its reading, and a box above the line at
        // the start of the base that makes the line tall enough to hold the reading. Their ids
        // follow the images'.
        for (i, spec) in ruby.iter().enumerate() {
            if spec.spacing > 0.0 {
                b.push(
                    StyleProperty::LetterSpacing(spec.spacing),
                    spec.range.clone(),
                );
            }
            b.push_inline_box(InlineBox {
                id: (tabs.len() + para.images.len() + i) as u64,
                kind: InlineBoxKind::InFlow,
                index: spec.range.start,
                width: 0.0,
                height: spec.box_height,
                baseline: Some(spec.box_height),
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
        // Hyphenated lines keep room for the drawn hyphen, and only those lines: the line is
        // justified to a width one hyphen narrower, so its text ends where the hyphen starts
        // while every other line still reaches the margin. Breaking is unchanged (both passes
        // use the same width), so the line indices from this pass are the final ones.
        if !points.is_empty() {
            let byte_points: HashSet<usize> = text
                .char_indices()
                .enumerate()
                .filter(|(ci, _)| points.contains(ci))
                .map(|(_, (b, _))| b)
                .collect();
            let n = layout.len();
            let mut hyphenated = vec![false; n];
            for (li, hyphen) in hyphenated.iter_mut().enumerate().take(n.saturating_sub(1)) {
                if let Some(line) = layout.get(li) {
                    *hyphen = byte_points.contains(&line.text_range().end);
                }
            }
            if hyphenated.iter().any(|h| *h) {
                let reserve = base * HYPHEN_RESERVE_EM;
                let mut breaker = layout.break_lines();
                breaker.state_mut().set_layout_max_advance(break_width);
                breaker.state_mut().set_line_max_advance(break_width);
                let mut i = 0;
                while breaker.break_next().is_some() {
                    let line_width = if hyphenated.get(i).copied().unwrap_or(false) {
                        (break_width - reserve).max(1.0)
                    } else {
                        break_width
                    };
                    breaker.set_prior_line_width(line_width);
                    i += 1;
                }
                breaker.finish();
            }
        }
        let alignment = match para.align {
            Align::Start => Alignment::Start,
            Align::Centre => Alignment::Center,
            Align::End => Alignment::End,
            Align::Justify => Alignment::Justify,
        };
        layout.align(alignment, AlignmentOptions::default());
        (layout, points)
    }

    /// Lays the paragraph out, repeating until its tab widths settle (see [`build`](Self::build)).
    fn settle(
        &mut self,
        para: &SpikeParagraph,
        width: f32,
        tabs: &[usize],
        widths: &mut Vec<f32>,
        ruby: &[RubySpec],
    ) -> (Layout<Brush>, HashSet<usize>) {
        let default_style = SpanStyle::new(self.opts.base_size_pt);
        let base = para.base.as_ref().unwrap_or(&default_style).font_size;
        let (left, _right, break_width) = frame(para, base, width);
        let (mut layout, points) = self.layout_once(para, width, tabs, widths, ruby);
        for _ in 0..MAX_TAB_PASSES {
            if tabs.is_empty() {
                break;
            }
            let next = solve_tab_widths(para, &layout, tabs, widths, break_width, left);
            let settled = next
                .iter()
                .zip(widths.iter())
                .all(|(a, b)| (a - b).abs() < TAB_EPSILON_PT);
            if settled {
                break;
            }
            *widths = next;
            let (l, p) = self.layout_once(para, width, tabs, widths, ruby);
            layout = l;
            debug_assert_eq!(p, points);
        }
        (layout, points)
    }

    /// The style of the text at byte `at`: its span's, else the paragraph's.
    fn style_at(para: &SpikeParagraph, at: usize, fallback: &SpanStyle) -> SpanStyle {
        para.spans
            .iter()
            .find(|(range, _)| range.start <= at && at < range.end)
            .map_or_else(
                || para.base.clone().unwrap_or_else(|| fallback.clone()),
                |(_, s)| s.clone(),
            )
    }

    /// The layout of a reading, at its own small size and in its base's colour.
    fn ruby_layout(
        &mut self,
        para: &SpikeParagraph,
        at: usize,
        text: &str,
    ) -> Arc<ParagraphLayout> {
        let fallback = SpanStyle::new(self.opts.base_size_pt);
        let style = Self::style_at(para, at, &fallback);
        let mut reading = SpikeParagraph::new(0, text);
        reading.base = Some(SpanStyle {
            font_size: style.font_size * RUBY_SIZE_RATIO,
            underline: false,
            strikethrough: false,
            highlight: None,
            vertical: VerticalAlign::Baseline,
            letter_spacing: 0.0,
            ..style
        });
        reading.line_spacing = Some(LineSpacing::Multiple(1.0));
        reading.locale = para.locale;
        reading.vertical = para.vertical && !para.upright;
        Arc::new(self.build(&reading, f32::MAX / 4.0))
    }

    /// The valid ruby of a paragraph, with the room each needs above its line.
    fn ruby_specs(&mut self, para: &SpikeParagraph) -> Vec<RubySpec> {
        let mut out = Vec::new();
        let text = para.text.as_str();
        for (range, reading) in &para.ruby {
            let valid = range.start < range.end
                && range.end <= text.len()
                && text.is_char_boundary(range.start)
                && text.is_char_boundary(range.end)
                && !reading.is_empty();
            if !valid {
                continue;
            }
            let fallback = SpanStyle::new(self.opts.base_size_pt);
            let size = Self::style_at(para, range.start, &fallback).font_size;
            let layout = self.ruby_layout(para, range.start, reading);
            let line = layout
                .lines
                .first()
                .map_or(size * RUBY_SIZE_RATIO, |l| l.height);
            out.push(RubySpec {
                range: range.clone(),
                spacing: 0.0,
                box_height: size * (RUBY_BASE_ASCENT_EM + RUBY_GAP_EM) + line,
            });
        }
        out
    }

    /// The reading of `spec`, in the order `ruby_specs` made them, to match against `para.ruby`.
    fn readings<'a>(para: &'a SpikeParagraph, specs: &[RubySpec]) -> Vec<(&'a str, usize)> {
        specs
            .iter()
            .filter_map(|spec| {
                para.ruby
                    .iter()
                    .find(|(range, _)| *range == spec.range)
                    .map(|(range, text)| (text.as_str(), range.start))
            })
            .collect()
    }

    /// For each ruby, the extra letter spacing that makes its base as wide as its reading
    /// (0 when it is), and the height of the room above its line: the font's ascent there,
    /// the gap and the reading.
    fn ruby_adjustments(
        &mut self,
        para: &SpikeParagraph,
        layout: &Layout<Brush>,
        specs: &[RubySpec],
    ) -> Vec<(f32, f32)> {
        let mut out = Vec::with_capacity(specs.len());
        for ((reading, start), spec) in Self::readings(para, specs).into_iter().zip(specs) {
            let reading_layout = self.ruby_layout(para, start, reading);
            let selection = parley::editing::Selection::new(
                Cursor::from_byte_index(layout, spec.range.start, Affinity::Downstream),
                Cursor::from_byte_index(layout, spec.range.end, Affinity::Downstream),
            );
            let fragments = selection.geometry(layout);
            let base_width: f32 = fragments
                .iter()
                .map(|(rect, _)| (rect.x1 - rect.x0) as f32)
                .sum();
            let chars = para.text[spec.range.clone()].chars().count().max(1) as f32;
            let spacing = ((reading_layout.layout.width() - base_width) / chars).max(0.0);
            let fallback = SpanStyle::new(self.opts.base_size_pt);
            let size = Self::style_at(para, start, &fallback).font_size;
            let ascent = fragments
                .first()
                .and_then(|(_, line)| layout.get(*line))
                .map_or(size * RUBY_BASE_ASCENT_EM, Self::glyph_ascent);
            let reading_height = reading_layout.lines.first().map_or(0.0, |l| l.height);
            out.push((spacing, ascent + size * RUBY_GAP_EM + reading_height));
        }
        out
    }

    /// The tallest glyph run ascent on a line.
    fn glyph_ascent(line: parley::layout::Line<'_, Brush>) -> f32 {
        line.items()
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(run) => Some(run.run().font_metrics().ascent),
                _ => None,
            })
            .fold(0.0f32, f32::max)
    }

    /// Where each reading goes: centred over the start of its base, its bottom just above the
    /// tallest glyphs on that line.
    fn place_ruby(
        &mut self,
        para: &SpikeParagraph,
        layout: &Layout<Brush>,
        specs: &[RubySpec],
    ) -> Vec<PlacedRuby> {
        let mut out = Vec::new();
        for (i, ((reading, start), spec)) in Self::readings(para, specs)
            .into_iter()
            .zip(specs)
            .enumerate()
        {
            let reading_layout = self.ruby_layout(para, start, reading);
            let selection = parley::editing::Selection::new(
                Cursor::from_byte_index(layout, spec.range.start, Affinity::Downstream),
                Cursor::from_byte_index(layout, spec.range.end, Affinity::Downstream),
            );
            let Some((rect, line)) = selection.geometry(layout).into_iter().next() else {
                continue;
            };
            let Some(line_ref) = layout.get(line) else {
                continue;
            };
            let metrics = line_ref.metrics();
            let ascent = Self::glyph_ascent(line_ref);
            let fallback = SpanStyle::new(self.opts.base_size_pt);
            let size = Self::style_at(para, start, &fallback).font_size;
            let gap = size * RUBY_GAP_EM;
            let reading_height = reading_layout.lines.first().map_or(0.0, |l| l.height);
            let centre = (rect.x0 + rect.x1) as f32 * 0.5;
            out.push(PlacedRuby {
                index: i,
                line,
                x: centre - reading_layout.layout.width() * 0.5,
                y: metrics.baseline - ascent - gap - reading_height,
                layout: reading_layout,
            });
        }
        out
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
        let (left, _right, _break_width) = frame(para, base, width);
        let mut widths = vec![0.0f32; tabs.len()];
        let mut ruby_specs = self.ruby_specs(para);
        let (mut layout, mut points) = self.settle(para, width, &tabs, &mut widths, &ruby_specs);
        if !ruby_specs.is_empty() {
            // A reading wider than its base spreads the base's letters to match, so
            // neighbouring readings never overlap, and the room above each line is what the
            // font's real ascent leaves. One more pass with those.
            let wanted = self.ruby_adjustments(para, &layout, &ruby_specs);
            let mut changed = false;
            for (spec, (spacing, box_height)) in ruby_specs.iter_mut().zip(wanted) {
                changed |= (spec.spacing - spacing).abs() > 0.01
                    || (spec.box_height - box_height).abs() > 0.05;
                spec.spacing = spacing;
                spec.box_height = box_height;
            }
            if changed {
                (layout, points) = self.settle(para, width, &tabs, &mut widths, &ruby_specs);
            }
        }
        let placed_ruby = self.place_ruby(para, &layout, &ruby_specs);
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
                if let Some(line) = layout.get(li)
                    && byte_points.contains(&line.text_range().end)
                {
                    hyphen_lines.push(li);
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
            text: text.to_owned(),
            hyphen_lines,
            images,
            ruby: placed_ruby,
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
        if let Some(e) = self.full.get_mut(&key)
            && e.layout.version == para.version
        {
            e.tick = self.tick;
            return e.layout.clone();
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

    /// A copy for a worker thread: it shares the font collection and the loaded font data,
    /// has its own shaping state and empty caches.
    ///
    /// The font collection and its data cache are switched to fontique's shared mode first.
    /// Unshared clones each load their own copy of every font they touch (about 55 MB of
    /// mapped font data per worker, measured), so the working set grew with the thread count.
    fn fork(&mut self) -> Engine {
        self.fcx.collection.make_shared();
        self.fcx.source_cache.make_shared();
        Engine {
            fcx: self.fcx.clone(),
            lcx: LayoutContext::new(),
            family: self.family.clone(),
            opts: self.opts.clone(),
            report: self.report.clone(),
            full: HashMap::new(),
            metrics: HashMap::new(),
            labels: HashMap::new(),
            markers: HashMap::new(),
            tick: 0,
            layouts_built: 0,
        }
    }

    /// How many threads a batch of `jobs` paragraphs may use.
    pub(crate) fn worker_count(&self, jobs: usize) -> usize {
        let wanted = if self.opts.threads == 0 {
            std::thread::available_parallelism()
                .map_or(1, |n| n.get())
                .min(DEFAULT_LAYOUT_THREADS)
        } else {
            self.opts.threads
        };
        wanted.min(MAX_LAYOUT_THREADS).min(jobs / 8).max(1)
    }

    /// True when the compact metrics of `para` at `width` are cached for its current version.
    pub(crate) fn has_metrics(&self, para: &SpikeParagraph, width: f32) -> bool {
        self.metrics
            .get(&(para.id, width.to_bits()))
            .is_some_and(|m| m.version == para.version)
    }

    /// Lays out `paras` at `width` on several threads and caches the results, so the
    /// pagination that follows finds them. The layouts equal what [`Engine::full`] builds on
    /// this thread (same fonts, same code); only the order of work differs. Does nothing when
    /// one thread would do the work. Returns how many paragraphs were built.
    pub(crate) fn prewarm(&mut self, paras: &[&SpikeParagraph], width: f32) -> usize {
        let threads = self.worker_count(paras.len());
        if threads < 2 {
            return 0;
        }
        const CLAIM: usize = 8;
        let next = AtomicUsize::new(0);
        let mut forks: Vec<Engine> = (0..threads).map(|_| self.fork()).collect();
        let built: Vec<Vec<(Key, Arc<ParagraphLayout>)>> = std::thread::scope(|scope| {
            let handles: Vec<_> = forks
                .iter_mut()
                .map(|fork| {
                    let next = &next;
                    scope.spawn(move || {
                        let mut out = Vec::new();
                        loop {
                            let from = next.fetch_add(CLAIM, Ordering::Relaxed);
                            if from >= paras.len() {
                                break;
                            }
                            for para in &paras[from..(from + CLAIM).min(paras.len())] {
                                let layout = fork.full(para, width);
                                out.push(((para.id, width.to_bits()), layout));
                            }
                        }
                        out
                    })
                })
                .collect();
            // A worker that panicked simply contributes nothing; pagination builds its
            // paragraphs itself.
            handles
                .into_iter()
                .map(|h| h.join().unwrap_or_default())
                .collect()
        });
        let mut count = 0;
        for (key, layout) in built.into_iter().flatten() {
            self.tick += 1;
            self.metrics.insert(
                key,
                Arc::new(ParaMetrics {
                    heights: layout.lines.iter().map(|l| l.height).collect(),
                    version: layout.version,
                    width_bits: key.1,
                }),
            );
            self.full.insert(
                key,
                FullEntry {
                    layout,
                    tick: self.tick,
                },
            );
            count += 1;
        }
        self.layouts_built += forks.iter().map(|f| f.layouts_built).sum::<u64>();
        self.evict();
        count
    }

    /// Line heights only (what pagination needs). Cached for every paragraph.
    pub(crate) fn metrics(&mut self, para: &SpikeParagraph, width: f32) -> Arc<ParaMetrics> {
        let key = (para.id, width.to_bits());
        if let Some(m) = self.metrics.get(&key)
            && m.version == para.version
        {
            return m.clone();
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

#[cfg(test)]
mod hyphenation_tests {
    use super::hyphenation_points;
    use crate::script::Cyrillic;

    #[test]
    fn cyrillic_words_are_hyphenated_by_their_language() {
        // Ukrainian letters decide without a hint; ambiguous words follow the hint.
        let uk = hyphenation_points("інформаційний", None);
        assert!(!uk.is_empty());
        // Russian has no patterns: its words are left whole whatever the hint.
        assert!(hyphenation_points("объявление", Some(Cyrillic::Ukrainian)).is_empty());
        assert!(hyphenation_points("слово", None).is_empty());
        // Mixed scripts in one text keep their own patterns.
        let mixed = hyphenation_points("extraordinarily інформаційний", None);
        assert!(mixed.iter().any(|p| *p < 13) && mixed.iter().any(|p| *p > 13));
    }
}
