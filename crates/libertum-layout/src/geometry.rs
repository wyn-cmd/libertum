use libertum_model::PageSetup;

/// Page margins in points.
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// Physical page size and margins, in points (1 pt = 1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    pub width_pt: f32,
    pub height_pt: f32,
    pub margins: Margins,
}

impl PageGeometry {
    /// ISO A4 (210 x 297 mm) with 1-inch margins.
    pub const fn a4() -> Self {
        Self {
            width_pt: 595.276,
            height_pt: 841.89,
            margins: Margins::uniform(72.0),
        }
    }

    /// US Letter (8.5 x 11 in) with 1-inch margins.
    pub const fn letter() -> Self {
        Self {
            width_pt: 612.0,
            height_pt: 792.0,
            margins: Margins::uniform(72.0),
        }
    }

    /// The layout geometry for a document's page setup (both are in points).
    pub fn from_setup(setup: &PageSetup) -> Self {
        Self {
            width_pt: setup.width_pt(),
            height_pt: setup.height_pt(),
            margins: Margins {
                top: setup.margins.top,
                right: setup.margins.right,
                bottom: setup.margins.bottom,
                left: setup.margins.left,
            },
        }
    }

    /// Width available to text, in points.
    pub fn content_width(&self) -> f32 {
        (self.width_pt - self.margins.left - self.margins.right).max(1.0)
    }

    /// Height available to text, in points.
    pub fn content_height(&self) -> f32 {
        (self.height_pt - self.margins.top - self.margins.bottom).max(1.0)
    }
}

impl Default for PageGeometry {
    fn default() -> Self {
        Self::a4()
    }
}
