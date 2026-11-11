//! Lengths as the formats write them. The model stores everything in points.
//!
//! ODF (and CSS) lengths carry a unit suffix: `12pt`, `2.5cm`, `1in`. DOCX uses twips, which
//! its importer converts itself. [`parse`] is lenient about whitespace and accepts a bare
//! number as points; [`format`] always writes points so a round trip is exact.

/// Points per inch, as both formats define it.
pub const PT_PER_IN: f32 = 72.0;
/// Points per centimetre.
pub const PT_PER_CM: f32 = PT_PER_IN / 2.54;
/// Points per millimetre.
pub const PT_PER_MM: f32 = PT_PER_IN / 25.4;
/// Points per pica (1/6 in).
pub const PT_PER_PC: f32 = 12.0;
/// Points per CSS pixel (96 dpi).
pub const PT_PER_PX: f32 = PT_PER_IN / 96.0;

/// Parses an ODF/CSS length into points. Returns `None` for an empty or malformed value, or
/// for a percentage (callers that allow percentages handle them first).
pub fn parse(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.is_empty() || value.ends_with('%') {
        return None;
    }
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | '-' | '+')))
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f32 = number.trim().parse().ok()?;
    if !number.is_finite() {
        return None;
    }
    let factor = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "pt" => 1.0,
        "cm" => PT_PER_CM,
        "mm" => PT_PER_MM,
        "in" => PT_PER_IN,
        "pc" => PT_PER_PC,
        "px" => PT_PER_PX,
        other => return unknown_unit(other),
    };
    Some(number * factor)
}

/// A unit this crate does not know is a parse failure rather than a silent point value.
fn unknown_unit(_unit: &str) -> Option<f32> {
    None
}

/// Parses a percentage (`160%` or `1.6`) as a factor (`1.6`). Used for relative font sizes
/// and line heights.
pub fn parse_percent(value: &str) -> Option<f32> {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| v / 100.0);
    }
    value.parse::<f32>().ok().filter(|v| v.is_finite())
}

/// Formats points for an attribute. Whole points stay short; other values use the shortest
/// decimal form that parses back to the same `f32`, so a round trip never drifts.
pub fn format(points: f32) -> String {
    if points.is_finite() && points.fract() == 0.0 && points.abs() < 1.0e9 {
        return format!("{}pt", points as i64);
    }
    format!("{points}pt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_supported_unit() {
        assert_eq!(parse("72pt"), Some(72.0));
        assert_eq!(parse("1in"), Some(72.0));
        assert!((parse("2.54cm").unwrap() - 72.0).abs() < 0.01);
        assert!((parse("25.4mm").unwrap() - 72.0).abs() < 0.01);
        assert_eq!(parse("6pc"), Some(72.0));
        assert_eq!(parse("96px"), Some(72.0));
        assert_eq!(parse(" 36 pt "), Some(36.0));
        assert_eq!(parse("36"), Some(36.0), "bare numbers are points");
        assert_eq!(parse("160%"), None, "percentages are not lengths");
        assert_eq!(parse("bad"), None);
        assert_eq!(parse("12zz"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn percent_parsing() {
        assert_eq!(parse_percent("160%"), Some(1.6));
        assert_eq!(parse_percent("1.5"), Some(1.5));
        assert_eq!(parse_percent("x"), None);
    }

    #[test]
    fn formatting_round_trips_through_parse() {
        for value in [0.0, 1.0, 12.0, 35.5, 28.800_001, 595.276, 841.89] {
            let text = format(value);
            let back = parse(&text).expect("parse own output");
            assert_eq!(back, value, "{value} -> {text} -> {back}");
        }
        assert_eq!(format(36.0), "36pt");
        assert_eq!(format(35.5), "35.5pt");
    }
}
