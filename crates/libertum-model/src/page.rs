//! Page setup: paper size, orientation and margins (PLAN §4.1 `Section`).
//!
//! Kept in the model, in points, so a document carries its own page layout and the format
//! crates can map it without depending on the layout engine (DOCX `w:pgSz`/`w:pgMar`, ODT
//! `style:page-layout`). `libertum-layout` converts this to its own [`PageGeometry`].
//!
//! [`PageGeometry`]: https://docs.rs/libertum-layout

/// Paper size, portrait, in points (1 pt = 1/72 in).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum PaperSize {
    /// ISO A4, 210 x 297 mm.
    #[default]
    A4,
    /// US Letter, 8.5 x 11 in.
    Letter,
    /// ISO A5, 148 x 210 mm.
    A5,
    /// US Legal, 8.5 x 14 in.
    Legal,
}

impl PaperSize {
    pub const ALL: [PaperSize; 4] = [
        PaperSize::A4,
        PaperSize::Letter,
        PaperSize::A5,
        PaperSize::Legal,
    ];

    /// Portrait size in millimetres (width, height).
    pub fn millimetres(self) -> (f32, f32) {
        match self {
            PaperSize::A4 => (210.0, 297.0),
            PaperSize::Letter => (215.9, 279.4),
            PaperSize::A5 => (148.0, 210.0),
            PaperSize::Legal => (215.9, 355.6),
        }
    }

    /// Portrait size in points (width, height).
    pub fn points(self) -> (f32, f32) {
        match self {
            PaperSize::A4 => (595.276, 841.89),
            PaperSize::Letter => (612.0, 792.0),
            PaperSize::A5 => (419.528, 595.276),
            PaperSize::Legal => (612.0, 1008.0),
        }
    }
}

/// Which way round the paper is used.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Orientation {
    #[default]
    Portrait,
    Landscape,
}

/// Page margins in points.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Margins {
    pub const fn uniform(pt: f32) -> Self {
        Self {
            top: pt,
            right: pt,
            bottom: pt,
            left: pt,
        }
    }
}

impl Default for Margins {
    /// One inch, as Word's default and `PageGeometry::a4()`.
    fn default() -> Self {
        Self::uniform(72.0)
    }
}

/// Everything about the page itself: paper, orientation and margins.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PageSetup {
    pub paper: PaperSize,
    pub orientation: Orientation,
    pub margins: Margins,
}

impl PageSetup {
    /// ISO A4 with 1-inch margins.
    pub const fn a4() -> Self {
        Self {
            paper: PaperSize::A4,
            orientation: Orientation::Portrait,
            margins: Margins::uniform(72.0),
        }
    }

    /// US Letter with 1-inch margins.
    pub const fn letter() -> Self {
        Self {
            paper: PaperSize::Letter,
            ..Self::a4()
        }
    }

    /// ISO A5 with 1-inch margins.
    pub const fn a5() -> Self {
        Self {
            paper: PaperSize::A5,
            ..Self::a4()
        }
    }

    /// Page width after applying the orientation.
    pub fn width_pt(self) -> f32 {
        let (w, h) = self.paper.points();
        match self.orientation {
            Orientation::Portrait => w,
            Orientation::Landscape => h,
        }
    }

    /// Page height after applying the orientation.
    pub fn height_pt(self) -> f32 {
        let (w, h) = self.paper.points();
        match self.orientation {
            Orientation::Portrait => h,
            Orientation::Landscape => w,
        }
    }

    /// Width available to text, in points. Never below 1 pt, so a wild margin cannot make the
    /// layout engine divide by zero.
    pub fn content_width_pt(self) -> f32 {
        (self.width_pt() - self.margins.left - self.margins.right).max(1.0)
    }

    /// Height available to text, in points.
    pub fn content_height_pt(self) -> f32 {
        (self.height_pt() - self.margins.top - self.margins.bottom).max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landscape_swaps_the_page_dimensions() {
        let setup = PageSetup {
            orientation: Orientation::Landscape,
            ..PageSetup::a4()
        };
        assert!((setup.width_pt() - 841.89).abs() < 0.01);
        assert!((setup.height_pt() - 595.276).abs() < 0.01);
        assert!((setup.content_width_pt() - (841.89 - 144.0)).abs() < 0.01);
    }

    #[test]
    fn content_area_survives_margins_wider_than_the_page() {
        let setup = PageSetup {
            margins: Margins::uniform(500.0),
            ..PageSetup::a5()
        };
        assert_eq!(setup.content_width_pt(), 1.0);
        assert_eq!(setup.content_height_pt(), 1.0);
    }

    #[test]
    fn paper_millimetres_and_points_agree() {
        for paper in PaperSize::ALL {
            let (w_mm, h_mm) = paper.millimetres();
            let (w_pt, h_pt) = paper.points();
            assert!((w_mm * 72.0 / 25.4 - w_pt).abs() < 1.0, "{paper:?} width");
            assert!((h_mm * 72.0 / 25.4 - h_pt).abs() < 1.0, "{paper:?} height");
        }
    }
}
