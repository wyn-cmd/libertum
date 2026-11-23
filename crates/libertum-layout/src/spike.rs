//! Layout-facing paragraph snapshots and the deterministic test-document generator.
//!
//! The model's [`libertum_model::Paragraph`] is converted into a [`ParagraphSnapshot`]: only
//! the fields layout needs (id, version, text, spans, spacing) cross the crate boundary, so
//! parley stays isolated behind Libertum's own types (PLAN §10).

use std::ops::Range;
use std::sync::Arc;

use libertum_model::{
    Align, BOLD, BlockId, CharFormat, Document, Ligatures, LineSpacing, ParaFormat, ParaProps,
    Paragraph, REGULAR, ResolvedChar, Run, StyleSheet, VerticalAlign,
};

/// Resolved character formatting for a span: what layout and the renderer draw.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanStyle {
    /// Font family; `None` uses the layout's preferred families.
    pub family: Option<Arc<str>>,
    /// Font size in points.
    pub font_size: f32,
    /// 100 to 900 (400 regular, 700 bold).
    pub weight: u16,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    /// Straight (non-premultiplied) RGBA.
    pub color: [u8; 4],
    /// Background highlight behind the text.
    pub highlight: Option<[u8; 4]>,
    /// Superscript or subscript: drawn smaller and raised or lowered.
    pub vertical: VerticalAlign,
    /// Extra space after each character, points.
    pub letter_spacing: f32,
    pub ligatures: Ligatures,
}

impl SpanStyle {
    pub const BLACK: [u8; 4] = [0, 0, 0, 255];

    pub const fn new(font_size: f32) -> Self {
        Self {
            family: None,
            font_size,
            weight: REGULAR,
            italic: false,
            underline: false,
            strikethrough: false,
            color: Self::BLACK,
            highlight: None,
            vertical: VerticalAlign::Baseline,
            letter_spacing: 0.0,
            ligatures: Ligatures::Standard,
        }
    }
}

impl From<&ResolvedChar> for SpanStyle {
    fn from(r: &ResolvedChar) -> Self {
        Self {
            family: Some(r.family.clone()),
            font_size: r.size_pt,
            weight: r.weight,
            italic: r.italic,
            underline: r.underline,
            strikethrough: r.strikethrough,
            color: r.colour,
            highlight: r.highlight,
            vertical: r.vertical,
            letter_spacing: r.letter_spacing_pt,
            ligatures: r.ligatures,
        }
    }
}

/// A paragraph as the layout engine sees it.
///
/// `id` is stable for the life of the paragraph; `version` must be bumped on every edit
/// (cache key). Span ranges are byte ranges into `text` and should lie on char boundaries
/// (invalid ones are ignored rather than panicking).
#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphSnapshot {
    pub id: u64,
    pub version: u64,
    pub text: String,
    pub spans: Vec<(Range<usize>, SpanStyle)>,
    /// Formatting of text no span covers (the paragraph style); `None` uses the layout
    /// defaults (`LayoutOptions`).
    pub base: Option<SpanStyle>,
    pub space_before_pt: f32,
    pub space_after_pt: f32,
    pub align: Align,
    /// Indents from the margins, points; the first-line indent is relative to the left one
    /// (negative hangs).
    pub indent_left_pt: f32,
    pub indent_right_pt: f32,
    pub indent_first_pt: f32,
    /// `None` uses the layout default (`LayoutOptions::line_height`).
    pub line_spacing: Option<LineSpacing>,
    /// Pagination: keep on the page of the next paragraph, never split, and avoid a lone
    /// first or last line on a page.
    pub keep_with_next: bool,
    pub keep_together: bool,
    pub widow_control: bool,
    /// Break words at line ends at their syllables, with a hyphen (English patterns).
    pub hyphenate: bool,
    /// The locale to give the font selector for Han ideographs and CJK punctuation with
    /// nothing in the paragraph to say which language they are (from the document language;
    /// see [`crate::script::locale_for_tag`]).
    pub locale: Option<&'static str>,
    /// Which Cyrillic hyphenation patterns to use for words that do not show their own
    /// language by their letters (from the document language).
    pub cyrillic: Option<crate::script::Cyrillic>,
    /// The document is set vertically: East Asian punctuation and kana take their vertical
    /// forms (the OpenType `vert` feature), as they are drawn upright in columns.
    pub vertical: bool,
    /// The paragraph is in a table cell that stays upright on a vertical page: laid out as
    /// ordinary horizontal text whatever `vertical` says (set by the layout state).
    pub upright: bool,
    /// Ruby: the readings set beside ranges of the text (byte ranges, reading).
    pub ruby: Vec<(std::ops::Range<usize>, String)>,
    /// Border box drawn around the text (DOCX `w:pBdr`), if any.
    pub borders: Option<libertum_model::Borders>,
    /// Explicit tab stops; `None` uses the default stops every half inch.
    pub tab_stops: Option<libertum_model::TabStops>,
    /// Starts on a new page.
    pub page_break_before: bool,
    /// Starts a new section (and a new page); see `libertum_model::Section`.
    pub section_start: bool,
    /// Generated table-of-contents entry: id of the heading whose page number it shows.
    pub toc_target: Option<u64>,
    /// List item: its bullet or number, drawn in the hanging indent of the first line.
    /// Filled in by the caller (numbering depends on the paragraphs before it).
    pub list_label: Option<String>,
    /// The model's list item, so a caller can tell when numbering may have changed.
    pub list: Option<libertum_model::ListItem>,
    /// Footnote/endnote text attached to this paragraph.
    pub notes: Vec<NoteSnapshot>,
    /// Inline images, sorted by offset. Each is laid out as an inline box at its offset.
    pub images: Vec<ImageSnapshot>,
}

/// An inline image as layout sees it: where it sits, how large it is drawn and what it
/// shows. The bytes are shared with the model (`Arc`), never copied or decoded here.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageSnapshot {
    pub offset: usize,
    pub width_pt: f32,
    pub height_pt: f32,
    pub media: Arc<libertum_model::Media>,
}

/// A note as layout sees it: kind, anchor, text and computed number.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteSnapshot {
    pub kind: libertum_model::NoteKind,
    pub offset: usize,
    pub text: String,
    pub number: u32,
}

/// Phase 0 name kept so the spike examples and tests compile unchanged.
pub type SpikeParagraph = ParagraphSnapshot;

impl ParagraphSnapshot {
    /// Sets the paragraph in vertical text (or back). The paragraph is laid out on the page
    /// turned a quarter turn, so a picture's box must be as wide along the line as the picture
    /// is tall: its width and height swap, and turned back onto the sheet the box has the
    /// picture's own shape, so it can be drawn upright. Does nothing when already so.
    /// Takes what a document's second language adds: the regional font for East Asian text
    /// and the Cyrillic hyphenation hint, when the first language gave none.
    pub fn add_language(&mut self, tag: Option<&str>) {
        if self.locale.is_none() {
            self.locale = tag.and_then(crate::script::locale_for_tag);
        }
        if self.cyrillic.is_none() {
            self.cyrillic = tag.and_then(crate::script::cyrillic_for_tag);
        }
    }

    pub fn set_vertical(&mut self, on: bool) {
        if self.vertical == on {
            return;
        }
        self.vertical = on;
        for image in &mut self.images {
            std::mem::swap(&mut image.width_pt, &mut image.height_pt);
        }
    }

    pub fn new(id: u64, text: impl Into<String>) -> Self {
        Self {
            id,
            version: 0,
            text: text.into(),
            spans: Vec::new(),
            base: None,
            space_before_pt: 0.0,
            space_after_pt: 6.0,
            align: Align::Start,
            indent_left_pt: 0.0,
            indent_right_pt: 0.0,
            indent_first_pt: 0.0,
            line_spacing: None,
            keep_with_next: false,
            keep_together: false,
            widow_control: false,
            hyphenate: false,
            locale: None,
            cyrillic: None,
            vertical: false,
            upright: false,
            ruby: Vec::new(),
            borders: None,
            tab_stops: None,
            page_break_before: false,
            section_start: false,
            toc_target: None,
            list_label: None,
            list: None,
            notes: Vec::new(),
            images: Vec::new(),
        }
    }

    /// True when the paragraph must start at the top of a page.
    pub fn forces_new_page(&self) -> bool {
        self.page_break_before || self.section_start
    }

    /// Converts a model paragraph into the layout-facing snapshot, resolving its formatting
    /// (document defaults, then the paragraph style, then each run's direct formatting).
    pub fn from_model(p: &Paragraph, styles: &StyleSheet, language: Option<&str>) -> Self {
        let base = styles.resolve_char(p.props.style);
        let para = styles.resolve_props(&p.props);
        Self {
            id: p.id.0,
            version: p.version,
            text: p.text().to_owned(),
            spans: p
                .runs()
                .iter()
                .map(|run| (run.range.clone(), SpanStyle::from(&base.apply(&run.format))))
                .collect(),
            base: Some(SpanStyle::from(&base)),
            space_before_pt: para.space_before_pt,
            space_after_pt: para.space_after_pt,
            align: para.align,
            indent_left_pt: para.indent_left_pt,
            indent_right_pt: para.indent_right_pt,
            indent_first_pt: para.indent_first_pt,
            line_spacing: Some(para.line_spacing),
            keep_with_next: para.keep_with_next,
            keep_together: para.keep_together,
            widow_control: para.widow_control,
            hyphenate: para.hyphenate,
            locale: language.and_then(crate::script::locale_for_tag),
            cyrillic: language.and_then(crate::script::cyrillic_for_tag),
            vertical: false,
            upright: false,
            ruby: p
                .ruby()
                .iter()
                .map(|r| (r.range.clone(), r.text.clone()))
                .collect(),
            borders: para.borders,
            tab_stops: para.tab_stops,
            page_break_before: p.props.page_break_before,
            section_start: p.props.section_start,
            toc_target: p.props.toc_target.map(|id| id.0),
            list_label: None,
            list: p.props.list,
            notes: p
                .notes()
                .iter()
                .map(|note| NoteSnapshot {
                    kind: note.kind,
                    offset: note.offset,
                    text: note.text.clone(),
                    number: 0,
                })
                .collect(),
            images: p
                .images()
                .iter()
                .map(|image| ImageSnapshot {
                    offset: image.offset,
                    width_pt: image.width_pt,
                    height_pt: image.height_pt,
                    media: image.media.clone(),
                })
                .collect(),
        }
    }
}

impl SpikeParagraph {
    /// Inserts `ch` at byte offset `at` (must be on a char boundary; clamped otherwise) and
    /// keeps span ranges consistent: spans after the insertion shift right, a span containing
    /// the insertion point grows. Does not bump `version`; use
    /// `LayoutState::edit_paragraph`, which does.
    pub fn insert_char(&mut self, at: usize, ch: char) {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        self.text.insert(at, ch);
        let n = ch.len_utf8();
        for (r, _) in &mut self.spans {
            if r.start >= at {
                r.start += n;
                r.end += n;
            } else if r.end > at {
                r.end += n;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Deterministic document generator (shared by tests and the spike_a example).
// ---------------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
    "ad",
    "minim",
    "veniam",
    "quis",
    "nostrud",
    "exercitation",
    "ullamco",
    "laboris",
    "nisi",
    "aliquip",
    "ex",
    "ea",
    "commodo",
    "consequat",
    "duis",
    "aute",
    "irure",
    "in",
    "reprehenderit",
    "voluptate",
    "velit",
    "esse",
    "cillum",
    "fugiat",
    "nulla",
    "pariatur",
    "excepteur",
    "sint",
    "occaecat",
    "cupidatat",
    "non",
    "proident",
    "sunt",
    "culpa",
    "qui",
    "officia",
    "deserunt",
    "mollit",
    "anim",
    "id",
    "est",
    "laborum",
    "café",
    "naïve",
    "résumé",
    "\u{201C}quoted\u{201D}",
    "\u{2014}",
];

/// Number of leading entries of `WORDS` that are plain words (safe for headings).
const PLAIN_WORDS: usize = 62;

fn capitalise_into(w: &str, out: &mut String) {
    let mut cs = w.chars();
    if let Some(c) = cs.next() {
        out.extend(c.to_uppercase());
        out.push_str(cs.as_str());
    }
}

fn sentence(rng: &mut Rng, out: &mut String) {
    let n = 6 + rng.below(14);
    for i in 0..n {
        let w = WORDS[rng.below(WORDS.len())];
        if i == 0 {
            capitalise_into(w, out);
        } else {
            out.push(' ');
            out.push_str(w);
        }
    }
    out.push_str(". ");
}

/// Generates a deterministic pseudo-random document of roughly `approx_pages` A4 pages
/// (11 pt body text, 1-inch margins): headings, body paragraphs with bold/italic/size/colour
/// spans, and the odd empty paragraph. The same `seed` gives the same document.
pub fn generate_document(approx_pages: usize, seed: u64) -> Vec<SpikeParagraph> {
    // Calibrated for 11 pt serif on an A4 page with 1-inch margins.
    let target_chars = approx_pages * CHARS_PER_PAGE;
    let mut rng = Rng(seed.max(1) ^ 0x9E37_79B9_7F4A_7C15);
    let mut paras = Vec::new();
    let mut total = 0usize;
    let mut id = 0u64;
    while total < target_chars {
        let k = paras.len();
        if k % 14 == 0 {
            let mut text = String::new();
            for i in 0..(2 + rng.below(4)) {
                if i > 0 {
                    text.push(' ');
                }
                capitalise_into(WORDS[rng.below(PLAIN_WORDS)], &mut text);
            }
            let mut p = SpikeParagraph::new(id, text);
            let style = SpanStyle {
                font_size: if k % 56 == 0 { 22.0 } else { 16.0 },
                weight: BOLD,
                color: [20, 40, 90, 255],
                ..SpanStyle::new(0.0)
            };
            p.spans.push((0..p.text.len(), style));
            p.space_before_pt = 18.0;
            p.space_after_pt = 8.0;
            total += p.text.len();
            paras.push(p);
        } else if rng.chance(2) {
            paras.push(SpikeParagraph::new(id, ""));
        } else {
            let mut text = String::new();
            for _ in 0..(2 + rng.below(7)) {
                sentence(&mut rng, &mut text);
            }
            let text = text.trim_end().to_owned();
            let mut p = SpikeParagraph::new(id, text);
            let boundaries: Vec<usize> = p
                .text
                .char_indices()
                .filter(|(_, c)| *c == ' ')
                .map(|(i, _)| i)
                .collect();
            for _ in 0..rng.below(4) {
                if boundaries.len() < 4 {
                    break;
                }
                let a = rng.below(boundaries.len() - 2);
                let b = (a + 1 + rng.below(3)).min(boundaries.len() - 1);
                let range = boundaries[a] + 1..boundaries[b];
                if range.start >= range.end
                    || p.spans
                        .iter()
                        .any(|(r, _)| r.start < range.end && range.start < r.end)
                {
                    continue;
                }
                let mut s = SpanStyle::new(11.0);
                match rng.below(5) {
                    0 => s.weight = BOLD,
                    1 => s.italic = true,
                    2 => {
                        s.weight = BOLD;
                        s.italic = true;
                    }
                    3 => s.color = [170, 30, 30, 255],
                    _ => s.font_size = 14.0,
                }
                p.spans.push((range, s));
            }
            p.spans.sort_by_key(|(r, _)| r.start);
            total += p.text.len();
            paras.push(p);
        }
        id += 1;
    }
    paras
}

/// Converts the generated spike document into a `libertum-model` document, so the app and the
/// editor work on the real model while the deterministic generator stays in one place.
pub fn generate_model_document(approx_pages: usize, seed: u64) -> Document {
    let paragraphs = generate_document(approx_pages, seed)
        .into_iter()
        .map(|s| {
            let runs = s
                .spans
                .into_iter()
                .map(|(range, style)| Run {
                    range,
                    format: CharFormat {
                        size_pt: Some(style.font_size),
                        weight: (style.weight != REGULAR).then_some(style.weight),
                        italic: style.italic.then_some(true),
                        colour: (style.color != SpanStyle::BLACK).then_some(style.color),
                        ..CharFormat::default()
                    },
                })
                .collect();
            let mut p = Paragraph::with_runs(BlockId(s.id), s.text, runs);
            p.version = s.version;
            p.props = ParaProps {
                format: ParaFormat {
                    space_before_pt: Some(s.space_before_pt),
                    space_after_pt: Some(s.space_after_pt),
                    ..ParaFormat::default()
                },
                ..ParaProps::default()
            };
            p
        })
        .collect();
    Document::from_paragraphs(paragraphs)
}

/// Approximate characters per generated page; tuned in the Spike A example.
pub const CHARS_PER_PAGE: usize = 3_900;
