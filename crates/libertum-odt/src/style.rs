//! ODF styles: common styles (`styles.xml`), automatic styles (`content.xml`), list styles,
//! page layouts and master pages.
//!
//! ODF keeps formatting in named style families and resolves it through
//! `style:parent-style-name` chains, like WordprocessingML's `w:basedOn`. Libertum maps
//! paragraph styles to the model's style sheet; automatic styles (per-paragraph and per-run
//! formatting) become direct formatting, because the model keeps "set here" apart from
//! "inherited from the style" in exactly that way. Text (character) styles have no model
//! counterpart, so a run using one gets the style's properties as direct formatting.

use std::collections::HashMap;
use std::sync::Arc;

use libertum_formats_common::{colour, units};
use libertum_model::{
    Align, BorderSides, Borders, CharFormat, LineSpacing, ListStyle, ParagraphStyle, ResolvedChar,
    Section, StyleId, StyleSheet, TabStop, TabStopKind, TabStops, TableBorders, VerticalAlign,
    Watermark,
};
use quick_xml::Reader;

use crate::OdtError;
use crate::xml::{self, XmlContent, XmlNode};

pub(crate) const PART_STYLES: &str = "styles.xml";
pub(crate) const PART_CONTENT: &str = "content.xml";

/// Deepest `style:parent-style-name` chain followed, the same bound the model's style sheet
/// uses.
const MAX_STYLE_DEPTH: usize = 16;

/// A border as an ODF style writes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BorderLine {
    pub width_pt: f32,
    pub colour: [u8; 4],
    pub visible: bool,
}

/// A character property a style sets; a size may be relative to the parent style.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawChar {
    family: Option<Arc<str>>,
    size: Option<Size>,
    weight: Option<u16>,
    italic: Option<bool>,
    underline: Option<bool>,
    strikethrough: Option<bool>,
    colour: Option<[u8; 4]>,
    /// `Some(None)` is an explicit transparent background, which removes an inherited one.
    highlight: Option<Option<[u8; 4]>>,
    vertical: Option<VerticalAlign>,
    letter_spacing_pt: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
enum Size {
    Points(f32),
    Factor(f32),
}

impl RawChar {
    /// The partial formatting this style adds over `base`, with relative sizes resolved.
    pub fn format(&self, base: &ResolvedChar) -> CharFormat {
        CharFormat {
            family: self.family.clone(),
            size_pt: self.size.map(|size| match size {
                Size::Points(points) => points,
                Size::Factor(factor) => base.size_pt * factor,
            }),
            weight: self.weight,
            italic: self.italic,
            underline: self.underline,
            strikethrough: self.strikethrough,
            colour: self.colour,
            highlight: self.highlight,
            vertical: self.vertical,
            letter_spacing_pt: self.letter_spacing_pt,
            ligatures: None,
        }
    }
}

/// One style definition from either part.
#[derive(Clone, Debug, Default)]
pub(crate) struct RawStyle {
    pub name: String,
    pub family: String,
    pub display_name: Option<String>,
    pub parent: Option<String>,
    pub next: Option<String>,
    pub char: RawChar,
    pub para: libertum_model::ParaFormat,
    pub outline: Option<u8>,
    pub master_page: Option<String>,
    pub page_break_before: bool,
    pub automatic: bool,
    /// `style:table-properties`, kept for table import.
    pub table: Option<XmlNode>,
    /// `style:table-cell-properties`, kept for table import.
    pub cell: Option<XmlNode>,
    /// `style:table-column-properties`, kept for table import.
    pub column: Option<XmlNode>,
    /// `style:table-row-properties`, kept for table import.
    pub row: Option<XmlNode>,
}

impl RawStyle {
    fn display(&self) -> String {
        match &self.display_name {
            Some(name) if !name.is_empty() => name.clone(),
            _ => decode_name(&self.name),
        }
    }
}

/// A style's effect on one paragraph.
#[derive(Clone, Debug, Default)]
pub(crate) struct ParagraphRef {
    pub style: StyleId,
    pub format: libertum_model::ParaFormat,
    pub char: CharFormat,
    pub page_break_before: bool,
    pub master_page: Option<String>,
    pub outline: Option<u8>,
}

/// A page's size and margins, from `style:page-layout-properties`.
#[derive(Clone, Debug)]
pub(crate) struct PageLayout {
    pub page: libertum_model::PageSetup,
    pub first_page_number: Option<u32>,
    pub columns: u8,
    pub column_gutter_pt: f32,
    pub header_distance_pt: f32,
    pub footer_distance_pt: f32,
    /// `fo:background-color` of the page (the page colour).
    pub background: Option<[u8; 4]>,
}

impl Default for PageLayout {
    fn default() -> Self {
        Self {
            page: libertum_model::PageSetup::a4(),
            first_page_number: None,
            columns: 1,
            column_gutter_pt: libertum_model::DEFAULT_COLUMN_GUTTER_PT,
            header_distance_pt: libertum_model::DEFAULT_HEADER_FOOTER_DISTANCE_PT,
            footer_distance_pt: libertum_model::DEFAULT_HEADER_FOOTER_DISTANCE_PT,
            background: None,
        }
    }
}

/// A `style:master-page` plus the raw header and footer blocks read.rs parses into text.
#[derive(Clone, Debug, Default)]
pub(crate) struct MasterPage {
    pub layout: String,
    pub header: Option<XmlNode>,
    pub header_first: Option<XmlNode>,
    pub header_left: Option<XmlNode>,
    pub footer: Option<XmlNode>,
    pub footer_first: Option<XmlNode>,
    pub footer_left: Option<XmlNode>,
}

/// Marks a shape as a watermark, as LibreOffice names it (and Word, in DOCX).
pub(crate) const WATERMARK_MARKER: &str = "PowerPlusWaterMarkObject";

/// The master page with any watermark shape removed from its headers, and the watermark found
/// there (its text, and whether it is turned).
pub(crate) fn split_watermark(master: &MasterPage) -> (MasterPage, Option<Watermark>) {
    let mut master = master.clone();
    let mut found = None;
    for block in [
        &mut master.header,
        &mut master.header_first,
        &mut master.header_left,
    ]
    .into_iter()
    .flatten()
    {
        if let Some(watermark) = take_watermark(block) {
            found.get_or_insert(watermark);
        }
    }
    (master, found)
}

fn take_watermark(node: &mut XmlNode) -> Option<Watermark> {
    let mut found = None;
    let mut index = 0;
    while index < node.content.len() {
        let is_shape = matches!(
            &node.content[index],
            XmlContent::Element(child)
                if child.name == "custom-shape"
                    && child.attr("name").is_some_and(|name| name.contains(WATERMARK_MARKER))
        );
        if is_shape {
            if let XmlContent::Element(shape) = node.content.remove(index)
                && found.is_none()
            {
                found = shape_watermark(&shape);
            }
            continue;
        }
        if let XmlContent::Element(child) = &mut node.content[index]
            && let Some(inner) = take_watermark(child)
        {
            found.get_or_insert(inner);
        }
        index += 1;
    }
    found
}

/// All the text under a node, spans included.
fn deep_text(node: &XmlNode, out: &mut String) {
    for content in &node.content {
        match content {
            XmlContent::Text(text) => out.push_str(text),
            XmlContent::Element(child) => deep_text(child, out),
        }
    }
}

fn shape_watermark(shape: &XmlNode) -> Option<Watermark> {
    let mut text = String::new();
    for paragraph in shape.elements().filter(|child| child.name == "p") {
        deep_text(paragraph, &mut text);
        text.push(' ');
    }
    let turned = shape
        .attr("transform")
        .and_then(|transform| {
            let start = transform.find("rotate")? + "rotate".len();
            let number: String = transform[start..]
                .trim_start_matches([' ', '('])
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E'))
                .collect();
            number.parse::<f32>().ok()
        })
        .is_some_and(|radians| radians.abs() > 0.01);
    Watermark::new(&text, turned)
}

/// A list style's levels, reduced to what the model's four built-in lists need.
#[derive(Clone, Debug)]
pub(crate) struct ListStyleDef {
    /// Per level: true for a bullet, false for a number, plus the numeric format.
    pub levels: [Option<ListLevelDef>; 9],
}

#[derive(Clone, Debug)]
pub(crate) struct ListLevelDef {
    pub bullet: bool,
    pub format: Option<String>,
}

/// Every style in the document, and the model style sheet built from them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Styles {
    pub sheet: StyleSheet,
    /// ODT style name to model paragraph style, for the common paragraph styles.
    pub ids: HashMap<String, StyleId>,
    /// Every style definition by ODT name, common and automatic.
    pub raw: HashMap<String, RawStyle>,
    /// Common paragraph style names in file order (the model's style ids follow it).
    pub order: Vec<String>,
    pub list_styles: HashMap<String, ListStyleDef>,
    pub layouts: HashMap<String, PageLayout>,
    pub masters: HashMap<String, MasterPage>,
    pub fonts: HashMap<String, Arc<str>>,
    default_para: libertum_model::ParaFormat,
    prepared: bool,
}

/// True for the three paragraph styles Libertum writes for header, footer and page-number
/// text. They carry no user formatting, and importing them as model styles would grow the
/// style sheet on every round trip.
fn is_tool_style(style: &RawStyle) -> bool {
    matches!(
        style.name.as_str(),
        "Header" | "Footer" | "Page_20_Number" | "Libertum_20_Page_20_Number"
    ) || style.display().trim().eq_ignore_ascii_case("page number")
}

impl Styles {
    pub fn new() -> Self {
        Self {
            sheet: StyleSheet::default(),
            ..Self::default()
        }
    }

    /// Reads the parts of `styles.xml` Libertum models: common styles, list styles, page
    /// layouts, master pages and font faces.
    pub fn read_styles_part(
        &mut self,
        text: &str,
        warnings: &mut Vec<String>,
    ) -> Result<(), OdtError> {
        let root = map_xml(xml::parse(text, PART_STYLES))?;
        for child in root.elements() {
            match child.name.as_str() {
                "font-face-decls" => self.read_fonts(child),
                "styles" => self.read_common_styles(child, warnings)?,
                "automatic-styles" => self.read_automatic_styles(child, warnings)?,
                "master-styles" => self.read_master_styles(child),
                _ => {}
            }
        }
        Ok(())
    }

    /// Reads the style blocks that come before the body in `content.xml`.
    pub fn read_content_styles(
        &mut self,
        node: &XmlNode,
        warnings: &mut Vec<String>,
    ) -> Result<(), OdtError> {
        match node.name.as_str() {
            "font-face-decls" => self.read_fonts(node),
            "automatic-styles" => self.read_automatic_styles(node, warnings)?,
            _ => {}
        }
        Ok(())
    }

    fn read_fonts(&mut self, node: &XmlNode) {
        for face in node.children_named("font-face") {
            let Some(name) = face.attr("name") else {
                continue;
            };
            let family = face
                .attr("font-family")
                .or_else(|| face.attr("font-adornments"))
                .map(|value| value.trim().trim_matches(['\'', '"']).to_owned());
            if let Some(family) = family {
                self.fonts.insert(name.to_owned(), Arc::from(family));
            }
        }
    }

    fn read_common_styles(
        &mut self,
        node: &XmlNode,
        warnings: &mut Vec<String>,
    ) -> Result<(), OdtError> {
        for child in node.elements() {
            match child.name.as_str() {
                "default-style" => {
                    let family = child.attr("family").unwrap_or("paragraph");
                    if family == "paragraph" || family == "text" {
                        if let Some(props) = child.child("text-properties") {
                            let raw = self.parse_char(props);
                            let base = self.sheet.defaults.clone();
                            let part = raw.format(&base);
                            self.sheet.defaults = base.apply(&part);
                        }
                        if family == "paragraph"
                            && let Some(props) = child.child("paragraph-properties")
                        {
                            self.default_para = parse_para_format(props);
                            let defaults = self.sheet.para_defaults;
                            self.sheet.para_defaults = defaults.apply(&self.default_para);
                        }
                    }
                }
                "style" => {
                    let style = self.parse_style(child, false);
                    if style.family == "paragraph"
                        && !style.name.is_empty()
                        && !is_tool_style(&style)
                    {
                        self.order.push(style.name.clone());
                    }
                    self.insert_style(style, warnings);
                }
                "list-style" => {
                    let style = parse_list_style(child);
                    if let Some(name) = child.attr("name") {
                        self.list_styles.insert(name.to_owned(), style);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_automatic_styles(
        &mut self,
        node: &XmlNode,
        warnings: &mut Vec<String>,
    ) -> Result<(), OdtError> {
        for child in node.elements() {
            match child.name.as_str() {
                "style" => {
                    let style = self.parse_style(child, true);
                    self.insert_style(style, warnings);
                }
                "list-style" => {
                    let style = parse_list_style(child);
                    if let Some(name) = child.attr("name") {
                        self.list_styles.insert(name.to_owned(), style);
                    }
                }
                "page-layout" => {
                    let layout = self.parse_page_layout(child);
                    if let Some(name) = child.attr("name") {
                        self.layouts.insert(name.to_owned(), layout);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_master_styles(&mut self, node: &XmlNode) {
        for child in node.children_named("master-page") {
            let Some(name) = child.attr("name") else {
                continue;
            };
            let mut master = MasterPage {
                layout: child
                    .attr("page-layout-name")
                    .unwrap_or_default()
                    .to_owned(),
                ..MasterPage::default()
            };
            for block in child.elements() {
                let slot = match block.name.as_str() {
                    "header" => &mut master.header,
                    "header-first" => &mut master.header_first,
                    "header-left" => &mut master.header_left,
                    "footer" => &mut master.footer,
                    "footer-first" => &mut master.footer_first,
                    "footer-left" => &mut master.footer_left,
                    _ => continue,
                };
                *slot = Some(block.clone());
            }
            self.masters.insert(name.to_owned(), master);
        }
    }

    fn insert_style(&mut self, style: RawStyle, warnings: &mut Vec<String>) {
        if style.name.is_empty() {
            return;
        }
        if let Some(previous) = self.raw.get(&style.name)
            && previous.automatic != style.automatic
        {
            warnings.push(format!(
                "style {:?} is defined in both styles.xml and content.xml; the later one wins",
                style.name
            ));
        }
        self.raw.insert(style.name.clone(), style);
    }

    fn parse_style(&self, node: &XmlNode, automatic: bool) -> RawStyle {
        let para_node = node.child("paragraph-properties");
        let para = para_node.map(parse_para_format).unwrap_or_default();
        let page_break_before = para_node.is_some_and(|props| {
            props
                .attr("break-before")
                .is_some_and(|value| value.eq_ignore_ascii_case("page"))
        });
        RawStyle {
            name: node.attr("name").unwrap_or_default().to_owned(),
            family: node.attr("family").unwrap_or("paragraph").to_owned(),
            display_name: node.attr("display-name").map(str::to_owned),
            parent: node.attr("parent-style-name").map(str::to_owned),
            next: node.attr("next-style-name").map(str::to_owned),
            char: node
                .child("text-properties")
                .map(|props| self.parse_char(props))
                .unwrap_or_default(),
            outline: node
                .attr_u32("default-outline-level")
                .map(|level| (level as u8).clamp(1, 9)),
            master_page: node.attr("master-page-name").map(str::to_owned),
            page_break_before,
            automatic,
            table: node.child("table-properties").cloned(),
            cell: node.child("table-cell-properties").cloned(),
            column: node.child("table-column-properties").cloned(),
            row: node.child("table-row-properties").cloned(),
            para,
        }
    }

    fn parse_char(&self, node: &XmlNode) -> RawChar {
        let mut raw = RawChar::default();
        if let Some(family) = node.attr("font-name").and_then(|name| self.fonts.get(name)) {
            raw.family = Some(family.clone());
        } else if let Some(family) = node.attr("font-family") {
            raw.family = Some(Arc::from(
                family.trim().trim_matches(['\'', '"']).to_owned(),
            ));
        }
        if let Some(size) = node.attr("font-size") {
            raw.size = if size.trim().ends_with('%') {
                units::parse_percent(size).map(Size::Factor)
            } else {
                units::parse(size).map(Size::Points)
            };
        }
        if let Some(weight) = node.attr("font-weight") {
            raw.weight = match weight.trim().to_ascii_lowercase().as_str() {
                "bold" => Some(libertum_model::BOLD),
                "normal" => Some(libertum_model::REGULAR),
                other => other.parse::<u16>().ok().map(|w| w.clamp(100, 900)),
            };
        }
        if let Some(style) = node.attr("font-style") {
            raw.italic = Some(matches!(
                style.trim().to_ascii_lowercase().as_str(),
                "italic" | "oblique"
            ));
        }
        if let Some(style) = node.attr("text-underline-style") {
            raw.underline = Some(!style.trim().eq_ignore_ascii_case("none"));
        } else if let Some(kind) = node.attr("text-underline-type") {
            raw.underline = Some(!kind.trim().eq_ignore_ascii_case("none"));
        }
        if let Some(style) = node.attr("text-line-through-style") {
            raw.strikethrough = Some(!style.trim().eq_ignore_ascii_case("none"));
        } else if let Some(kind) = node.attr("text-line-through-type") {
            raw.strikethrough = Some(!kind.trim().eq_ignore_ascii_case("none"));
        }
        if let Some(value) = node.attr("color")
            && !colour::is_auto(value)
        {
            raw.colour = colour::parse(value);
        }
        if let Some(value) = node.attr("background-color") {
            raw.highlight = Some(if colour::is_auto(value) {
                None
            } else {
                colour::parse(value)
            });
        }
        if let Some(value) = node.attr("text-position") {
            let value = value.trim().to_ascii_lowercase();
            raw.vertical = if value.starts_with("super") {
                Some(VerticalAlign::Superscript)
            } else if value.starts_with("sub") {
                Some(VerticalAlign::Subscript)
            } else if value.starts_with("0") || value.starts_with("baseline") {
                Some(VerticalAlign::Baseline)
            } else {
                None
            };
        }
        if let Some(value) = node.attr("letter-spacing")
            && !value.trim().eq_ignore_ascii_case("normal")
        {
            raw.letter_spacing_pt = units::parse(value);
        }
        raw
    }

    fn parse_page_layout(&self, node: &XmlNode) -> PageLayout {
        let mut layout = PageLayout::default();
        let Some(props) = node.child("page-layout-properties") else {
            return layout;
        };
        let width = props.attr("page-width").and_then(units::parse);
        let height = props.attr("page-height").and_then(units::parse);
        if let (Some(width), Some(height)) = (width, height) {
            let landscape = props
                .attr("print-orientation")
                .is_some_and(|value| value.eq_ignore_ascii_case("landscape"))
                || (width > height
                    && !props
                        .attr("print-orientation")
                        .is_some_and(|value| value.eq_ignore_ascii_case("portrait")));
            layout.page.orientation = if landscape {
                libertum_model::Orientation::Landscape
            } else {
                libertum_model::Orientation::Portrait
            };
            let (long, short) = if width >= height {
                (width, height)
            } else {
                (height, width)
            };
            let mut best = libertum_model::PaperSize::A4;
            let mut best_delta = f32::MAX;
            for paper in libertum_model::PaperSize::ALL {
                let (pw, ph) = paper.points();
                let (paper_long, paper_short) = if pw >= ph { (pw, ph) } else { (ph, pw) };
                let delta = (paper_long - long).abs() + (paper_short - short).abs();
                if delta < best_delta {
                    best_delta = delta;
                    best = paper;
                }
            }
            layout.page.paper = best;
        }
        let margin =
            |name: &str, fallback: f32| props.attr(name).and_then(units::parse).unwrap_or(fallback);
        layout.page.margins.top = margin("margin-top", layout.page.margins.top);
        layout.page.margins.right = margin("margin-right", layout.page.margins.right);
        layout.page.margins.bottom = margin("margin-bottom", layout.page.margins.bottom);
        layout.page.margins.left = margin("margin-left", layout.page.margins.left);
        if let Some(number) = props.attr_u32("first-page-number") {
            layout.first_page_number = Some(number);
        }
        // `fo:background-color` (ODF 1.2), or the fill attributes newer LibreOffice also writes.
        layout.background = props
            .attr("background-color")
            .and_then(colour::parse)
            .or_else(|| {
                (props.attr("fill") == Some("solid"))
                    .then(|| props.attr("fill-color").and_then(colour::parse))
                    .flatten()
            });
        if let Some(columns) = props.child("columns") {
            if let Some(count) = columns.attr_u32("column-count") {
                layout.columns = (count as u8).clamp(1, libertum_model::MAX_COLUMNS);
            }
            if let Some(gap) = columns.attr("column-gap").and_then(units::parse) {
                layout.column_gutter_pt = gap.max(0.0);
            }
        }
        if let Some(header) = node
            .child("header-style")
            .and_then(|style| style.child("header-footer-properties"))
            && let Some(value) = header.attr("margin-top").and_then(units::parse)
        {
            layout.header_distance_pt = value.max(0.0);
        }
        if let Some(footer) = node
            .child("footer-style")
            .and_then(|style| style.child("header-footer-properties"))
            && let Some(value) = footer.attr("margin-bottom").and_then(units::parse)
        {
            layout.footer_distance_pt = value.max(0.0);
        }
        layout
    }

    // ---------------------------------------------------------------- resolution

    /// Builds the model style sheet from the common paragraph styles and resolves relative
    /// sizes through the parent chain. Call once, after every style block has been read and
    /// before the body is parsed.
    pub fn prepare(&mut self, warnings: &mut Vec<String>) {
        if self.prepared {
            return;
        }
        self.prepared = true;
        let mut builtins: HashMap<&'static str, StyleId> = HashMap::new();
        for (name, id) in [
            ("body", StyleId::BODY),
            ("normal", StyleId::BODY),
            ("standard", StyleId::BODY),
            ("default style", StyleId::BODY),
            ("default paragraph style", StyleId::BODY),
            ("text body", StyleId::BODY),
            ("body text", StyleId::BODY),
            ("title", StyleId::TITLE),
            ("heading 1", StyleId::HEADING_1),
            ("heading 2", StyleId::HEADING_2),
            ("heading 3", StyleId::HEADING_3),
            ("heading 4", StyleId::HEADING_4),
            ("quote", StyleId::QUOTE),
            ("quotation", StyleId::QUOTE),
        ] {
            builtins.insert(name, id);
        }

        // First pass: give every common paragraph style a model id, in file order. Built-in
        // names keep their slots.
        for name in self.order.clone() {
            let Some(style) = self.raw.get(&name) else {
                continue;
            };
            let display = style.display();
            let id = match builtins.get(display.trim().to_ascii_lowercase().as_str()) {
                Some(&id) => id,
                None => {
                    let id = StyleId(self.sheet.paragraph.len() as u32);
                    self.sheet.paragraph.push(ParagraphStyle {
                        name: display,
                        base: None,
                        char: CharFormat::default(),
                        para: libertum_model::ParaFormat::default(),
                        next: None,
                        outline_level: None,
                    });
                    id
                }
            };
            self.ids.insert(name, id);
        }

        // Second pass: fill in the definitions now that every id exists.
        for name in self.order.clone() {
            let Some(style) = self.raw.get(&name).cloned() else {
                continue;
            };
            let Some(&id) = self.ids.get(&name) else {
                continue;
            };
            if let Some(slot) = self.sheet.paragraph.get_mut(id.0 as usize) {
                if !style.display().is_empty() {
                    slot.name = style.display();
                }
                slot.para = style.para;
                slot.outline_level = style.outline;
                slot.base = style
                    .parent
                    .as_deref()
                    .and_then(|parent| self.ids.get(parent).copied())
                    .filter(|base| *base != id);
                slot.next = style
                    .next
                    .as_deref()
                    .and_then(|next| self.ids.get(next).copied());
            }
        }

        // A parent may appear after its child in the file, so resolve recursively.
        let names: Vec<String> = self.order.clone();
        let mut resolved: HashMap<String, ResolvedChar> = HashMap::new();
        for name in names {
            self.resolve_common_char(&name, &mut resolved, 0);
        }

        // An outline level inherited through `base` is kept on the style that sets it; the
        // model's `outline_level` walks the chain, so nothing else is needed.
        for name in self.order.clone() {
            if !self.raw.contains_key(&name) {
                warnings.push(format!("style {name:?} disappeared while preparing styles"));
            }
        }
    }

    fn resolve_common_char(
        &mut self,
        name: &str,
        resolved: &mut HashMap<String, ResolvedChar>,
        depth: usize,
    ) -> ResolvedChar {
        if let Some(base) = resolved.get(name) {
            return base.clone();
        }
        let Some(style) = self.raw.get(name).cloned() else {
            return self.sheet.defaults.clone();
        };
        let base = match style.parent.as_deref() {
            Some(parent) if depth < MAX_STYLE_DEPTH && self.ids.contains_key(parent) => {
                self.resolve_common_char(parent, resolved, depth + 1)
            }
            _ => self.sheet.defaults.clone(),
        };
        let part = style.char.format(&base);
        if let Some(&id) = self.ids.get(name)
            && let Some(slot) = self.sheet.paragraph.get_mut(id.0 as usize)
        {
            slot.char = part.clone();
        }
        let next = base.apply(&part);
        resolved.insert(name.to_owned(), next.clone());
        next
    }

    /// The styles from `name` up its parent chain, nearest first.
    fn chain(&self, name: &str) -> Vec<&RawStyle> {
        let mut chain = Vec::new();
        let mut at = Some(name);
        while let Some(current) = at {
            if chain.len() >= MAX_STYLE_DEPTH {
                break;
            }
            let Some(style) = self.raw.get(current) else {
                break;
            };
            chain.push(style);
            at = style.parent.as_deref();
        }
        chain
    }

    /// What a paragraph style name does to a paragraph: the model style it belongs to and the
    /// direct formatting its automatic styles add.
    pub fn paragraph(&self, name: Option<&str>) -> ParagraphRef {
        let Some(name) = name else {
            return ParagraphRef::default();
        };
        let chain = self.chain(name);
        if chain.is_empty() {
            return ParagraphRef::default();
        }
        let common = chain
            .iter()
            .position(|style| !style.automatic && style.family == "paragraph");
        let style = common
            .and_then(|index| self.ids.get(&chain[index].name).copied())
            .unwrap_or(StyleId::BODY);
        let direct = match common {
            Some(index) => &chain[..index],
            None => &chain[..],
        };
        let mut format = libertum_model::ParaFormat::default();
        for style in direct.iter().rev() {
            format = format.overlay(&style.para);
        }
        let mut base = self.sheet.resolve_char(style);
        let mut char = CharFormat::default();
        for style in direct.iter().rev() {
            let part = style.char.format(&base);
            base = base.apply(&part);
            char = char.overlay(&part);
        }
        ParagraphRef {
            style,
            format,
            char,
            page_break_before: chain.iter().any(|style| style.page_break_before),
            master_page: chain.iter().find_map(|style| style.master_page.clone()),
            outline: chain.iter().find_map(|style| style.outline),
        }
    }

    /// The direct formatting a text (character) style adds over a run's inherited formatting.
    /// `base` is the fully resolved formatting the run sits under, so relative font sizes in
    /// the style resolve against the paragraph style's size, not the document default.
    pub fn run_format(&self, name: Option<&str>, base: &ResolvedChar) -> CharFormat {
        let Some(name) = name else {
            return CharFormat::default();
        };
        let chain = self.chain(name);
        let mut base = base.clone();
        let mut format = CharFormat::default();
        for style in chain.iter().rev() {
            let part = style.char.format(&base);
            base = base.apply(&part);
            format = format.overlay(&part);
        }
        format
    }

    /// True for the paragraph style Libertum writes around a page-number field, so a saved
    /// file distinguishes it from header text that happens to be `{page}`.
    pub fn is_page_number_style(&self, name: Option<&str>) -> bool {
        let Some(name) = name else {
            return false;
        };
        self.chain(name).iter().any(|style| {
            style.display().trim().eq_ignore_ascii_case("page number")
                || style.name == "Page_20_Number"
        })
    }

    /// The model section a master page describes, without its header/footer text (read.rs
    /// fills that in from the raw blocks).
    pub fn section_for(&self, master_name: &str) -> Section {
        let master = self.masters.get(master_name);
        let mut section = Section::default();
        if let Some(master) = master
            && let Some(layout) = self.layouts.get(&master.layout)
        {
            section.page = layout.page;
            section.first_page_number = layout.first_page_number;
            section.set_columns(layout.columns);
            section.set_column_gutter(layout.column_gutter_pt);
            section.page_colour = layout.background;
            section.header_distance_pt = layout.header_distance_pt;
            section.footer_distance_pt = layout.footer_distance_pt;
        }
        section
    }

    pub fn master(&self, name: &str) -> Option<&MasterPage> {
        self.masters.get(name)
    }

    /// The master page name a document starts with. ODF documents almost always call it
    /// "Standard"; a file that does not use it falls back to the first master page.
    pub fn default_master_name(&self) -> String {
        if self.masters.contains_key("Standard") {
            return "Standard".to_owned();
        }
        if let Some(name) = self.masters.keys().min() {
            return name.clone();
        }
        String::new()
    }

    pub fn list_style(&self, name: Option<&str>) -> Option<ListStyle> {
        let name = name?;
        let style = self.list_styles.get(name)?;
        let level = style
            .levels
            .iter()
            .find_map(|level| level.as_ref())
            .or_else(|| style.levels[0].as_ref())?;
        Some(if level.bullet {
            ListStyle::Bullet
        } else {
            match level.format.as_deref().unwrap_or("1") {
                "I" | "i" => ListStyle::Harvard,
                "A" | "a" => ListStyle::Lettered,
                _ => ListStyle::Numbered,
            }
        })
    }

    // ---------------------------------------------------------------- table styles

    pub fn table_border(&self, name: Option<&str>) -> Option<BorderLine> {
        let props = self.raw.get(name?)?.table.as_ref()?;
        line_from_props(props)
    }

    pub fn cell_border(&self, name: Option<&str>) -> Option<BorderLine> {
        let props = self.raw.get(name?)?.cell.as_ref()?;
        line_from_props(props)
    }

    pub fn cell_fill(&self, name: Option<&str>) -> Option<[u8; 4]> {
        let props = self.raw.get(name?)?.cell.as_ref()?;
        let value = props.attr("background-color")?;
        if colour::is_auto(value) {
            return None;
        }
        colour::parse(value)
    }

    pub fn column_width(&self, name: Option<&str>) -> Option<f32> {
        let props = self.raw.get(name?)?.column.as_ref()?;
        let value = props.attr("column-width")?;
        units::parse(value)
    }

    pub fn row_height(&self, name: Option<&str>) -> Option<f32> {
        let props = self.raw.get(name?)?.row.as_ref()?;
        props
            .attr("row-height")
            .or_else(|| props.attr("min-row-height"))
            .and_then(units::parse)
    }
}

// -------------------------------------------------------------------- parsing helpers

fn map_xml<T>(result: Result<T, xml::XmlError>) -> Result<T, OdtError> {
    result.map_err(|error| match error {
        xml::XmlError::Xml { part, message } => OdtError::Xml { part, message },
        xml::XmlError::Content { part, message } => OdtError::Content { part, message },
    })
}

/// Parses a `style:list-style`, remembering each level's kind and number format.
pub(crate) fn parse_list_style(node: &XmlNode) -> ListStyleDef {
    let mut style = ListStyleDef {
        levels: std::array::from_fn(|_| None),
    };
    for level in node.elements() {
        let (bullet, format) = match level.name.as_str() {
            "list-level-style-bullet" => (true, None),
            "list-level-style-number" => (false, level.attr("num-format").map(str::to_owned)),
            _ => continue,
        };
        let index = level
            .attr_u32("level")
            .map(|value| value.saturating_sub(1).min(8) as usize)
            .unwrap_or(0);
        style.levels[index] = Some(ListLevelDef { bullet, format });
    }
    style
}

/// Parses `style:paragraph-properties`.
pub(crate) fn parse_para_format(node: &XmlNode) -> libertum_model::ParaFormat {
    let mut format = libertum_model::ParaFormat::default();
    if let Some(align) = node.attr("text-align") {
        format.align = match align.trim().to_ascii_lowercase().as_str() {
            "center" | "centre" => Some(Align::Centre),
            "end" | "right" => Some(Align::End),
            "justify" => Some(Align::Justify),
            "start" | "left" => Some(Align::Start),
            _ => None,
        };
    }
    let length = |name: &str| node.attr(name).and_then(units::parse);
    if let Some(left) = length("margin-left").or_else(|| length("margin")) {
        format.indent_left_pt = Some(left);
    }
    if let Some(right) = length("margin-right") {
        format.indent_right_pt = Some(right);
    }
    if let Some(first) = length("text-indent") {
        format.indent_first_pt = Some(first);
    }
    if let Some(before) = length("margin-top") {
        format.space_before_pt = Some(before);
    }
    if let Some(after) = length("margin-bottom") {
        format.space_after_pt = Some(after);
    }
    if let Some(value) = node.attr("line-spacing") {
        format.line_spacing = units::parse(value).map(LineSpacing::Leading);
    } else if let Some(value) = node.attr("line-height-at-least") {
        format.line_spacing = units::parse(value).map(LineSpacing::AtLeast);
    } else if let Some(value) = node.attr("line-height") {
        format.line_spacing = parse_line_height(value);
    }
    if let Some(value) = node.attr("keep-together") {
        format.keep_together = Some(!value.trim().eq_ignore_ascii_case("auto"));
    }
    if let Some(value) = node.attr("keep-with-next") {
        format.keep_with_next = Some(!value.trim().eq_ignore_ascii_case("auto"));
    }
    if let Some(value) = node.attr("hyphenate") {
        format.hyphenate = Some(value.trim().eq_ignore_ascii_case("true"));
    }
    let widow = node
        .attr("widows")
        .or_else(|| node.attr("orphans"))
        .and_then(|value| value.trim().parse::<u32>().ok());
    if let Some(lines) = widow {
        format.widow_control = Some(lines >= 2);
    }
    if let Some(borders) = parse_borders(node) {
        format.borders = Some(borders);
    }
    if let Some(tabs) = node.child("tab-stops") {
        let mut stops = TabStops::new();
        for tab in tabs.children_named("tab-stop") {
            let Some(position) = tab.attr("position").and_then(units::parse) else {
                continue;
            };
            let kind = match tab
                .attr("type")
                .unwrap_or("left")
                .trim()
                .to_ascii_lowercase()
                .as_str()
            {
                "center" | "centre" => TabStopKind::Centre,
                "right" => TabStopKind::Right,
                "char" => {
                    if tab.attr("char") == Some(".") {
                        TabStopKind::Decimal
                    } else {
                        TabStopKind::Left
                    }
                }
                _ => TabStopKind::Left,
            };
            let _ = stops.push(TabStop {
                position_pt: position,
                kind,
            });
        }
        // An explicit (even empty) tab-stops element clears an inherited set.
        format.tab_stops = Some(stops);
    }
    format
}

fn parse_line_height(value: &str) -> Option<LineSpacing> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("normal") {
        return Some(LineSpacing::Multiple(1.0));
    }
    if value.ends_with('%') || value.parse::<f32>().is_ok() {
        return units::parse_percent(value).map(LineSpacing::Multiple);
    }
    units::parse(value).map(LineSpacing::Exactly)
}

/// Reads a paragraph's border box. `fo:border` is the shorthand; per-side attributes override
/// it. Returns `Some(Borders::none())` for an explicit `none`, so direct formatting can clear
/// an inherited box.
fn parse_borders(node: &XmlNode) -> Option<Borders> {
    let shorthand = node.attr("border");
    let mut sides = BorderSides::NONE;
    let mut any = false;
    let mut width = 0.0f32;
    let mut colour = [0, 0, 0, 255];
    let mut explicit_none = false;
    for (name, side) in [
        ("border-top", &mut sides.top),
        ("border-right", &mut sides.right),
        ("border-bottom", &mut sides.bottom),
        ("border-left", &mut sides.left),
    ] {
        let Some(value) = node.attr(name).or(shorthand) else {
            continue;
        };
        match border_line(value) {
            Some(line) => {
                *side = true;
                any = true;
                width = width.max(line.width_pt);
                colour = line.colour;
            }
            None => explicit_none = true,
        }
    }
    if any {
        let space = node
            .attr("padding")
            .or_else(|| {
                [
                    "padding-top",
                    "padding-right",
                    "padding-bottom",
                    "padding-left",
                ]
                .into_iter()
                .find_map(|name| node.attr(name))
            })
            .and_then(units::parse)
            .unwrap_or(0.0);
        Some(Borders {
            sides,
            width_pt: width.max(0.25),
            colour,
            space_pt: space,
        })
    } else if explicit_none || shorthand.is_some() {
        Some(Borders::none())
    } else {
        None
    }
}

/// One side's border value (`0.5pt solid #000000`, `thin`, `none`).
fn border_line(value: &str) -> Option<BorderLine> {
    let mut width = None;
    let mut colour_value = [0, 0, 0, 255];
    let mut none = false;
    for token in value.split_whitespace() {
        let lower = token.to_ascii_lowercase();
        if lower == "none" || lower == "hidden" {
            none = true;
            continue;
        }
        if let Some(points) = units::parse(token) {
            width = Some(points);
        } else if lower == "thin" {
            width = Some(0.5);
        } else if lower == "medium" {
            width = Some(1.0);
        } else if lower == "thick" {
            width = Some(2.0);
        } else if let Some(rgba) = colour::parse(token) {
            colour_value = rgba;
        }
    }
    if none {
        return None;
    }
    Some(BorderLine {
        width_pt: width.unwrap_or(1.0).max(0.25),
        colour: colour_value,
        visible: true,
    })
}

/// The border a table or cell style sets, or an explicit invisible one.
fn line_from_props(props: &XmlNode) -> Option<BorderLine> {
    let shorthand = props.attr("border");
    let mut width = 0.0f32;
    let mut colour_value = [0, 0, 0, 255];
    let mut any = false;
    let mut explicit_none = false;
    for value in [
        shorthand,
        props.attr("border-top"),
        props.attr("border-right"),
        props.attr("border-bottom"),
        props.attr("border-left"),
    ]
    .into_iter()
    .flatten()
    {
        match border_line(value) {
            Some(line) => {
                any = true;
                width = width.max(line.width_pt);
                colour_value = line.colour;
            }
            None => explicit_none = true,
        }
    }
    if any {
        Some(BorderLine {
            width_pt: width,
            colour: colour_value,
            visible: true,
        })
    } else if explicit_none || shorthand.is_some() {
        Some(BorderLine {
            width_pt: 0.0,
            colour: colour_value,
            visible: false,
        })
    } else {
        None
    }
}

impl Styles {
    /// The table borders a table style sets (outer and inner use the same line, as the model
    /// has one width and colour). ODF draws table lines per cell, so the cell style carries
    /// them; a table style with an explicit border is still honoured for foreign files.
    pub fn table_borders(&self, name: Option<&str>, cell_style: Option<&str>) -> TableBorders {
        let table = self.table_border(name);
        let cell = self.cell_border(cell_style);
        let outer = table.is_some_and(|line| line.visible) || cell.is_some_and(|line| line.visible);
        let inner = cell.is_some_and(|line| line.visible);
        let width = table
            .map(|line| line.width_pt)
            .unwrap_or(0.0)
            .max(cell.map(|line| line.width_pt).unwrap_or(0.0));
        let colour = table
            .map(|line| line.colour)
            .or(cell.map(|line| line.colour))
            .unwrap_or([0, 0, 0, 255]);
        if !outer && !inner {
            TableBorders::none()
        } else {
            TableBorders {
                width_pt: width.max(0.25),
                colour,
                outer,
                inner,
            }
        }
    }
}

/// ODF escapes characters in style names as `_XX_` (hex bytes of UTF-8): `Heading_20_1` is
/// `Heading 1`. A multi-byte character becomes several such escapes in a row.
pub(crate) fn decode_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut bytes: Vec<u8> = Vec::new();
    let mut chars = name.chars().peekable();
    let flush = |bytes: &mut Vec<u8>, out: &mut String| {
        if !bytes.is_empty() {
            out.push_str(&String::from_utf8_lossy(bytes));
            bytes.clear();
        }
    };
    while let Some(character) = chars.next() {
        let escape = (character == '_')
            .then(|| chars.clone().take(2).collect::<String>())
            .filter(|next| next.len() == 2)
            .filter(|next| next.chars().all(|c| c.is_ascii_hexdigit()))
            .filter(|_| chars.clone().nth(2) == Some('_'))
            .and_then(|hex| u8::from_str_radix(&hex, 16).ok());
        match escape {
            Some(byte) => {
                bytes.push(byte);
                // Consume the two hex digits and the closing underscore.
                chars.next();
                chars.next();
                chars.next();
            }
            None => {
                flush(&mut bytes, &mut out);
                out.push(character);
            }
        }
    }
    flush(&mut bytes, &mut out);
    out
}

/// Encodes a style or table name the way ODF requires: letters, digits, `-`, `.` and `_` stay;
/// everything else becomes `_XX_` with its UTF-8 bytes in hex.
pub(crate) fn encode_name(name: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_') {
            out.push(character);
        } else {
            let mut buffer = [0u8; 4];
            for byte in character.encode_utf8(&mut buffer).as_bytes() {
                out.push_str(&format!("_{byte:02X}_"));
            }
        }
    }
    if out.is_empty() {
        return fallback.to_owned();
    }
    if out.as_bytes()[0].is_ascii_digit() {
        out.insert(0, 'S');
    }
    out
}

/// Reads the import-only summary of a `<text:list>` element's attributes.
pub(crate) fn list_continues(node: &XmlNode) -> bool {
    node.flag("continue-numbering").unwrap_or(false) || node.attr("continue-list").is_some()
}

/// A paragraph's text, its runs, and how many page-number fields it contains.
#[derive(Clone, Debug, Default)]
pub(crate) struct InlineText {
    pub text: String,
    pub runs: Vec<libertum_model::Run>,
    pub has_page_number: bool,
    /// Notes found inline, with their byte offset in the paragraph and kind.
    pub notes: Vec<(libertum_model::NoteKind, usize, String)>,
    /// Named anchors found inline, at their byte offset in the paragraph.
    pub bookmarks: Vec<libertum_model::Bookmark>,
    /// Inline (`as-char`) picture frames, still to be resolved against the package.
    pub frames: Vec<FrameRef>,
    /// Cross-references to bookmarks, as byte ranges of the text read so far.
    pub cross_refs: Vec<libertum_model::CrossRef>,
}

/// A `draw:frame` anchored as a character that holds a picture. The reader resolves it to an
/// image once it has the package at hand.
#[derive(Clone, Debug)]
pub(crate) struct FrameRef {
    /// Byte offset in the paragraph text.
    pub offset: usize,
    /// The `xlink:href` of each `draw:image` in the frame, in file order (a frame may hold
    /// the same picture in several encodings).
    pub hrefs: Vec<String>,
    pub width_pt: Option<f32>,
    pub height_pt: Option<f32>,
    pub alt: String,
    pub name: String,
}

impl InlineText {
    pub fn append(&mut self, text: &str, format: &CharFormat) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.runs.push(libertum_model::Run {
            range: start..self.text.len(),
            format: format.clone(),
        });
    }
}

/// Drawing elements that can stand where text does. None of them has a model counterpart
/// except a picture frame anchored as a character.
pub(crate) fn is_drawing(name: &str) -> bool {
    matches!(
        name,
        "frame"
            | "image"
            | "object"
            | "text-box"
            | "rect"
            | "line"
            | "polyline"
            | "polygon"
            | "regular-polygon"
            | "path"
            | "circle"
            | "ellipse"
            | "g"
            | "page-thumbnail"
            | "measure"
            | "caption"
            | "connector"
            | "control"
            | "custom-shape"
    )
}

fn contains_drawing(node: &XmlNode) -> bool {
    node.elements()
        .any(|child| is_drawing(&child.name) || contains_drawing(child))
}

pub(crate) fn unsupported(part: &str, what: &str) -> OdtError {
    OdtError::Unsupported {
        part: part.to_owned(),
        what: what.to_owned(),
    }
}

/// True for a picture reference that points outside the package (a linked picture).
fn is_external_href(href: &str) -> bool {
    href.contains(':') || href.starts_with('/') || href.starts_with("..")
}

/// Reads a `draw:frame` found in a paragraph. Only a frame anchored as a character whose
/// content is a picture stored in the package can be modelled; every other frame is refused,
/// because saving would lose it.
fn read_frame(
    node: &XmlNode,
    offset: usize,
    warnings: &mut Vec<String>,
    part: &str,
) -> Result<FrameRef, OdtError> {
    if node.attr("anchor-type") != Some("as-char") {
        return Err(unsupported(part, "a floating picture or frame"));
    }
    let mut hrefs = Vec::new();
    let mut title = None;
    let mut desc = None;
    for child in node.elements() {
        match child.name.as_str() {
            "image" => match child.attr("href") {
                Some(href) if is_external_href(href) => {
                    return Err(unsupported(part, "a linked picture"));
                }
                Some(href) if !href.is_empty() => hrefs.push(href.to_owned()),
                _ => {
                    return Err(unsupported(
                        part,
                        "a picture stored inside the text rather than in the package",
                    ));
                }
            },
            "title" => title = Some(child.text().trim().to_owned()),
            "desc" => desc = Some(child.text().trim().to_owned()),
            // The wrap contour means nothing for a picture that sits in the line.
            "contour-polygon" | "contour-path" => {}
            _ => {
                return Err(unsupported(
                    part,
                    "a text box, embedded object or other frame content",
                ));
            }
        }
    }
    if hrefs.is_empty() {
        return Err(unsupported(part, "a frame without a picture"));
    }
    if node.attr("transform").is_some() {
        warnings.push("the rotation of a picture was dropped".to_owned());
    }
    let length = |name: &str| {
        node.attr(name)
            .and_then(units::parse)
            .filter(|value| *value > 0.0)
    };
    Ok(FrameRef {
        offset,
        hrefs,
        width_pt: length("width"),
        height_pt: length("height"),
        // `svg:desc` is the alternative text; some files only have a title.
        alt: desc
            .filter(|text| !text.is_empty())
            .or(title)
            .unwrap_or_default(),
        name: node.attr("name").unwrap_or_default().to_owned(),
    })
}

/// Records a cross-reference over `start..` of the text read so far. A reference the writer
/// had to split (at a formatting change or an anchor inside it) arrives as adjacent pieces
/// with the same target and kind; they are joined back into one.
fn push_cross_ref(
    out: &mut InlineText,
    start: usize,
    target: &str,
    kind: libertum_model::CrossRefKind,
) {
    let end = out.text.len();
    if let Some(last) = out.cross_refs.last_mut()
        && last.range.end == start
        && last.kind == kind
        && last.target == target
    {
        last.range.end = end;
        return;
    }
    out.cross_refs.push(libertum_model::CrossRef {
        range: start..end,
        target: target.to_owned(),
        kind,
    });
}

fn note_text(node: &libertum_formats_common::xml::XmlNode) -> String {
    fn collect(node: &libertum_formats_common::xml::XmlNode, out: &mut String) {
        out.push_str(&node.text());
        for child in node.elements() {
            collect(child, out);
        }
    }
    let mut paragraphs = Vec::new();
    for body in node.children_named("note-body") {
        for paragraph in body
            .elements()
            .filter(|child| child.name == "p" || child.name == "h")
        {
            let mut text = String::new();
            collect(paragraph, &mut text);
            paragraphs.push(text);
        }
    }
    paragraphs.join("\n")
}

///
/// ODF's whitespace rules are honoured: runs of literal whitespace collapse to one space and
/// leading and trailing literal spaces are dropped, because a file that means several spaces
/// writes `text:s`. `text:span` styles are resolved to direct formatting, and fields that the
/// model has no place for are refused rather than flattened.
pub(crate) fn read_inline(
    node: &XmlNode,
    styles: &Styles,
    base: &CharFormat,
    resolved: &ResolvedChar,
    out: &mut InlineText,
    warnings: &mut Vec<String>,
    part: &str,
) -> Result<(), OdtError> {
    for content in &node.content {
        match content {
            XmlContent::Text(raw) => {
                let collapsed = collapse_whitespace(raw);
                out.append(&collapsed, base);
            }
            XmlContent::Element(child) => match child.name.as_str() {
                "span" => {
                    let span = styles.run_format(child.attr("style-name"), resolved);
                    let format = base.overlay(&span);
                    let child_resolved = resolved.apply(&span);
                    read_inline(child, styles, &format, &child_resolved, out, warnings, part)?;
                }
                "s" => {
                    let count = child.attr_u32("c").unwrap_or(1).max(1);
                    out.append(&" ".repeat(count as usize), base);
                }
                "tab" => out.append("\t", base),
                "line-break" => {
                    out.append(" ", base);
                    warnings.push("a line break inside a paragraph became a space".to_owned());
                }
                "soft-page-break" => {}
                "page-number" => {
                    out.has_page_number = true;
                    out.append("{page}", base);
                }
                "page-count" => out.append("{pages}", base),
                "bookmark" | "bookmark-start" => {
                    if let Some(name) = child.attr("name") {
                        // LibreOffice writes Word's hidden anchors (`_GoBack`) too.
                        if !name.is_empty() && !name.starts_with('_') {
                            out.bookmarks.push(libertum_model::Bookmark {
                                name: name.to_owned(),
                                offset: out.text.len(),
                            });
                        }
                    }
                }
                "bookmark-end"
                | "reference-mark"
                | "reference-mark-start"
                | "reference-mark-end"
                | "note-ref" => {}
                "a" => {
                    read_inline(child, styles, base, resolved, out, warnings, part)?;
                    warnings.push("a hyperlink was flattened to its text".to_owned());
                }
                "bookmark-ref" => {
                    let kind = match child.attr("reference-format") {
                        Some("text") => Some(libertum_model::CrossRefKind::Text),
                        Some("page") => Some(libertum_model::CrossRefKind::Page),
                        _ => None,
                    };
                    let target = child.attr("ref-name").filter(|name| !name.is_empty());
                    let start = out.text.len();
                    read_inline(child, styles, base, resolved, out, warnings, part)?;
                    match (kind, target) {
                        (Some(kind), Some(target)) if out.text.len() > start => {
                            push_cross_ref(out, start, target, kind);
                        }
                        (Some(_), Some(_)) => {
                            warnings.push("a cross-reference with no text was dropped".to_owned());
                        }
                        // Chapter, direction and number references: the text stays, as for
                        // any other inline markup the model has no place for.
                        _ => warnings.push(
                            "a cross-reference of an unsupported kind became plain text".to_owned(),
                        ),
                    }
                }
                "note" => {
                    if contains_drawing(child) {
                        return Err(unsupported(part, "a picture or drawing in a note"));
                    }
                    let kind = if child.attr("note-class") == Some("endnote") {
                        libertum_model::NoteKind::Endnote
                    } else {
                        libertum_model::NoteKind::Footnote
                    };
                    let offset = out.text.len();
                    let text = note_text(child);
                    out.notes.push((kind, offset, text));
                    warnings.push("note formatting was flattened to plain text".to_owned());
                }
                "note-body" | "note-citation" => {}
                "annotation" | "annotation-end" => {
                    return Err(OdtError::Unsupported {
                        part: part.to_owned(),
                        what: "comments".to_owned(),
                    });
                }
                "frame" => {
                    let frame = read_frame(child, out.text.len(), warnings, part)?;
                    out.frames.push(frame);
                }
                name if is_drawing(name) => {
                    return Err(unsupported(part, "a picture or drawing"));
                }
                "tracked-changes" => {
                    return Err(OdtError::Unsupported {
                        part: part.to_owned(),
                        what: "tracked changes".to_owned(),
                    });
                }
                "index-field" | "table-of-content" | "sequence" | "variable-set"
                | "variable-get" | "user-field-get" => {
                    return Err(OdtError::Unsupported {
                        part: part.to_owned(),
                        what: "a field".to_owned(),
                    });
                }
                "ruby" | "ruby-text" => {
                    read_inline(child, styles, base, resolved, out, warnings, part)?;
                }
                _ => {
                    // Unknown inline markup: keep its text rather than losing it.
                    read_inline(child, styles, base, resolved, out, warnings, part)?;
                }
            },
        }
    }
    Ok(())
}

/// Collapses ODF's insignificant whitespace: a run of spaces, tabs and newlines is one space.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(character);
            in_space = false;
        }
    }
    out
}

/// Reads the paragraphs of a header or footer block into text and an optional page number.
pub(crate) fn read_header_footer(
    block: Option<&XmlNode>,
    styles: &Styles,
    warnings: &mut Vec<String>,
    part: &str,
) -> Result<HeaderFooterRead, OdtError> {
    let Some(block) = block else {
        return Ok(HeaderFooterRead::default());
    };
    let mut paragraph_count = 0usize;
    let mut text = String::new();
    let mut align = Align::Start;
    let mut text_align_set = false;
    let mut number_align = Align::Start;
    let mut page_number = false;
    for child in block.elements() {
        if !matches!(child.name.as_str(), "p" | "h") {
            continue;
        }
        let style_name = child.attr("style-name");
        let paragraph_style = styles.paragraph(style_name);
        let resolved = styles
            .sheet
            .resolve_char(paragraph_style.style)
            .apply(&paragraph_style.char);
        let mut inline = InlineText::default();
        read_inline(
            child,
            styles,
            &paragraph_style.char,
            &resolved,
            &mut inline,
            warnings,
            part,
        )?;
        if !inline.frames.is_empty() {
            return Err(unsupported(part, "a picture in a header or footer"));
        }
        if !inline.cross_refs.is_empty() {
            warnings.push("a cross-reference in a header or footer became plain text".to_owned());
        }
        inline.text = inline.text.trim().to_owned();
        if styles.is_page_number_style(style_name) && inline.text == "{page}" {
            page_number = true;
            number_align = paragraph_style.format.align.unwrap_or(Align::Start);
            continue;
        }
        if !inline.text.is_empty() {
            paragraph_count += 1;
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(&inline.text);
            if !text_align_set {
                align = paragraph_style.format.align.unwrap_or(Align::Start);
                text_align_set = true;
            }
        }
    }
    if paragraph_count > 1 {
        warnings.push("a header or footer had several paragraphs; they were joined".to_owned());
    }
    Ok(HeaderFooterRead {
        text,
        align,
        number_align,
        page_number,
    })
}

/// What a header or footer block contained.
#[derive(Clone, Debug, Default)]
pub(crate) struct HeaderFooterRead {
    pub text: String,
    pub align: Align,
    /// Alignment of the page-number paragraph, which may differ from the text's.
    pub number_align: Align,
    pub page_number: bool,
}

/// Parses a whole part into a tree; used by the style tests.
#[cfg(test)]
pub(crate) fn parse_part(text: &str, part: &str) -> Result<XmlNode, OdtError> {
    map_xml(xml::parse(text, part))
}

/// Reads one element subtree from a streaming reader; re-exported for read.rs.
pub(crate) fn read_node(
    reader: &mut Reader<&[u8]>,
    start: &quick_xml::events::BytesStart<'_>,
    part: &str,
) -> Result<XmlNode, OdtError> {
    map_xml(xml::read_node(reader, start, part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_names_encode_and_decode() {
        assert_eq!(decode_name("Heading_20_1"), "Heading 1");
        assert_eq!(decode_name("Plain"), "Plain");
        assert_eq!(decode_name("A_5F_B"), "A_B");
        assert_eq!(encode_name("Heading 1", "S"), "Heading_20_1");
        assert_eq!(
            encode_name("Libertum Page Number", "S"),
            "Libertum_20_Page_20_Number"
        );
        assert_eq!(encode_name("", "Fallback"), "Fallback");
        assert_eq!(encode_name("1st", "S"), "S1st");
        assert_eq!(decode_name(&encode_name("Quote — 1", "S")), "Quote — 1");
    }

    #[test]
    fn parses_paragraph_properties() {
        let xml = r#"<p style:text-align="center" fo:margin-left="1.25cm" fo:text-indent="-0.5in" fo:margin-top="6pt" fo:margin-bottom="0pt" fo:line-height="150%" fo:keep-with-next="always" fo:widows="2" fo:border="0.5pt solid #FF0000" fo:padding="2pt"><tab-stops><tab-stop style:position="4cm" style:type="right"/></tab-stops></p>"#;
        let node = parse_part(xml, "test").expect("parse");
        let format = parse_para_format(&node);
        assert_eq!(format.align, Some(Align::Centre));
        assert!((format.indent_left_pt.unwrap() - 35.43).abs() < 0.1);
        assert!((format.indent_first_pt.unwrap() + 36.0).abs() < 0.1);
        assert_eq!(format.space_after_pt, Some(0.0));
        assert_eq!(format.line_spacing, Some(LineSpacing::Multiple(1.5)));
        assert_eq!(format.keep_with_next, Some(true));
        assert_eq!(format.widow_control, Some(true));
        let borders = format.borders.expect("borders");
        assert_eq!(borders.sides, BorderSides::ALL);
        assert_eq!(borders.colour, [255, 0, 0, 255]);
        assert!((borders.space_pt - 2.0).abs() < 0.01);
    }

    #[test]
    fn line_height_forms() {
        assert_eq!(parse_line_height("15pt"), Some(LineSpacing::Exactly(15.0)));
        assert_eq!(parse_line_height("120%"), Some(LineSpacing::Multiple(1.2)));
        assert_eq!(parse_line_height("1.5"), Some(LineSpacing::Multiple(1.5)));
        assert_eq!(
            parse_line_height("normal"),
            Some(LineSpacing::Multiple(1.0))
        );
    }

    #[test]
    fn collapse_whitespace_rules() {
        assert_eq!(collapse_whitespace("a  b\tc\nd"), "a b c d");
        assert_eq!(collapse_whitespace(" plain "), " plain ");
    }
    #[test]
    fn relative_text_sizes_resolve_against_the_paragraph_style() {
        let mut styles = Styles::new();
        let xml = r#"<automatic-styles><style:style style:name="T1" style:family="text"><style:text-properties fo:font-size="150%"/></style:style></automatic-styles>"#;
        let node = parse_part(xml, "test").expect("parse");
        styles
            .read_content_styles(&node, &mut Vec::new())
            .expect("styles");
        let mut base = styles.sheet.defaults.clone();
        base.size_pt = 18.0;
        let format = styles.run_format(Some("T1"), &base);
        assert_eq!(
            format.size_pt,
            Some(27.0),
            "150% of the paragraph style's 18 pt, not of the 11 pt document default"
        );
        assert!(
            styles.run_format(None, &base).is_empty(),
            "no style means no direct formatting"
        );
    }

    #[test]
    fn border_values() {
        let line = border_line("0.75pt solid #00FF00").expect("line");
        assert!((line.width_pt - 0.75).abs() < 0.01);
        assert_eq!(line.colour, [0, 255, 0, 255]);
        assert!(border_line("none").is_none());
        assert_eq!(border_line("thin solid #000000").unwrap().width_pt, 0.5);
    }
}
