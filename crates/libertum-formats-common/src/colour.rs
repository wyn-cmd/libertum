//! Straight RGBA colours in the `#RRGGBB` form both formats use.
//!
//! The model stores `[u8; 4]` (straight RGBA). ODF and WordprocessingML store six hex digits
//! with an alpha of 255; ODF also accepts `transparent`.

/// Parses `#RRGGBB` (the `#` is optional). Returns `None` for `auto`, `transparent`, a named
/// colour or malformed input.
pub fn parse(value: &str) -> Option<[u8; 4]> {
    let value = value.trim();
    let digits = value.strip_prefix('#').unwrap_or(value);
    if digits.len() != 6 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&digits[0..2], 16).ok()?;
    let g = u8::from_str_radix(&digits[2..4], 16).ok()?;
    let b = u8::from_str_radix(&digits[4..6], 16).ok()?;
    Some([r, g, b, 255])
}

/// `#RRGGBB`, discarding alpha (both formats have no per-run opacity here).
pub fn hex([r, g, b, _]: [u8; 4]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// True for a colour value that asks for "whatever the background is" and must be treated as
/// unset rather than as black.
pub fn is_auto(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "auto" | "transparent" | "none"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_hash() {
        assert_eq!(parse("#FF8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse("00ff00"), Some([0, 255, 0, 255]));
        assert_eq!(parse("#FFF"), None);
        assert_eq!(parse("transparent"), None);
        assert_eq!(parse("auto"), None);
        assert_eq!(parse("#GG0000"), None);
    }

    #[test]
    fn writes_uppercase_hex() {
        assert_eq!(hex([0, 128, 255, 255]), "#0080FF");
        assert_eq!(hex([1, 2, 3, 0]), "#010203");
    }

    #[test]
    fn auto_values() {
        assert!(is_auto("auto") && is_auto(" none ") && is_auto("Transparent"));
        assert!(!is_auto("#000000"));
    }
}
