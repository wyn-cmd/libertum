//! The style sheet both directions agree on, and CSS properties mapped onto the model.
//!
//! The reader gives every paragraph a built-in style chosen by its tag (Body, a heading,
//! Quote) from a fresh [`Baseline`] sheet, then applies the element's CSS as direct
//! formatting. The writer computes the same baseline and writes only the CSS needed to get
//! from it to the document's resolved formatting, so a saved file reads back looking the
//! same even when the document used styles of its own.

use std::sync::Arc;

use libertum_model::{
    Align, BorderSides, Borders, CharFormat, Ligatures, LineSpacing, PageSetup, ParaFormat,
    ParaProps, ParagraphStyle, StyleId, StyleSheet, VerticalAlign,
};

use crate::css;

/// The family code and preformatted text are given.
pub const MONOSPACE: &str = "Courier New";

/// Name of the style a horizontal rule (`<hr>`) becomes.
pub const RULE_STYLE: &str = "Horizontal Rule";

/// The reader's style sheet for a document whose defaults are `family`, `size_pt` and
/// `colour`.
pub struct Baseline {
    pub sheet: StyleSheet,
    pub rule: StyleId,
    /// Width images and tables are fitted to.
    pub content_width_pt: f32,
}

impl Baseline {
    pub fn new(family: &str, size_pt: f32, colour: [u8; 4]) -> Self {
        let mut sheet = StyleSheet::with_defaults(family, size_pt);
        sheet.defaults.colour = colour;
        sheet.paragraph.push(ParagraphStyle {
            name: RULE_STYLE.to_owned(),
            base: Some(StyleId::BODY),
            char: CharFormat::default(),
            para: ParaFormat {
                space_before_pt: Some(2.0),
                space_after_pt: Some(10.0),
                borders: Some(Borders {
                    sides: BorderSides {
                        bottom: true,
                        ..BorderSides::NONE
                    },
                    width_pt: 0.75,
                    colour: [150, 150, 150, 255],
                    space_pt: 1.0,
                }),
                ..ParaFormat::default()
            },
            next: Some(StyleId::BODY),
            outline_level: None,
        });
        let rule = StyleId(sheet.paragraph.len() as u32 - 1);
        Self {
            sheet,
            rule,
            content_width_pt: PageSetup::default().content_width_pt(),
        }
    }

    /// The baseline style with this name (case-insensitive).
    pub fn style_named(&self, name: &str) -> Option<StyleId> {
        self.sheet
            .paragraph
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(name.trim()))
            .map(|i| StyleId(i as u32))
    }
}

/// The built-in style for a heading level (5 and 6 share Heading 4's look).
pub fn heading_style(level: u8) -> StyleId {
    match level {
        0 | 1 => StyleId::HEADING_1,
        2 => StyleId::HEADING_2,
        3 => StyleId::HEADING_3,
        _ => StyleId::HEADING_4,
    }
}

/// Applies one character property. `size_now` is the current size (for `em`).
pub fn apply_char_css(format: &mut CharFormat, key: &str, value: &str, size_now: f32) {
    match key {
        "color" => {
            if let Some(c) = css::colour(value) {
                format.colour = Some([c[0], c[1], c[2], 255]);
            }
        }
        "background" | "background-color" => {
            let lower = value.trim().to_ascii_lowercase();
            if lower == "transparent" || lower == "none" {
                format.highlight = Some(None);
            } else if let Some(c) = css::background_colour(value) {
                format.highlight = Some((c[3] > 0).then_some([c[0], c[1], c[2], 255]));
            }
        }
        "font-weight" => {
            if let Some(w) = css::font_weight(value) {
                format.weight = Some(w);
            }
        }
        "font-style" => {
            let lower = value.trim().to_ascii_lowercase();
            format.italic = Some(lower == "italic" || lower.starts_with("oblique"));
        }
        "text-decoration" | "text-decoration-line" => {
            let lower = value.to_ascii_lowercase();
            if lower.contains("none") {
                format.underline = Some(false);
                format.strikethrough = Some(false);
            } else {
                if lower.contains("underline") {
                    format.underline = Some(true);
                }
                if lower.contains("line-through") {
                    format.strikethrough = Some(true);
                }
            }
        }
        "font-family" => {
            if let Some(f) = css::font_family(value) {
                format.family = Some(Arc::from(f));
            } else if value.to_ascii_lowercase().contains("monospace") {
                format.family = Some(Arc::from(MONOSPACE));
            }
        }
        "font-size" => {
            if let Some(s) = css::font_size(value, size_now) {
                format.size_pt = Some(s);
            }
        }
        "vertical-align" => match value.trim().to_ascii_lowercase().as_str() {
            "super" => format.vertical = Some(VerticalAlign::Superscript),
            "sub" => format.vertical = Some(VerticalAlign::Subscript),
            "baseline" => format.vertical = Some(VerticalAlign::Baseline),
            _ => {}
        },
        "letter-spacing" => {
            if value.trim().eq_ignore_ascii_case("normal") {
                format.letter_spacing_pt = Some(0.0);
            } else if let Some(v) = css::length(value, size_now) {
                format.letter_spacing_pt = Some(v);
            }
        }
        "font-variant-ligatures" => {
            let lower = value.to_ascii_lowercase();
            format.ligatures = Some(if lower.contains("none") || lower.contains("no-common") {
                Ligatures::None
            } else if lower.contains("discretionary") && !lower.contains("no-discretionary") {
                Ligatures::All
            } else {
                Ligatures::Standard
            });
        }
        _ => {}
    }
}

/// Applies one paragraph property.
pub fn apply_para_css(props: &mut ParaProps, key: &str, value: &str, size: f32) {
    let f = &mut props.format;
    let len = |v: &str| css::length(v, size);
    match key {
        "margin" => {
            if let Some([t, r, b, l]) = css::box_lengths(value, size) {
                f.space_before_pt = Some(t);
                f.indent_right_pt = Some(r);
                f.space_after_pt = Some(b);
                f.indent_left_pt = Some(l);
            }
        }
        "margin-top" => f.space_before_pt = len(value).or(f.space_before_pt),
        "margin-bottom" => f.space_after_pt = len(value).or(f.space_after_pt),
        "margin-left" | "margin-inline-start" | "padding-left" => {
            f.indent_left_pt = len(value).or(f.indent_left_pt);
        }
        "margin-right" | "margin-inline-end" => {
            f.indent_right_pt = len(value).or(f.indent_right_pt);
        }
        "text-indent" => f.indent_first_pt = len(value).or(f.indent_first_pt),
        "text-align" => {
            f.align = match value.trim().to_ascii_lowercase().as_str() {
                "left" | "start" => Some(Align::Start),
                "center" => Some(Align::Centre),
                "right" | "end" => Some(Align::End),
                "justify" => Some(Align::Justify),
                _ => f.align,
            };
        }
        "line-height" => {
            let v = value.trim().to_ascii_lowercase();
            f.line_spacing = if v == "normal" {
                Some(LineSpacing::Multiple(1.0))
            } else if let Ok(n) = v.parse::<f32>() {
                n.is_finite()
                    .then_some(LineSpacing::Multiple(n))
                    .or(f.line_spacing)
            } else if let Some(p) = v.strip_suffix('%') {
                p.trim()
                    .parse::<f32>()
                    .ok()
                    .map(|p| LineSpacing::Multiple(p / 100.0))
                    .or(f.line_spacing)
            } else {
                len(&v).map(LineSpacing::Exactly).or(f.line_spacing)
            };
        }
        // Libertum's own exact line rule, which `line-height` cannot say.
        "--libertum-line" => {
            let mut parts = value.split_whitespace();
            let kind = parts.next().unwrap_or("");
            if let Some(v) = parts.next().and_then(len) {
                f.line_spacing = match kind {
                    "at-least" => Some(LineSpacing::AtLeast(v)),
                    "leading" => Some(LineSpacing::Leading(v)),
                    "exactly" => Some(LineSpacing::Exactly(v)),
                    _ => f.line_spacing,
                };
            }
        }
        "break-before" | "page-break-before" => {
            let v = value.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "page" | "always" | "left" | "right") {
                props.page_break_before = true;
            }
        }
        "break-after" | "page-break-after" => match value.trim().to_ascii_lowercase().as_str() {
            "avoid" | "avoid-page" => f.keep_with_next = Some(true),
            "auto" => f.keep_with_next = Some(false),
            _ => {}
        },
        "break-inside" | "page-break-inside" => match value.trim().to_ascii_lowercase().as_str() {
            "avoid" | "avoid-page" => f.keep_together = Some(true),
            "auto" => f.keep_together = Some(false),
            _ => {}
        },
        "widows" | "orphans" => {
            if let Ok(n) = value.trim().parse::<u32>() {
                f.widow_control = Some(n >= 2);
            }
        }
        "hyphens" | "-webkit-hyphens" => match value.trim().to_ascii_lowercase().as_str() {
            "auto" => f.hyphenate = Some(true),
            "manual" | "none" => f.hyphenate = Some(false),
            _ => {}
        },
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" => {
            let mut borders = f.borders.unwrap_or_else(Borders::none);
            let line = css::border(value);
            let sides = match key {
                "border" => BorderSides::ALL,
                "border-top" => BorderSides {
                    top: true,
                    ..BorderSides::NONE
                },
                "border-right" => BorderSides {
                    right: true,
                    ..BorderSides::NONE
                },
                "border-bottom" => BorderSides {
                    bottom: true,
                    ..BorderSides::NONE
                },
                _ => BorderSides {
                    left: true,
                    ..BorderSides::NONE
                },
            };
            let set = |b: &mut bool, from: bool, on: bool| {
                if from {
                    *b = on;
                }
            };
            let on = line.is_some();
            set(&mut borders.sides.top, sides.top, on);
            set(&mut borders.sides.right, sides.right, on);
            set(&mut borders.sides.bottom, sides.bottom, on);
            set(&mut borders.sides.left, sides.left, on);
            if let Some((width, colour)) = line {
                borders.width_pt = width;
                borders.colour = colour;
            }
            f.borders = Some(borders);
        }
        "padding" => {
            if let Some(b) = &mut f.borders
                && let Some([t, ..]) = css::box_lengths(value, size)
            {
                b.space_pt = t;
            }
        }
        _ => {}
    }
}
