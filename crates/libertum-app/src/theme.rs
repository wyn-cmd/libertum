//! Colour tokens for the Pages-style look, light and dark.
//!
//! Every colour the UI uses comes from [`Tokens`]; view code never writes a hex value. The
//! tokens are `Copy`, so view functions take them by value and style closures capture them.
//! [`app_theme`] also builds an Iced `Theme` from the same colours so the widgets we do not
//! restyle (pick list menus, scrollbars) pick up the accent and the dark background.

use iced::theme::Palette;
use iced::{Color, Theme};

use crate::settings::ThemePreference;

/// Light or dark, after resolving "follow the system".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
}

impl Scheme {
    /// `system_dark` is `None` when the system theme is unknown (defaults to light).
    pub fn resolve(preference: ThemePreference, system_dark: Option<bool>) -> Self {
        match preference {
            ThemePreference::Light => Self::Light,
            ThemePreference::Dark => Self::Dark,
            ThemePreference::System => {
                if system_dark == Some(true) {
                    Self::Dark
                } else {
                    Self::Light
                }
            }
        }
    }
}

const fn rgb(hex: u32) -> Color {
    Color::from_rgb(
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    )
}

const fn rgba(hex: u32, alpha: f32) -> Color {
    let c = rgb(hex);
    Color { a: alpha, ..c }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    /// Toolbar and title bar.
    pub chrome: Color,
    /// Hairline under the toolbar and left of the inspector.
    pub chrome_border: Color,
    /// Area behind the pages.
    pub canvas: Color,
    /// Inspector background (slightly lighter than the chrome).
    pub inspector: Color,
    /// Hairline dividers between inspector sections.
    pub divider: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_disabled: Color,
    /// The highlight colour (selection, active controls, focus). Coral orange, so the neutral
    /// grey chrome stays grey; nothing else in the palette leans warm.
    pub accent: Color,
    /// Text and icons drawn on the accent colour.
    pub on_accent: Color,
    /// Flat button hover and pressed backgrounds.
    pub hover: Color,
    pub pressed: Color,
    /// Popup buttons, text fields.
    pub control: Color,
    pub control_hover: Color,
    pub control_border: Color,
    /// Segmented control track and the raised selected segment.
    pub segment_track: Color,
    pub segment_selected: Color,
    pub menu: Color,
    pub menu_border: Color,
    pub menu_hover: Color,
    pub shadow: Color,
    /// Word-count pill.
    pub pill: Color,
    pub pill_text: Color,
    /// Paper of the template thumbnails (white in both themes, like the pages).
    pub paper: Color,
    /// Thumbnail "text" lines on the paper: body and headings.
    pub paper_ink: Color,
    pub paper_ink_strong: Color,
}

impl Tokens {
    pub const LIGHT: Self = Self {
        dark: false,
        chrome: rgb(0xECECEC),
        chrome_border: rgb(0xD0D0D0),
        canvas: rgb(0xE3E3E3),
        inspector: rgb(0xF4F4F4),
        divider: rgb(0xDCDCDC),
        text: rgb(0x1D1D1F),
        text_secondary: rgb(0x6C6C70),
        text_disabled: rgb(0xB4B4B8),
        accent: rgb(0xD9532F),
        on_accent: rgb(0xFFFFFF),
        hover: rgba(0x000000, 0.06),
        pressed: rgba(0x000000, 0.12),
        control: rgb(0xFFFFFF),
        control_hover: rgb(0xFAFAFA),
        control_border: rgb(0xD1D1D6),
        segment_track: rgb(0xE4E4E6),
        segment_selected: rgb(0xFFFFFF),
        menu: rgb(0xF7F7F7),
        menu_border: rgb(0xD4D4D4),
        menu_hover: rgb(0xD9532F),
        shadow: rgba(0x000000, 0.22),
        pill: rgba(0x4A4A4C, 0.82),
        pill_text: rgb(0xFFFFFF),
        paper: rgb(0xFFFFFF),
        paper_ink: rgb(0xD1D1D6),
        paper_ink_strong: rgb(0x8E8E93),
    };

    pub const DARK: Self = Self {
        dark: true,
        chrome: rgb(0x1E1E1E),
        chrome_border: rgb(0x0F0F0F),
        canvas: rgb(0x111111),
        inspector: rgb(0x232323),
        divider: rgb(0x353535),
        text: rgb(0xF2F2F7),
        text_secondary: rgb(0x9E9EA6),
        text_disabled: rgb(0x5C5C60),
        accent: rgb(0xE9663F),
        on_accent: rgb(0xFFFFFF),
        hover: rgba(0xFFFFFF, 0.08),
        pressed: rgba(0xFFFFFF, 0.15),
        control: rgb(0x2D2D2F),
        control_hover: rgb(0x37373A),
        control_border: rgb(0x454548),
        segment_track: rgb(0x2D2D2F),
        segment_selected: rgb(0x4F4F54),
        menu: rgb(0x29292B),
        menu_border: rgb(0x3F3F42),
        menu_hover: rgb(0xE9663F),
        shadow: rgba(0x000000, 0.6),
        // The pill floats over white pages in both themes, so it stays dark here too.
        pill: rgba(0x3A3A3C, 0.88),
        pill_text: rgb(0xFFFFFF),
        paper: rgb(0xFFFFFF),
        paper_ink: rgb(0xD1D1D6),
        paper_ink_strong: rgb(0x8E8E93),
    };

    pub fn of(scheme: Scheme) -> Self {
        match scheme {
            Scheme::Light => Self::LIGHT,
            Scheme::Dark => Self::DARK,
        }
    }
}

/// An Iced theme built from the tokens, for the widgets we do not style by hand.
pub fn app_theme(scheme: Scheme) -> Theme {
    let t = Tokens::of(scheme);
    let (name, success, warning, danger) = match scheme {
        Scheme::Light => (
            "Libertum Light",
            rgb(0x34C759),
            rgb(0xFF9500),
            rgb(0xFF3B30),
        ),
        Scheme::Dark => ("Libertum Dark", rgb(0x30D158), rgb(0xFF9F0A), rgb(0xFF453A)),
    };
    Theme::custom(
        name,
        Palette {
            background: t.inspector,
            text: t.text,
            primary: t.accent,
            success,
            warning,
            danger,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_system_theme_is_light() {
        assert_eq!(
            Scheme::resolve(ThemePreference::System, None),
            Scheme::Light
        );
        assert_eq!(
            Scheme::resolve(ThemePreference::System, Some(true)),
            Scheme::Dark
        );
        assert_eq!(
            Scheme::resolve(ThemePreference::Light, Some(true)),
            Scheme::Light
        );
        assert_eq!(
            Scheme::resolve(ThemePreference::Dark, Some(false)),
            Scheme::Dark
        );
    }

    #[test]
    fn dark_theme_is_dark() {
        assert!(app_theme(Scheme::Dark).extended_palette().is_dark);
        assert!(!app_theme(Scheme::Light).extended_palette().is_dark);
    }

    #[test]
    fn accent_is_coral_orange_in_both_schemes() {
        for t in [Tokens::LIGHT, Tokens::DARK] {
            assert!(t.accent.r > t.accent.g, "the accent leans orange");
            assert!(t.accent.r > t.accent.b, "the accent is not blue");
            assert!(t.accent.b < 0.5, "the accent is warm, not a tinted grey");
        }
    }

    #[test]
    fn inspector_is_lighter_than_chrome_in_both_schemes() {
        for t in [Tokens::LIGHT, Tokens::DARK] {
            assert!(t.inspector.r >= t.chrome.r);
        }
    }
}
