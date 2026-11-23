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

    /// The geometry vertical text (set from the right, lines running down) is laid out in. The
    /// page is turned a quarter turn clockwise: text then runs left to right along what is
    /// the page's height, and lines stack downwards along what is its width, and the engine
    /// lays it out and paginates it exactly as horizontal text. A point (x, y) of this layout
    /// page is the point (width - y, x) of the real one, so the real page's right margin is
    /// the layout's top, its bottom margin the layout's right, its left the layout's bottom
    /// and its top the layout's left.
    pub fn transposed(&self) -> Self {
        Self {
            width_pt: self.height_pt,
            height_pt: self.width_pt,
            margins: Margins {
                top: self.margins.right,
                right: self.margins.bottom,
                bottom: self.margins.left,
                left: self.margins.top,
            },
        }
    }

    /// The real page of a layout page made by [`transposed`](Self::transposed).
    pub fn untransposed(&self) -> Self {
        Self {
            width_pt: self.height_pt,
            height_pt: self.width_pt,
            margins: Margins {
                top: self.margins.left,
                right: self.margins.top,
                bottom: self.margins.right,
                left: self.margins.bottom,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> PageGeometry {
        PageGeometry {
            width_pt: 400.0,
            height_pt: 600.0,
            margins: Margins {
                top: 10.0,
                right: 20.0,
                bottom: 30.0,
                left: 40.0,
            },
        }
    }

    #[test]
    fn a_page_turned_for_vertical_text_swaps_its_sides_and_turns_back() {
        let layout = page().transposed();
        assert_eq!((layout.width_pt, layout.height_pt), (600.0, 400.0));
        // The real right margin is the layout's top, bottom is right, left is bottom, top is left.
        assert_eq!(layout.margins.top, 20.0);
        assert_eq!(layout.margins.right, 30.0);
        assert_eq!(layout.margins.bottom, 40.0);
        assert_eq!(layout.margins.left, 10.0);
        assert_eq!(layout.untransposed(), page());
        // The text area is the same rectangle, seen from the other way round.
        assert_eq!(layout.content_width(), page().content_height());
        assert_eq!(layout.content_height(), page().content_width());
    }
}
