//! Headers and footers (PLAN §4.1 `headers_footers`).
//!
//! Headers and footers are per section and come in three variants, as in Word and ODT: one
//! for every page, one for the section's first page and one for even pages. A section may
//! switch the first-page and even-page variants on independently; when one is switched on but
//! has no text of its own, the default variant stands in, so switching it on never blanks a
//! page.
//!
//! The content is one paragraph of plain text. A `{page}` token is replaced by the page's
//! number and `{pages}` by the document's page count when the page is laid out (DOCX `PAGE`
//! and `NUMPAGES` fields, ODT `text:page-number` / `text:page-continuation`). Rich formatting
//! of header text is left to the format importers: they can set [`HeaderFooter::align`] and a
//! later revision can add runs without changing this shape.

use crate::paragraph::Align;

/// Which page of a section a header or footer belongs to.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum HfVariant {
    /// Every page that does not use one of the other variants.
    #[default]
    Default,
    /// The first page of the section.
    First,
    /// Even pages (page 2, 4, 6... counting from the start of the document).
    Even,
}

impl HfVariant {
    /// All three variants, in the order the inspector shows them.
    pub const ALL: [Self; 3] = [Self::Default, Self::First, Self::Even];
}

/// Whether a [`HeadersFooters`] set is the page's header or its footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum HfKind {
    Header,
    Footer,
}

/// One header or footer: a single paragraph of text and its alignment.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HeaderFooter {
    /// The text. `{page}` and `{pages}` are fields (see the module documentation).
    pub text: String,
    /// Alignment across the text area (left, centre, right, justify).
    pub align: Align,
}

impl HeaderFooter {
    /// A left-aligned header or footer with `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            align: Align::Start,
        }
    }

    /// The text with this page's fields filled in. `{pages}` is replaced first, so a page
    /// count is never mistaken for a page number.
    pub fn resolve(&self, page_number: u32, total_pages: u32) -> String {
        let mut text = self.text.clone();
        if text.contains("{pages}") {
            text = text.replace("{pages}", &total_pages.to_string());
        }
        if text.contains("{page}") {
            text = text.replace("{page}", &page_number.to_string());
        }
        text
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

/// The three variants of a section's header or footer, and whether the first and even
/// variants are in use.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HeadersFooters {
    /// Every page; `None` means the section has no header (or footer) at all.
    pub default: Option<HeaderFooter>,
    /// The section's first page, shown when [`different_first`](Self::different_first).
    pub first: Option<HeaderFooter>,
    /// Even pages, shown when [`different_even_odd`](Self::different_even_odd).
    pub even: Option<HeaderFooter>,
    /// The first page of the section uses [`first`](Self::first) (DOCX `w:titlePg`).
    pub different_first: bool,
    /// Even pages use [`even`](Self::even) (DOCX `w:evenAndOddHeaders`).
    pub different_even_odd: bool,
}

impl HeadersFooters {
    /// A section with a default header (or footer) and the other variants off.
    pub fn with_default(text: impl Into<String>) -> Self {
        Self {
            default: Some(HeaderFooter::new(text)),
            ..Self::default()
        }
    }

    /// True when the section shows a header or footer on ordinary pages.
    pub fn enabled(&self) -> bool {
        self.default.is_some()
    }

    pub fn get(&self, variant: HfVariant) -> Option<&HeaderFooter> {
        match variant {
            HfVariant::Default => self.default.as_ref(),
            HfVariant::First => self.first.as_ref(),
            HfVariant::Even => self.even.as_ref(),
        }
    }

    pub fn get_mut(&mut self, variant: HfVariant) -> &mut Option<HeaderFooter> {
        match variant {
            HfVariant::Default => &mut self.default,
            HfVariant::First => &mut self.first,
            HfVariant::Even => &mut self.even,
        }
    }

    /// The header (or footer) for a physical page of a section, with the variant used.
    ///
    /// `page_index` is zero-based over the whole document, so page index 1 is printed page 2
    /// and gets the even-page variant. A variant that is switched on but has no text falls
    /// back to the default one; a section with no default has no header or footer at all.
    pub fn for_page(
        &self,
        page_index: usize,
        first_in_section: bool,
    ) -> Option<(HfVariant, &HeaderFooter)> {
        let default = self.default.as_ref()?;
        if self.different_first
            && first_in_section
            && let Some(first) = self.first.as_ref()
        {
            return Some((HfVariant::First, first));
        }
        if self.different_even_odd
            && page_index % 2 == 1
            && let Some(even) = self.even.as_ref()
        {
            return Some((HfVariant::Even, even));
        }
        Some((HfVariant::Default, default))
    }

    /// Every variant that carries text, for layout measurement and content hashing.
    pub fn non_empty(&self) -> impl Iterator<Item = (HfVariant, &HeaderFooter)> {
        HfVariant::ALL
            .into_iter()
            .filter_map(|variant| self.get(variant).map(|hf| (variant, hf)))
            .filter(|(_, hf)| !hf.is_empty())
    }
}
