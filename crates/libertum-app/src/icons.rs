//! Bundled Lucide icons (ISC licence, see `assets/icons/LICENSE`), drawn with Iced's `svg`
//! widget and recoloured to the theme. Apple SF Symbols are deliberately not used: their
//! licence limits them to Apple platforms.

use std::sync::OnceLock;

use iced::widget::svg::{self, Handle, Svg};
use iced::{Color, Length};

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon { $($variant),+ }

        impl Icon {
            const ALL: &'static [Self] = &[$(Self::$variant),+];

            fn bytes(self) -> &'static [u8] {
                match self {
                    $(Self::$variant => include_bytes!(concat!("../assets/icons/", $file)).as_slice()),+
                }
            }
        }
    };
}

icons! {
    PanelRight => "panel-right.svg",
    FilePlus => "file-plus.svg",
    CirclePlus => "circle-plus.svg",
    Table => "table.svg",
    Image => "image.svg",
    Asterisk => "asterisk.svg",
    TextBox => "text-cursor-input.svg",
    Comment => "message-square.svg",
    Paintbrush => "paintbrush.svg",
    Document => "file-text.svg",
    List => "list.svg",
    AlignLeft => "align-left.svg",
    AlignCentre => "align-center.svg",
    AlignRight => "align-right.svg",
    AlignJustify => "align-justify.svg",
    IndentIncrease => "indent-increase.svg",
    IndentDecrease => "indent-decrease.svg",
    Bold => "bold.svg",
    Italic => "italic.svg",
    Underline => "underline.svg",
    Strikethrough => "strikethrough.svg",
    Settings => "settings.svg",
    ChevronDown => "chevron-down.svg",
    ChevronUp => "chevron-up.svg",
    Minus => "minus.svg",
    Plus => "plus.svg",
    Portrait => "rectangle-vertical.svg",
    Landscape => "rectangle-horizontal.svg",
}

/// One handle per icon, created once so Iced's SVG cache sees a stable id.
fn handle(icon: Icon) -> Handle {
    static HANDLES: OnceLock<Vec<Handle>> = OnceLock::new();
    HANDLES
        .get_or_init(|| {
            Icon::ALL
                .iter()
                .map(|i| Handle::from_memory(i.bytes()))
                .collect()
        })
        .get(icon as usize)
        .cloned()
        .unwrap_or_else(|| Handle::from_memory(icon.bytes()))
}

/// A square icon of `size` logical pixels in `colour`.
pub fn icon<'a>(icon: Icon, size: f32, colour: Color) -> Svg<'a> {
    svg::Svg::new(handle(icon))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_, _| svg::Style {
            color: Some(colour),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_an_svg() {
        for icon in Icon::ALL {
            let text = std::str::from_utf8(icon.bytes()).expect("utf-8");
            assert!(text.contains("<svg"), "{icon:?}");
        }
    }

    #[test]
    fn handles_are_indexed_by_discriminant() {
        for (i, icon) in Icon::ALL.iter().enumerate() {
            assert_eq!(*icon as usize, i);
        }
    }
}
