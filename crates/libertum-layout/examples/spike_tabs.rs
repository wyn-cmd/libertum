//! Spike E: tab stops on parley 0.12, using one in-flow inline box per tab.
//!
//! parley 0.12 has no tab stops: a `\t` is whitespace with whatever advance the font gives it.
//! This example tests the route proposed in `docs/STATUS.md`:
//!
//! 1. remove the tab characters from the text handed to parley (so they contribute no glyph),
//! 2. put one `InlineBox` (`InFlow`) at each tab's byte offset,
//! 3. lay the paragraph out once with all box widths 0, read each box's x, and compute the
//!    width that puts the following text at the next tab stop,
//! 4. rebuild and repeat until the widths stop changing (line breaks can move, which moves x).
//!
//! Four kinds are exercised: left, centre, right and decimal. The output reports convergence,
//! final alignment error and the cost per paragraph.
//!
//! Run: `cargo run --release -p libertum-layout --example spike_tabs`

use std::borrow::Cow;
use std::time::Instant;

use libertum_layout::parley::editing::Cursor;
use libertum_layout::parley::layout::Affinity;
use libertum_layout::parley::style::{FontFamily, FontFamilyName, FontWeight, GenericFamily};
use libertum_layout::parley::{
    Alignment, AlignmentOptions, FontContext, InlineBox, InlineBoxKind, Layout, LayoutContext,
    LineHeight, PositionedLayoutItem, StyleProperty, VerticalAlign,
};

type Brush = ();
type TabLayout = Layout<Brush>;

/// Default tab stops every half inch, as Word and Pages.
const STOP_PT: f32 = 36.0;
/// Content width of A4 with 1-inch margins.
const WIDTH_PT: f32 = 451.276;
/// Body size, points.
const SIZE_PT: f32 = 11.0;
/// Widest alignment error accepted, points.
const TOLERANCE_PT: f32 = 0.05;
/// Width changes below this stop the iteration.
const EPSILON_PT: f32 = 0.01;
/// Hard cap on passes: a tab whose line-break position oscillates must terminate the spike.
const MAX_PASSES: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabKind {
    Left,
    Centre,
    Right,
    Decimal,
}

#[derive(Clone, Copy, Debug)]
struct TabSpec {
    kind: TabKind,
    /// Explicit stop in points; `None` means the next default stop.
    stop: Option<f32>,
}

impl TabSpec {
    fn left() -> Self {
        Self {
            kind: TabKind::Left,
            stop: None,
        }
    }
}

/// One paragraph's layout input: the text without tab characters, and each tab's position in
/// that text (byte offset) plus its kind.
struct TabbedParagraph {
    text: String,
    tabs: Vec<(usize, TabSpec)>,
}

impl TabbedParagraph {
    /// Splits `source` (which may contain `\t`) into layout text plus tab positions. `specs`
    /// gives the kind of each tab, in order; missing entries are left tabs.
    fn new(source: &str, specs: &[TabSpec]) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut tabs = Vec::new();
        let mut tab_index = 0;
        for c in source.chars() {
            if c == '\t' {
                tabs.push((
                    text.len(),
                    specs.get(tab_index).copied().unwrap_or_else(TabSpec::left),
                ));
                tab_index += 1;
            } else {
                text.push(c);
            }
        }
        Self { text, tabs }
    }

    /// Byte offset where the text after tab `i` ends: the next tab, or the paragraph end.
    fn segment_end(&self, i: usize) -> usize {
        self.tabs.get(i + 1).map_or(self.text.len(), |(at, _)| *at)
    }
}

fn family() -> FontFamily<'static> {
    FontFamily::List(Cow::Owned(vec![
        FontFamilyName::Named(Cow::Borrowed("Cambria")),
        FontFamilyName::Named(Cow::Borrowed("Times New Roman")),
        FontFamilyName::Named(Cow::Borrowed("Georgia")),
        FontFamilyName::Generic(GenericFamily::Serif),
    ]))
}

/// Builds one layout with the given tab box widths.
fn build(
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<Brush>,
    para: &TabbedParagraph,
    widths: &[f32],
) -> TabLayout {
    let mut b = lcx.ranged_builder(fcx, &para.text, 1.0, false);
    b.push_default(StyleProperty::FontFamily(family()));
    b.push_default(StyleProperty::FontSize(SIZE_PT));
    b.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(1.2)));
    b.push_default(StyleProperty::FontWeight(FontWeight::new(400.0)));
    for (i, (at, _)) in para.tabs.iter().enumerate() {
        b.push_inline_box(InlineBox {
            id: i as u64,
            kind: InlineBoxKind::InFlow,
            index: *at,
            width: widths.get(i).copied().unwrap_or(0.0),
            height: SIZE_PT * 1.2,
            baseline: Some(SIZE_PT * 0.8),
            vertical_align: VerticalAlign::BASELINE,
        });
    }
    let mut layout = b.build(&para.text);
    layout.break_all_lines(Some(WIDTH_PT));
    layout.align(Alignment::Start, AlignmentOptions::default());
    layout
}

/// (line index, x, width, height) of each positioned inline box, by box id.
fn boxes(layout: &TabLayout) -> Vec<(usize, f32, f32)> {
    let mut out = Vec::new();
    for (line_index, line) in layout.lines().enumerate() {
        for item in line.items() {
            if let PositionedLayoutItem::InlineBox(b) = item {
                let id = b.id as usize;
                if out.len() <= id {
                    out.resize(id + 1, (0, 0.0, 0.0));
                }
                out[id] = (line_index, b.x, b.width);
            }
        }
    }
    out
}

/// x coordinate of byte offset `at` in the laid-out text, after alignment.
fn cursor_x(layout: &TabLayout, at: usize) -> f32 {
    let cursor = Cursor::from_byte_index(layout, at, Affinity::Downstream);
    cursor.geometry(layout, WIDTH_PT).x0 as f32
}

/// Width of the text after tab `i`, up to the next tab or the paragraph end, in the current
/// layout. `None` when that text wrapped to another line: right, centre and decimal tabs cannot
/// align across a wrap.
fn segment_width(
    layout: &TabLayout,
    placed: &[(usize, f32, f32)],
    para: &TabbedParagraph,
    i: usize,
) -> Option<f32> {
    let (line, x, width) = *placed.get(i)?;
    let end = para.segment_end(i);
    let end_x = cursor_x(layout, end);
    let end_line = line_of(layout, end)?;
    // The box itself sits between `x` and the segment, so subtract its width.
    (end_line == line).then_some((end_x - x - width).max(0.0))
}

fn line_of(layout: &TabLayout, at: usize) -> Option<usize> {
    layout
        .lines()
        .enumerate()
        .find(|(_, line)| {
            let range = line.text_range();
            at >= range.start && at <= range.end
        })
        .map(|(i, _)| i)
}

/// Decimal offset inside tab `i`'s segment, if it has a '.'.
fn decimal_offset(para: &TabbedParagraph, i: usize) -> Option<usize> {
    let start = para.tabs.get(i)?.0;
    let end = para.segment_end(i);
    para.text.get(start..end)?.find('.').map(|at| start + at)
}

fn next_stop(after: f32) -> f32 {
    ((after / STOP_PT).floor() + 1.0) * STOP_PT
}

/// Width for tab `i` in the current layout.
fn desired_width(
    layout: &TabLayout,
    placed: &[(usize, f32, f32)],
    para: &TabbedParagraph,
    i: usize,
) -> f32 {
    let Some(&(_, x, _)) = placed.get(i) else {
        return 0.0;
    };
    let spec = para.tabs[i].1;
    let explicit = spec.stop;
    let stop_after = |after: f32| explicit.unwrap_or_else(|| next_stop(after));
    match spec.kind {
        TabKind::Left => (stop_after(x) - x).max(0.0),
        TabKind::Right => {
            let Some(suffix) = segment_width(layout, placed, para, i) else {
                return 0.0;
            };
            (stop_after(x + suffix) - x - suffix).max(0.0)
        }
        TabKind::Centre => {
            let Some(suffix) = segment_width(layout, placed, para, i) else {
                return 0.0;
            };
            (stop_after(x + suffix / 2.0) - x - suffix / 2.0).max(0.0)
        }
        TabKind::Decimal => {
            let Some(dot) = decimal_offset(para, i) else {
                // No decimal point: behave like a right tab.
                let Some(suffix) = segment_width(layout, placed, para, i) else {
                    return 0.0;
                };
                return (stop_after(x + suffix) - x - suffix).max(0.0);
            };
            let width = placed[i].2;
            let prefix = (cursor_x(layout, dot) - x - width).max(0.0);
            (stop_after(x + prefix) - x - prefix).max(0.0)
        }
    }
}

/// Solves tab widths for one paragraph. Returns the final layout, widths and pass count.
fn solve(
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<Brush>,
    para: &TabbedParagraph,
) -> (TabLayout, Vec<f32>, usize) {
    let mut widths = vec![0.0f32; para.tabs.len()];
    let mut layout = build(fcx, lcx, para, &widths);
    let mut passes = 1;
    loop {
        let placed = boxes(&layout);
        let next: Vec<f32> = (0..para.tabs.len())
            .map(|i| desired_width(&layout, &placed, para, i))
            .collect();
        let changed = next
            .iter()
            .zip(&widths)
            .any(|(a, b)| (a - b).abs() > EPSILON_PT);
        if !changed || passes >= MAX_PASSES {
            return (layout, widths, passes);
        }
        widths = next;
        layout = build(fcx, lcx, para, &widths);
        passes += 1;
    }
}

/// Final alignment error of every tab in a solved layout, points.
fn alignment_errors(layout: &TabLayout, para: &TabbedParagraph) -> Vec<f32> {
    let placed = boxes(layout);
    let mut out = Vec::new();
    for (i, (_, spec)) in para.tabs.iter().enumerate() {
        let Some(&(_, x, width)) = placed.get(i) else {
            continue;
        };
        let explicit = spec.stop;
        let stop_after = |after: f32| explicit.unwrap_or_else(|| next_stop(after));
        let error = match spec.kind {
            TabKind::Left => x + width - stop_after(x),
            TabKind::Right => match segment_width(layout, &placed, para, i) {
                Some(suffix) => x + width + suffix - stop_after(x + suffix),
                None => f32::NAN,
            },
            TabKind::Centre => match segment_width(layout, &placed, para, i) {
                Some(suffix) => x + width + suffix / 2.0 - stop_after(x + suffix / 2.0),
                None => f32::NAN,
            },
            TabKind::Decimal => match decimal_offset(para, i) {
                Some(dot) => {
                    let prefix = (cursor_x(layout, dot) - x - width).max(0.0);
                    x + width + prefix - stop_after(x + prefix)
                }
                None => f32::NAN,
            },
        };
        out.push(error);
    }
    out
}

/// Per-pass trace for one paragraph (set `SPIKE_TABS_DEBUG=1`).
fn trace(
    name: &str,
    fcx: &mut FontContext,
    lcx: &mut LayoutContext<Brush>,
    para: &TabbedParagraph,
) {
    let mut widths = vec![0.0f32; para.tabs.len()];
    let mut layout = build(fcx, lcx, para, &widths);
    println!("\n{name} trace (text {:?})", para.text);
    for pass in 0..MAX_PASSES {
        let placed = boxes(&layout);
        let mut line = String::new();
        for (i, (_, x, w)) in placed.iter().enumerate() {
            let end = para.segment_end(i);
            let suffix = segment_width(&layout, &placed, para, i);
            line.push_str(&format!(
                " tab{i}:x={x:.2},w={w:.2},end={:.2},suffix={suffix:?}",
                cursor_x(&layout, end)
            ));
        }
        println!(" pass {pass}: lines={}{line}", layout.lines().count());
        let next: Vec<f32> = (0..para.tabs.len())
            .map(|i| desired_width(&layout, &placed, para, i))
            .collect();
        println!("   next widths: {next:?}");
        widths = next;
        layout = build(fcx, lcx, para, &widths);
    }
}

fn check(name: &str, fcx: &mut FontContext, lcx: &mut LayoutContext<Brush>, para: TabbedParagraph) {
    let (layout, widths, passes) = solve(fcx, lcx, &para);
    let errors = alignment_errors(&layout, &para);
    let worst = errors
        .iter()
        .filter(|e| e.is_finite())
        .fold(0.0f32, |m, e| m.max(e.abs()));
    let lines = layout.lines().count();
    let tabs = para.tabs.len();
    let ok = worst <= TOLERANCE_PT && passes < MAX_PASSES;
    println!(
        "{name:<28} tabs={tabs:<3} lines={lines:<2} passes={passes} worst_error={worst:.3}pt {}",
        if ok { "PASS" } else { "CHECK" }
    );
    println!(
        "  widths: {}",
        widths
            .iter()
            .map(|w| format!("{w:.2}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
}

fn main() {
    let mut fcx = FontContext::new();
    let mut lcx = LayoutContext::<Brush>::new();

    println!("Spike E: tab stops via parley 0.12 inline boxes");
    println!("content width {WIDTH_PT} pt, body {SIZE_PT} pt, default stop {STOP_PT} pt\n");

    // 1. Left tabs in a line that fits.
    check(
        "left, one line",
        &mut fcx,
        &mut lcx,
        TabbedParagraph::new("Item\tWidgets\t42\tEach", &[]),
    );

    // 2. Many left tabs, so the line has to wrap; widths must still align on their line.
    check(
        "left, wrapped",
        &mut fcx,
        &mut lcx,
        TabbedParagraph::new(
            "Name\tQuantity\tUnit price\tTotal\tNotes\tWarehouse\tShelf\tBin\tChecked by",
            &[],
        ),
    );

    // 3. Right, centre and decimal on explicit stops.
    let right = TabSpec {
        kind: TabKind::Right,
        stop: Some(144.0),
    };
    let centre = TabSpec {
        kind: TabKind::Centre,
        stop: Some(216.0),
    };
    let decimal = TabSpec {
        kind: TabKind::Decimal,
        stop: Some(288.0),
    };
    if std::env::var("SPIKE_TABS_DEBUG").is_ok() {
        trace(
            "wrapped",
            &mut fcx,
            &mut lcx,
            &TabbedParagraph::new(
                "Name\tQuantity\tUnit price\tTotal\tNotes\tWarehouse\tShelf\tBin\tChecked by",
                &[],
            ),
        );
        trace(
            "right",
            &mut fcx,
            &mut lcx,
            &TabbedParagraph::new("Total\t1,234.50", &[right]),
        );
        trace(
            "centre",
            &mut fcx,
            &mut lcx,
            &TabbedParagraph::new("Status\tShipped", &[centre]),
        );
        trace(
            "decimal",
            &mut fcx,
            &mut lcx,
            &TabbedParagraph::new("Price\t1,234.50", &[decimal]),
        );
    }
    check(
        "right tab at 144 pt",
        &mut fcx,
        &mut lcx,
        TabbedParagraph::new("Total\t1,234.50", &[right]),
    );
    check(
        "centre tab at 216 pt",
        &mut fcx,
        &mut lcx,
        TabbedParagraph::new("Status\tShipped", &[centre]),
    );
    check(
        "decimal tab at 288 pt",
        &mut fcx,
        &mut lcx,
        TabbedParagraph::new("Price\t1,234.50", &[decimal]),
    );

    // 4. Tabs at the very start and end of a paragraph.
    check(
        "tab at start and end",
        &mut fcx,
        &mut lcx,
        TabbedParagraph::new("\tIndented\t", &[]),
    );

    // 5. A literal tab with no inline box, for comparison: what does parley do on its own?
    {
        let text = "a\tb";
        let mut b = lcx.ranged_builder(&mut fcx, text, 1.0, false);
        b.push_default(StyleProperty::FontFamily(family()));
        b.push_default(StyleProperty::FontSize(SIZE_PT));
        let mut layout = b.build(text);
        layout.break_all_lines(Some(WIDTH_PT));
        let line = layout.lines().next().expect("a line");
        let m = line.metrics();
        let glyphs = line
            .items()
            .filter_map(|item| match item {
                PositionedLayoutItem::GlyphRun(run) => Some(run.positioned_glyphs().count()),
                PositionedLayoutItem::InlineBox(_) => None,
            })
            .sum::<usize>();
        println!(
            "\nliteral \\t without boxes: line advance {:.2} pt, {} positioned glyphs",
            m.advance, glyphs
        );
    }

    // 6. Cost: one solve per paragraph, 1,000 paragraphs with three tabs each.
    let sample = TabbedParagraph::new("Name\t43\tWarehouse 7", &[]);
    let iterations = 1_000;
    let started = Instant::now();
    for _ in 0..iterations {
        let _ = solve(&mut fcx, &mut lcx, &sample);
    }
    let elapsed = started.elapsed();
    println!(
        "\nsolve cost over {iterations} paragraphs: {:.3} ms total, {:.1} us per paragraph",
        elapsed.as_secs_f64() * 1e3,
        elapsed.as_secs_f64() * 1e6 / iterations as f64
    );

    // 7. Cost of a document-sized set: 5,000 paragraphs with tabs, versus one pass each.
    let doc_sample = TabbedParagraph::new("Name\t43\tWarehouse 7\tChecked", &[]);
    let paragraphs = 5_000;
    let started = Instant::now();
    let mut passes = 0usize;
    for _ in 0..paragraphs {
        let (_, _, p) = solve(&mut fcx, &mut lcx, &doc_sample);
        passes += p;
    }
    let tabbed = started.elapsed();
    let started = Instant::now();
    for _ in 0..paragraphs {
        let _ = build(&mut fcx, &mut lcx, &doc_sample, &[0.0, 0.0, 0.0]);
    }
    let plain = started.elapsed();
    println!(
        "{paragraphs} paragraphs with 3 tabs: {:.1} ms ({:.2} passes each), single-pass baseline {:.1} ms, overhead {:.1}x",
        tabbed.as_secs_f64() * 1e3,
        passes as f64 / paragraphs as f64,
        plain.as_secs_f64() * 1e3,
        tabbed.as_secs_f64() / plain.as_secs_f64()
    );
}
