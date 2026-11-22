//! The little CSS the importer understands: inline `style` declarations, a `<style>` block's
//! rules for `body`/`html` and `@page`, colours and lengths. Anything else is ignored.

/// `property: value` pairs from a declaration list, property names in lower case.
pub fn declarations(style: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for part in split_outside_parens(style, ';') {
        let Some((key, value)) = part.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value
            .trim()
            .trim_end_matches("!important")
            .trim()
            .to_owned();
        if !key.is_empty() && !value.is_empty() {
            out.push((key, value));
        }
    }
    out
}

/// Splits at `sep`, but not inside parentheses (so `rgb(1, 2, 3)` stays whole) or quotes.
fn split_outside_parens(text: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, c) if c == sep && depth <= 0 => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Rules from a `<style>` block whose selector list names `selector` exactly (`body`,
/// `html`, `@page`), in order. Comments are removed first.
pub fn rules_for(sheet: &str, selector: &str) -> Vec<(String, String)> {
    let sheet = strip_comments(sheet);
    let mut out = Vec::new();
    let mut rest = sheet.as_str();
    while let Some(open) = rest.find('{') {
        let selectors = rest[..open].trim();
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        let body = &rest[open + 1..open + close];
        // A nested block (@media) is skipped whole: its inner rules are not ours to apply.
        if body.contains('{') {
            let Some(end) = find_block_end(rest, open) else {
                break;
            };
            rest = &rest[end..];
            continue;
        }
        if selectors
            .split(',')
            .any(|s| s.trim().eq_ignore_ascii_case(selector))
        {
            out.extend(declarations(body));
        }
        rest = &rest[open + close + 1..];
    }
    out
}

fn find_block_end(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// A CSS colour: `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`/`rgba()` and the common names.
/// `transparent` and unknown values are `None`.
pub fn colour(value: &str) -> Option<[u8; 4]> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255]),
            6 => Some([pair(0)?, pair(2)?, pair(4)?, 255]),
            8 => Some([pair(0)?, pair(2)?, pair(4)?, pair(6)?]),
            _ => None,
        };
    }
    if let Some(inner) = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|v| v.strip_suffix(')'))
    {
        let parts: Vec<&str> = inner
            .split(|c: char| c == ',' || c == '/' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let channel = |p: &str| -> Option<u8> {
            if let Some(percent) = p.strip_suffix('%') {
                let v: f32 = percent.parse().ok()?;
                Some((v.clamp(0.0, 100.0) * 2.55).round() as u8)
            } else {
                let v: f32 = p.parse().ok()?;
                Some(v.clamp(0.0, 255.0).round() as u8)
            }
        };
        let alpha = match parts.get(3) {
            Some(a) => {
                let v: f32 = match a.strip_suffix('%') {
                    Some(p) => p.parse::<f32>().ok()? / 100.0,
                    None => a.parse().ok()?,
                };
                (v.clamp(0.0, 1.0) * 255.0).round() as u8
            }
            None => 255,
        };
        return Some([
            channel(parts[0])?,
            channel(parts[1])?,
            channel(parts[2])?,
            alpha,
        ]);
    }
    named(&value)
}

fn named(name: &str) -> Option<[u8; 4]> {
    let rgb: [u8; 3] = match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "green" => [0, 128, 0],
        "lime" => [0, 255, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        "grey" | "gray" => [128, 128, 128],
        "silver" => [192, 192, 192],
        "lightgrey" | "lightgray" => [211, 211, 211],
        "darkgrey" | "darkgray" => [169, 169, 169],
        "maroon" => [128, 0, 0],
        "olive" => [128, 128, 0],
        "navy" => [0, 0, 128],
        "purple" => [128, 0, 128],
        "teal" => [0, 128, 128],
        "orange" => [255, 165, 0],
        "pink" => [255, 192, 203],
        "brown" => [165, 42, 42],
        "beige" => [245, 245, 220],
        "ivory" => [255, 255, 240],
        "wheat" => [245, 222, 179],
        "linen" => [250, 240, 230],
        _ => return None,
    };
    Some([rgb[0], rgb[1], rgb[2], 255])
}

/// The colour in a `background` shorthand (the first token that is a colour).
pub fn background_colour(value: &str) -> Option<[u8; 4]> {
    if let Some(c) = colour(value) {
        return Some(c);
    }
    tokens(value).into_iter().find_map(colour)
}

/// Splits a value at whitespace outside parentheses.
pub fn tokens(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    for (i, c) in value.char_indices() {
        match c {
            '(' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => {
                if let Some(s) = start.take() {
                    out.push(&value[s..i]);
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        out.push(&value[s..]);
    }
    out
}

/// A length in points. `em` and `rem` count against `font_pt`; percentages are refused.
pub fn length(value: &str, font_pt: f32) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    if value == "0" {
        return Some(0.0);
    }
    if let Some(n) = value
        .strip_suffix("rem")
        .or_else(|| value.strip_suffix("em"))
    {
        let n: f32 = n.trim().parse().ok()?;
        return n.is_finite().then_some(n * font_pt);
    }
    libertum_formats_common::units::parse(&value)
}

/// A font size: a length, a percentage of `parent_pt`, or a keyword.
pub fn font_size(value: &str, parent_pt: f32) -> Option<f32> {
    let value = value.trim().to_ascii_lowercase();
    let keyword = match value.as_str() {
        "xx-small" => Some(7.0),
        "x-small" => Some(7.5),
        "small" => Some(10.0),
        "medium" => Some(12.0),
        "large" => Some(13.5),
        "x-large" => Some(18.0),
        "xx-large" => Some(24.0),
        "smaller" => Some(parent_pt / 1.2),
        "larger" => Some(parent_pt * 1.2),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    if let Some(p) = value.strip_suffix('%') {
        let p: f32 = p.trim().parse().ok()?;
        return Some(parent_pt * p / 100.0);
    }
    length(&value, parent_pt).filter(|v| *v > 0.0)
}

/// The first family in a `font-family` list, without quotes; generic families are `None`.
pub fn font_family(value: &str) -> Option<String> {
    let first = split_outside_parens(value, ',').into_iter().next()?.trim();
    let name = first.trim_matches(|c| c == '"' || c == '\'').trim();
    let generic = [
        "serif",
        "sans-serif",
        "monospace",
        "cursive",
        "fantasy",
        "system-ui",
        "inherit",
        "initial",
    ];
    if name.is_empty() || generic.contains(&name.to_ascii_lowercase().as_str()) {
        return None;
    }
    Some(name.to_owned())
}

/// A `font-weight` as a number (`bold` is 700, `normal` 400).
pub fn font_weight(value: &str) -> Option<u16> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(400),
        "bold" => Some(700),
        "bolder" => Some(700),
        "lighter" => Some(300),
        other => other.parse::<u16>().ok().filter(|w| (1..=1000).contains(w)),
    }
}

/// A `@page` `size` value: two lengths, or a paper name with an optional orientation.
pub fn page_size(value: &str) -> Option<(f32, f32)> {
    let tokens = tokens(value);
    let mut named: Option<(f32, f32)> = None;
    let mut landscape = false;
    let mut lengths = Vec::new();
    for token in &tokens {
        match token.to_ascii_lowercase().as_str() {
            "a4" => named = Some((595.276, 841.89)),
            "a5" => named = Some((419.528, 595.276)),
            "letter" => named = Some((612.0, 792.0)),
            "legal" => named = Some((612.0, 1008.0)),
            "landscape" => landscape = true,
            "portrait" => {}
            other => lengths.push(length(other, 12.0)?),
        }
    }
    let (w, h) = match (named, lengths.as_slice()) {
        (Some(size), _) => size,
        (None, [w, h]) => (*w, *h),
        _ => return None,
    };
    Some(if landscape && w < h { (h, w) } else { (w, h) })
}

/// The one to four lengths of a box shorthand (`margin`, `padding`) as top, right, bottom,
/// left.
pub fn box_lengths(value: &str, font_pt: f32) -> Option<[f32; 4]> {
    let v: Vec<f32> = tokens(value)
        .into_iter()
        .map(|t| length(t, font_pt))
        .collect::<Option<_>>()?;
    Some(match v.as_slice() {
        [a] => [*a, *a, *a, *a],
        [a, b] => [*a, *b, *a, *b],
        [a, b, c] => [*a, *b, *c, *b],
        [a, b, c, d] => [*a, *b, *c, *d],
        _ => return None,
    })
}

/// A border shorthand (`0.75pt solid #000`): width and colour, or `None` for no line.
pub fn border(value: &str) -> Option<(f32, [u8; 4])> {
    let mut width = 1.0;
    let mut colour_value = [0, 0, 0, 255];
    let mut visible = false;
    for token in tokens(value) {
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "none" | "hidden" => return None,
            "solid" | "double" | "dotted" | "dashed" | "groove" | "ridge" | "inset" | "outset" => {
                visible = true
            }
            "thin" => width = 0.75,
            "medium" => width = 2.25,
            "thick" => width = 3.75,
            _ => {
                if let Some(w) = length(&lower, 12.0) {
                    width = w;
                } else if let Some(c) = colour(&lower) {
                    colour_value = c;
                }
            }
        }
    }
    (visible && width > 0.0).then_some((width, colour_value))
}

/// Formats a point value for CSS: shortest form that reads back as the same `f32`.
pub fn pt(value: f32) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{value}pt")
}

/// `#rrggbb` (alpha dropped).
pub fn hex(colour: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", colour[0], colour[1], colour[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_in_every_form() {
        assert_eq!(colour("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(colour("#00FF00"), Some([0, 255, 0, 255]));
        assert_eq!(colour("rgb(1, 2, 3)"), Some([1, 2, 3, 255]));
        assert_eq!(colour("rgba(1 2 3 / 0.5)"), Some([1, 2, 3, 128]));
        assert_eq!(colour("Grey"), Some([128, 128, 128, 255]));
        assert_eq!(colour("transparent"), None);
        assert_eq!(
            background_colour("url(x.png) #102030 no-repeat"),
            Some([16, 32, 48, 255])
        );
    }

    #[test]
    fn declarations_keep_commas_inside_functions() {
        let d = declarations("color: rgb(1, 2, 3); font-family: 'A; B', serif ; ");
        assert_eq!(d[0], ("color".into(), "rgb(1, 2, 3)".into()));
        assert_eq!(d[1].1, "'A; B', serif");
    }

    #[test]
    fn rules_for_body_skip_media_blocks_and_comments() {
        let sheet = "/* x */ body, p { background: #eee; } @media print { body { color: red } } html{color:blue}";
        assert_eq!(
            rules_for(sheet, "body"),
            vec![("background".into(), "#eee".into())]
        );
        assert_eq!(
            rules_for(sheet, "html"),
            vec![("color".into(), "blue".into())]
        );
    }

    #[test]
    fn lengths_and_sizes() {
        assert_eq!(length("12px", 12.0), Some(9.0));
        assert_eq!(length("2em", 11.0), Some(22.0));
        assert_eq!(font_size("150%", 10.0), Some(15.0));
        assert_eq!(page_size("A4 landscape").map(|s| s.0.round()), Some(842.0));
        assert_eq!(border("1pt solid red"), Some((1.0, [255, 0, 0, 255])));
        assert_eq!(border("none"), None);
        assert_eq!(pt(13.2), "13.2pt");
    }
}
