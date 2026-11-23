//! Searchable-text PDF export.
//!
//! The pages come from the same [`LayoutState`] the screen uses, so pagination, tables,
//! headers, footers, page numbers and TOC leaders land where they do on screen. Text is
//! written as real glyph IDs from parley, not as images: printpdf embeds (and subsets) the
//! fonts, builds a `ToUnicode` map, and therefore the text can be selected, searched and
//! copied. The archival variant adds the sRGB output intent and XMP metadata PDF/A-2b needs.

use std::collections::{BTreeMap, HashMap, HashSet};

use libertum_layout::parley::PositionedLayoutItem;
use libertum_layout::{Decoration, LayoutOptions, LayoutState, PageGeometry, ParagraphLayout};
use libertum_model::Document;
use printpdf::{
    Actions, BorderArray, Codepoint, Color, CurTransMat, Destination, FontId, LinePoint,
    LinkAnnotation, Mm, Op, PaintMode, ParsedFont, PdfDocument, PdfFontHandle, PdfPage,
    PdfSaveOptions, PdfWarnMsg, Point, Polygon, PolygonRing, Pt, RawImage, RawImageData,
    RawImageFormat, Rect, Rgb, TextItem, TextMatrix, TextRenderingMode, WindingOrder, XObjectId,
    XObjectTransform,
};

/// Why a PDF could not be written.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("the font {0:?} could not be embedded in the PDF")]
    Font(String),
    #[error("an image could not be read for the PDF: {0}")]
    Image(String),
    #[error("the PDF could not be written: {0}")]
    Write(String),
}

/// Exports the document as a PDF, or as PDF/A-2b when `archival` is set.
pub fn export(document: &Document, archival: bool, title: &str) -> Result<Vec<u8>, PdfError> {
    let geometry = PageGeometry::from_setup(&document.page_setup());
    let mut layout = LayoutState::from_document(document, geometry, LayoutOptions::default());
    layout.paginate_all();

    let mut pdf = PdfDocument::new(title);
    pdf.metadata.info.document_title = title.to_owned();
    pdf.metadata.info.creator = "Libertum".to_owned();
    pdf.metadata.info.producer = "Libertum".to_owned();
    // PDF 1.7: the version PDF/A-2 is based on.
    pdf.metadata.info.version = 7;

    let mut exporter = Exporter {
        pdf: &mut pdf,
        fonts: HashMap::new(),
        drawn_as_shapes: HashSet::new(),
        scale_cx: swash::scale::ScaleContext::new(),
        images: HashMap::new(),
        vertical: false,
    };
    let mut pages = Vec::with_capacity(layout.page_count());
    for index in 0..layout.page_count() {
        // The layout page (turned for vertical text) and the sheet it is written onto.
        let geometry = layout.page_geometry(index);
        let sheet = layout.physical_page_geometry(index);
        let mut ops = Vec::new();
        exporter.page(&layout, index, &geometry, &sheet, &mut ops)?;
        pages.push(PdfPage::new(
            Mm::from(Pt(sheet.width_pt)),
            Mm::from(Pt(sheet.height_pt)),
            ops,
        ));
    }
    pdf.with_pages(pages);
    add_outline(&mut pdf, document, &layout);

    let mut warnings: Vec<PdfWarnMsg> = Vec::new();
    let options = PdfSaveOptions {
        subset_fonts: true,
        ..PdfSaveOptions::default()
    };
    let bytes = pdf.save(&options, &mut warnings);
    for warning in &warnings {
        tracing::warn!(warning = ?warning, "PDF export warning");
    }
    if bytes.is_empty() {
        return Err(PdfError::Write(
            "the PDF writer produced no bytes".to_owned(),
        ));
    }
    if archival {
        archival_pdf(bytes, title)
    } else {
        Ok(bytes)
    }
}

/// The viewer's bookmark panel: one entry per heading (not the generated contents title or
/// its entries), in reading order, each pointing at the page the heading starts on. The
/// contents entries themselves are not links; the bookmarks are the way to jump.
fn add_outline(pdf: &mut PdfDocument, document: &Document, layout: &LayoutState) {
    let paragraphs: Vec<_> = document.paragraphs().collect();
    for (index, paragraph) in paragraphs.iter().enumerate() {
        if paragraph.props.outline_level.is_none() || paragraph.props.toc_target.is_some() {
            continue;
        }
        // The title above a generated contents list is heading-styled, but is not a chapter.
        let titles_contents = paragraphs
            .get(index + 1)
            .is_some_and(|next| next.props.toc_target.is_some());
        let text = paragraph.text();
        let text = text.trim();
        if titles_contents || text.is_empty() {
            continue;
        }
        if let Some(page) = layout.page_index_of_block(paragraph.id.0) {
            // Bookmark pages count from one.
            pdf.add_bookmark(text, page + 1);
        }
    }
}

struct FontInfo {
    id: FontId,
    /// Glyph widths in font units.
    widths: BTreeMap<u16, u16>,
    units_per_em: u16,
}

/// A run of glyphs to draw as shapes (see `Exporter::glyph_shapes`).
struct ShapeRun<'a> {
    data: &'a [u8],
    index: u32,
    /// The font's variation coordinates, as parley shaped the run.
    coords: &'a [i16],
    size: f32,
    /// Where the run's glyph positions are measured from, page points from the top-left.
    origin: (f32, f32),
    colour: &'a Color,
    bold: bool,
    /// Which glyphs stand upright on a vertical page (empty otherwise), with the run's ascent
    /// and descent and the sheet's width, for placing them.
    upright: &'a [crate::vertical::Stand],
    ascent: f32,
    descent: f32,
    sheet_width: f32,
    /// The run's font size, points: upright digits were given room after them in proportion.
    font_size: f32,
}

/// Where a watermark's single line goes: turned by an angle (screen sense, y down) about the
/// page's centre, in one flat colour.
struct Placement {
    colour: Color,
    cos: f32,
    sin: f32,
    /// The page's centre, page points from the top-left.
    centre: (f32, f32),
    /// The middle of the text's line box, in the layout's own coordinates.
    layout_centre: (f32, f32),
}

struct Exporter<'a> {
    pdf: &'a mut PdfDocument,
    fonts: HashMap<(u64, u32), FontInfo>,
    /// Fonts a PDF cannot carry as text (variable fonts with CFF2 outlines, which is how
    /// Noto CJK ships on some Linux distributions): their glyphs are drawn as vector shapes.
    drawn_as_shapes: HashSet<(u64, u32)>,
    scale_cx: swash::scale::ScaleContext,
    images: HashMap<u64, XObjectId>,
    /// The page being written is vertical text: its content is laid out on the turned page and
    /// turned back by one matrix, with ideographs counter-turned to stand upright.
    vertical: bool,
}

/// Whether the font at `index` of an sfnt file or collection has a table `tag`.
fn sfnt_has_table(data: &[u8], index: u32, tag: &[u8; 4]) -> bool {
    let be32 = |at: usize| -> Option<usize> {
        let bytes: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(bytes) as usize)
    };
    let start = if data.get(..4) == Some(b"ttcf") {
        let count = be32(8).unwrap_or(0);
        if (index as usize) >= count {
            return false;
        }
        match be32(12 + 4 * index as usize) {
            Some(at) => at,
            None => return false,
        }
    } else {
        0
    };
    let Some(tables) = data.get(start + 4..start + 6) else {
        return false;
    };
    let tables = usize::from(u16::from_be_bytes([tables[0], tables[1]]));
    (0..tables).any(|n| data.get(start + 12 + 16 * n..start + 16 + 16 * n) == Some(tag.as_slice()))
}

impl Exporter<'_> {
    fn font(&mut self, data: &[u8], data_id: u64, index: u32) -> Option<&FontInfo> {
        let key = (data_id, index);
        if self.drawn_as_shapes.contains(&key) {
            return None;
        }
        if !self.fonts.contains_key(&key) {
            let mut warnings = Vec::new();
            // A CFF2 font cannot be embedded as text, and one printpdf cannot read at all
            // would stop the export: both are drawn as shapes instead.
            let parsed = if sfnt_has_table(data, index, b"CFF2") {
                None
            } else {
                ParsedFont::from_bytes(data, index as usize, &mut warnings)
            };
            let Some(parsed) = parsed else {
                self.drawn_as_shapes.insert(key);
                return None;
            };
            let id = self.pdf.add_font(&parsed);
            let widths = parsed.glyph_widths.clone();
            let units_per_em = parsed.units_per_em.max(1);
            self.fonts.insert(
                key,
                FontInfo {
                    id,
                    widths,
                    units_per_em,
                },
            );
        }
        self.fonts.get(&key)
    }

    fn page(
        &mut self,
        layout: &LayoutState,
        page_index: usize,
        geometry: &PageGeometry,
        sheet: &PageGeometry,
        ops: &mut Vec<Op>,
    ) -> Result<(), PdfError> {
        let Some(page) = layout.pages().get(page_index) else {
            return Ok(());
        };
        let page_height = geometry.height_pt;
        self.vertical = layout.is_vertical();

        // A page colour fills the whole sheet, under everything else (as the editor shows it).
        if let Some(colour) = layout.page_colour(page_index) {
            rect(
                ops,
                0.0,
                0.0,
                sheet.width_pt,
                sheet.height_pt,
                colour,
                sheet.height_pt,
            );
        }
        // Vertical text: everything below is written for the turned layout page and turned
        // back onto the sheet by one matrix (a quarter turn clockwise: x' = y, y' = width - x).
        if self.vertical {
            ops.push(Op::SaveGraphicsState);
            ops.push(Op::SetTransformationMatrix {
                matrix: CurTransMat::Raw([0.0, -1.0, 1.0, 0.0, 0.0, geometry.width_pt]),
            });
        }

        // The watermark lies under everything else.
        if let Some(watermark) = layout.page_watermark(page_index) {
            let placement = Placement {
                colour: blend(watermark.colour),
                cos: watermark.angle_deg.to_radians().cos(),
                sin: watermark.angle_deg.to_radians().sin(),
                centre: (watermark.centre_x, watermark.centre_y),
                layout_centre: (
                    watermark.layout.layout.width() * 0.5,
                    watermark.layout.height * 0.5,
                ),
            };
            let lines = 0..watermark.layout.lines.len();
            self.lines(
                ops,
                &watermark.layout,
                lines,
                (0.0, 0.0),
                page_height,
                Some(&placement),
            )?;
        }

        // Table cell fills and borders, under the text. Upright tables are on the sheet
        // itself and come after the turned content.
        table_rects(
            ops,
            page.tables.iter().filter(|t| !t.upright),
            geometry,
            page_height,
        );

        // Body and table-cell paragraphs.
        for slice in page.slices.iter().filter(|s| !s.upright) {
            let Some(pl) = layout.paragraph_layout(slice.para) else {
                continue;
            };
            let Some(first) = pl.lines.get(slice.line_start) else {
                continue;
            };
            let dy = geometry.margins.top + slice.y - first.top;
            let dx = geometry.margins.left + slice.x + pl.x_offset;
            self.lines(
                ops,
                &pl,
                slice.line_start..slice.line_end,
                (dx, dy),
                page_height,
                None,
            )?;
        }

        // Each generated contents entry jumps to the page its heading is on. The box has no
        // border and does not change the look of the page.
        for link in layout.page_toc_links(page_index) {
            // Annotation boxes are in the sheet's own space, not the turned content's.
            let (x, y, width, height) = (
                link.x,
                page_height - (link.y + link.height),
                link.width,
                link.height,
            );
            let (x, y, width, height) = if self.vertical {
                (y, geometry.width_pt - x - width, height, width)
            } else {
                (x, y, width, height)
            };
            ops.push(Op::LinkAnnotation {
                link: LinkAnnotation::new(
                    Rect {
                        x: Pt(x),
                        y: Pt(y),
                        width: Pt(width),
                        height: Pt(height),
                        mode: None,
                        winding_order: None,
                    },
                    Actions::Goto(Destination::Xyz {
                        page: link.target_page + 1,
                        left: Some(0.0),
                        top: Some(page_height),
                        zoom: None,
                    }),
                    Some(BorderArray::Solid([0.0, 0.0, 0.0])),
                    None,
                    None,
                ),
            });
        }

        // Page numbers, TOC entries, headers and footers, inline images and rules. On a vertical
        // page the headers, footers and page numbers are on the sheet itself, upright and
        // horizontal, so they are written after the turned content, outside its matrix.
        let mut on_sheet = Vec::new();
        for decoration in layout.page_decorations(page_index) {
            match decoration {
                Decoration::Text {
                    layout,
                    x,
                    y,
                    sheet: true,
                    ..
                } => on_sheet.push((layout, x, y)),
                Decoration::Text { layout, x, y, .. } => {
                    self.lines(
                        ops,
                        &layout,
                        0..layout.lines.len(),
                        (x, y),
                        page_height,
                        None,
                    )?;
                }
                Decoration::Leader { x0, x1, baseline } => {
                    rect(
                        ops,
                        x0,
                        baseline - 0.5,
                        (x1 - x0).max(0.0),
                        0.5,
                        [0, 0, 0, 255],
                        page_height,
                    );
                }
                Decoration::Rect {
                    x,
                    y,
                    width,
                    height,
                    colour,
                } => rect(ops, x, y, width, height, colour, page_height),
                Decoration::Image {
                    media,
                    x,
                    y,
                    width,
                    height,
                } => {
                    let id = self.image(&media)?;
                    if self.vertical {
                        // The box has its width and height swapped, so on the sheet it is the
                        // picture's own shape. The page matrix would turn the picture with the
                        // text; this matrix turns it back the other way and puts it in its box:
                        // (u, v) -> (x + width - v, page_height - y - height + u).
                        ops.push(Op::SaveGraphicsState);
                        ops.push(Op::SetTransformationMatrix {
                            matrix: CurTransMat::Raw([
                                0.0,
                                1.0,
                                -1.0,
                                0.0,
                                x + width,
                                page_height - y - height,
                            ]),
                        });
                        ops.push(Op::UseXobject {
                            id,
                            transform: XObjectTransform {
                                scale_x: Some(height / media.width_px().max(1) as f32),
                                scale_y: Some(width / media.height_px().max(1) as f32),
                                dpi: Some(72.0),
                                ..XObjectTransform::default()
                            },
                        });
                        ops.push(Op::RestoreGraphicsState);
                    } else {
                        ops.push(Op::UseXobject {
                            id,
                            transform: XObjectTransform {
                                translate_x: Some(Pt(x)),
                                translate_y: Some(Pt(page_height - y - height)),
                                scale_x: Some(width / media.width_px().max(1) as f32),
                                scale_y: Some(height / media.height_px().max(1) as f32),
                                dpi: Some(72.0),
                                ..XObjectTransform::default()
                            },
                        });
                    }
                }
            }
        }
        if self.vertical {
            ops.push(Op::RestoreGraphicsState);
            self.vertical = false;
        }
        // Upright tables: fills, lines and text on the sheet like any horizontal page.
        if page.tables.iter().any(|t| t.upright) {
            table_rects(
                ops,
                page.tables.iter().filter(|t| t.upright),
                sheet,
                sheet.height_pt,
            );
            for slice in page.slices.iter().filter(|s| s.upright) {
                let Some(pl) = layout.paragraph_layout(slice.para) else {
                    continue;
                };
                let Some(first) = pl.lines.get(slice.line_start) else {
                    continue;
                };
                let dy = sheet.margins.top + slice.y - first.top;
                let dx = sheet.margins.left + slice.x + pl.x_offset;
                self.lines(
                    ops,
                    &pl,
                    slice.line_start..slice.line_end,
                    (dx, dy),
                    sheet.height_pt,
                    None,
                )?;
            }
        }
        for (text, x, y) in on_sheet {
            self.lines(
                ops,
                &text,
                0..text.lines.len(),
                (x, y),
                sheet.height_pt,
                None,
            )?;
        }
        Ok(())
    }

    fn lines(
        &mut self,
        ops: &mut Vec<Op>,
        pl: &ParagraphLayout,
        lines: std::ops::Range<usize>,
        (dx, dy): (f32, f32),
        page_height: f32,
        turned: Option<&Placement>,
    ) -> Result<(), PdfError> {
        for index in lines {
            let Some(line) = pl.layout.get(index) else {
                continue;
            };
            let line_box = pl.lines.get(index);
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let brush = run.style().brush;
                if let (Some(highlight), Some(line_box), None) = (brush.highlight, line_box, turned)
                {
                    rect(
                        ops,
                        dx + run.offset(),
                        dy + line_box.top,
                        run.advance(),
                        line_box.height,
                        highlight,
                        page_height,
                    );
                }
                let text_run = run.run();
                let font = text_run.font();
                let size = text_run.font_size();
                let rise = brush.rise_pt();
                let key = (font.data.id(), font.index);
                let as_text = self
                    .font(font.data.data(), font.data.id(), font.index)
                    .is_some();
                let positions: Vec<_> = run.positioned_glyphs().collect();
                if positions.is_empty() {
                    continue;
                }
                let colour = turned.map_or_else(|| blend(brush.color), |t| t.colour.clone());
                let bold = text_run.synthesis().embolden();
                // Vertical text: which glyphs stand upright. A watermark turns as a whole, so
                // its glyphs all go with the page.
                let vertical = self.vertical && turned.is_none();
                let upright = if vertical {
                    crate::vertical::upright_glyphs(&pl.text, text_run)
                } else {
                    Vec::new()
                };
                let run_metrics = text_run.font_metrics();
                let (ascent, descent) = (run_metrics.ascent, run_metrics.descent);
                if !as_text {
                    let coords: Vec<i16> = text_run
                        .normalized_coords()
                        .iter()
                        .map(|c| c.to_bits())
                        .collect();
                    self.glyph_shapes(
                        &ShapeRun {
                            data: font.data.data(),
                            index: font.index,
                            coords: &coords,
                            size,
                            origin: (dx, dy - rise),
                            colour: &colour,
                            bold,
                            upright: &upright,
                            ascent,
                            descent,
                            sheet_width: page_height,
                            font_size: size,
                        },
                        &positions,
                        page_height,
                        turned,
                        ops,
                    );
                }
                if as_text && let Some(info) = self.fonts.get(&key) {
                    // What each shaped glyph stands for, so the PDF's ToUnicode map (and so
                    // copying and searching) knows the text of a ligature or a contextual form.
                    let mut unicode: HashMap<u16, String> = HashMap::new();
                    for cluster in run.run().clusters() {
                        let text = pl
                            .text
                            .get(cluster.text_range())
                            .filter(|text| !text.is_empty())
                            .unwrap_or("");
                        if text.is_empty() {
                            continue;
                        }
                        for glyph in cluster.glyphs() {
                            unicode
                                .entry(glyph.id.min(u32::from(u16::MAX)) as u16)
                                .or_insert_with(|| text.to_owned());
                        }
                    }
                    let font_id = info.id.clone();
                    // Glyphs are written in groups: a vertical page's upright glyphs one at a
                    // time, each placed in its own cell, and everything else as one string.
                    let mut groups: Vec<(usize, usize, bool)> = Vec::new();
                    for at in 0..positions.len() {
                        let standing = upright.get(at).is_some_and(|stand| stand.upright());
                        match groups.last_mut() {
                            Some((_, end, last)) if !standing && !*last => *end = at + 1,
                            _ => groups.push((at, at + 1, standing)),
                        }
                    }
                    for (from, to, standing) in groups {
                        let group = &positions[from..to];
                        let mut glyphs = Vec::with_capacity(group.len());
                        for (at, glyph) in group.iter().enumerate() {
                            let gid = glyph.id.min(u32::from(u16::MAX)) as u16;
                            let natural = info.widths.get(&gid).copied().unwrap_or(0) as f32 * size
                                / f32::from(info.units_per_em);
                            let desired =
                                group.get(at + 1).map_or(natural, |next| next.x - glyph.x);
                            let offset = (natural - desired) * 1000.0 / size.max(0.01);
                            glyphs.push(match unicode.get(&gid) {
                                Some(text) => Codepoint::with_cid(gid, offset, text.clone()),
                                None => Codepoint::new(gid, offset),
                            });
                        }
                        ops.push(Op::StartTextSection);
                        ops.push(Op::SetFillColor {
                            col: colour.clone(),
                        });
                        ops.push(Op::SetFont {
                            font: PdfFontHandle::External(font_id.clone()),
                            size: Pt(size),
                        });
                        if bold {
                            ops.push(Op::SetTextRenderingMode {
                                mode: TextRenderingMode::FillStroke,
                            });
                            ops.push(Op::SetOutlineColor {
                                col: colour.clone(),
                            });
                            ops.push(Op::SetOutlineThickness {
                                pt: Pt(size * 0.03),
                            });
                        } else {
                            ops.push(Op::SetTextRenderingMode {
                                mode: TextRenderingMode::Fill,
                            });
                        }
                        let first = &group[0];
                        if standing {
                            // The page is turned a quarter turn clockwise by the page matrix,
                            // so an upright glyph is written turned a quarter turn the other
                            // way, with its em box centred on the column and its top at the
                            // cell's start (the same placement the page renderer uses).
                            let cell_of = crate::vertical::upright_cell(
                                upright[from],
                                &positions,
                                from,
                                size,
                            );
                            let (cell, centre) = crate::vertical::upright_origin(
                                dx + cell_of.start,
                                dy + first.y - rise,
                                &cell_of,
                                ascent,
                                descent,
                                page_height,
                            );
                            ops.push(Op::SetTextMatrix {
                                matrix: TextMatrix::Raw([0.0, 1.0, -1.0, 0.0, cell, centre]),
                            });
                        } else {
                            let baseline = dy + first.y - rise;
                            match turned {
                                None => ops.push(Op::SetTextCursor {
                                    pos: Point {
                                        x: Pt(dx + first.x),
                                        y: Pt(page_height - baseline),
                                    },
                                }),
                                Some(turn) => {
                                    // Turn the baseline's start about the page's centre; the
                                    // matrix turns the glyphs the same way (PDF's y axis
                                    // points up).
                                    let (ox, oy) = (
                                        first.x - turn.layout_centre.0,
                                        first.y - turn.layout_centre.1,
                                    );
                                    let x = turn.centre.0 + ox * turn.cos - oy * turn.sin;
                                    let y = turn.centre.1 + ox * turn.sin + oy * turn.cos;
                                    ops.push(Op::SetTextMatrix {
                                        matrix: TextMatrix::Raw([
                                            turn.cos,
                                            -turn.sin,
                                            turn.sin,
                                            turn.cos,
                                            x,
                                            page_height - y,
                                        ]),
                                    });
                                }
                            }
                        }
                        ops.push(Op::ShowText {
                            items: vec![TextItem::GlyphIds(glyphs)],
                        });
                        ops.push(Op::EndTextSection);
                    }
                }

                // Underline and strikethrough, at the font's own offsets.
                let style = run.style();
                let metrics = text_run.font_metrics();
                for (decoration, offset, size) in [
                    (
                        &style.underline,
                        metrics.underline_offset,
                        metrics.underline_size,
                    ),
                    (
                        &style.strikethrough,
                        metrics.strikethrough_offset,
                        metrics.strikethrough_size,
                    ),
                ] {
                    if let Some(decoration) = decoration {
                        let offset = decoration.offset.unwrap_or(offset);
                        let thickness = decoration.size.unwrap_or(size).max(0.5);
                        let top = dy + run.baseline() - rise - offset - thickness * 0.5;
                        rect(
                            ops,
                            dx + run.offset(),
                            top,
                            run.advance(),
                            thickness,
                            decoration.brush.color,
                            page_height,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// Draws a run's glyphs as filled vector shapes, for a font the PDF cannot embed as text.
    /// The page looks the same; the text of such a run cannot be selected or searched.
    fn glyph_shapes(
        &mut self,
        run: &ShapeRun<'_>,
        positions: &[libertum_layout::parley::Glyph],
        page_height: f32,
        turned: Option<&Placement>,
        ops: &mut Vec<Op>,
    ) {
        use swash::zeno::Verb;
        let Some(font) = swash::FontRef::from_index(run.data, run.index as usize) else {
            return;
        };
        let mut scaler = self
            .scale_cx
            .builder(font)
            .size(run.size)
            .hint(false)
            .normalized_coords(run.coords)
            .build();
        // Maps a point of a glyph (font size units, y up) to the page (PDF points, y up). An
        // upright glyph on a vertical page is turned a quarter turn the other way and put in
        // its cell, since the page matrix is about to turn everything clockwise.
        let place = |glyph: &libertum_layout::parley::Glyph,
                     cell: Option<&crate::vertical::UprightCell>,
                     x: f32,
                     y: f32|
         -> Point {
            let (gx, gy) = (run.origin.0 + glyph.x, run.origin.1 + glyph.y);
            if let Some(cell) = cell {
                let (along, across) = crate::vertical::upright_origin(
                    run.origin.0 + cell.start,
                    gy,
                    cell,
                    run.ascent,
                    run.descent,
                    run.sheet_width,
                );
                return Point {
                    x: Pt(along - y),
                    y: Pt(across + x),
                };
            }
            match turned {
                None => Point {
                    x: Pt(gx + x),
                    y: Pt(page_height - gy + y),
                },
                Some(turn) => {
                    let (dx, dy) = (gx - turn.layout_centre.0, gy - turn.layout_centre.1);
                    let cx = turn.centre.0 + dx * turn.cos - dy * turn.sin;
                    let cy = turn.centre.1 + dx * turn.sin + dy * turn.cos;
                    Point {
                        x: Pt(cx + turn.cos * x + turn.sin * y),
                        y: Pt(page_height - cy - turn.sin * x + turn.cos * y),
                    }
                }
            }
        };
        ops.push(Op::SetFillColor {
            col: run.colour.clone(),
        });
        let mode = if run.bold {
            ops.push(Op::SetOutlineColor {
                col: run.colour.clone(),
            });
            ops.push(Op::SetOutlineThickness {
                pt: Pt(run.size * 0.03),
            });
            PaintMode::FillStroke
        } else {
            PaintMode::Fill
        };
        for (index, glyph) in positions.iter().enumerate() {
            let stand = run
                .upright
                .get(index)
                .copied()
                .unwrap_or(crate::vertical::Stand::Sideways);
            let standing = stand.upright();
            let cell = standing
                .then(|| crate::vertical::upright_cell(stand, positions, index, run.font_size));
            let Some(outline) = scaler.scale_outline(glyph.id.min(u32::from(u16::MAX)) as u16)
            else {
                continue;
            };
            let points = outline.points();
            let mut rings: Vec<PolygonRing> = Vec::new();
            let mut ring: Vec<LinePoint> = Vec::new();
            let mut next = 0;
            let mut current = (0.0_f32, 0.0_f32);
            let at = |i: usize| (points[i].x, points[i].y);
            let on = |p: (f32, f32)| LinePoint {
                p: place(glyph, cell.as_ref(), p.0, p.1),
                bezier: false,
            };
            let off = |p: (f32, f32)| LinePoint {
                p: place(glyph, cell.as_ref(), p.0, p.1),
                bezier: true,
            };
            for verb in outline.verbs() {
                match verb {
                    Verb::MoveTo => {
                        if !ring.is_empty() {
                            rings.push(PolygonRing {
                                points: std::mem::take(&mut ring),
                            });
                        }
                        current = at(next);
                        ring.push(on(current));
                        next += 1;
                    }
                    Verb::LineTo => {
                        current = at(next);
                        ring.push(on(current));
                        next += 1;
                    }
                    Verb::QuadTo => {
                        // A quadratic curve as the cubic with the same shape.
                        let (c, end) = (at(next), at(next + 1));
                        let c1 = (
                            current.0 + 2.0 / 3.0 * (c.0 - current.0),
                            current.1 + 2.0 / 3.0 * (c.1 - current.1),
                        );
                        let c2 = (
                            end.0 + 2.0 / 3.0 * (c.0 - end.0),
                            end.1 + 2.0 / 3.0 * (c.1 - end.1),
                        );
                        ring.push(off(c1));
                        ring.push(off(c2));
                        ring.push(on(end));
                        current = end;
                        next += 2;
                    }
                    Verb::CurveTo => {
                        let (c1, c2, end) = (at(next), at(next + 1), at(next + 2));
                        ring.push(off(c1));
                        ring.push(off(c2));
                        ring.push(on(end));
                        current = end;
                        next += 3;
                    }
                    Verb::Close => {
                        if !ring.is_empty() {
                            rings.push(PolygonRing {
                                points: std::mem::take(&mut ring),
                            });
                        }
                    }
                }
            }
            if !ring.is_empty() {
                rings.push(PolygonRing { points: ring });
            }
            if rings.is_empty() {
                continue;
            }
            ops.push(Op::DrawPolygon {
                polygon: Polygon {
                    rings,
                    mode,
                    winding_order: WindingOrder::NonZero,
                },
            });
        }
    }

    fn image(&mut self, media: &libertum_model::Media) -> Result<XObjectId, PdfError> {
        if let Some(id) = self.images.get(&media.hash()) {
            return Ok(id.clone());
        }
        let decoded = image::load_from_memory(media.bytes())
            .map_err(|error| PdfError::Image(error.to_string()))?
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for pixel in decoded.pixels() {
            let [r, g, b, a] = pixel.0;
            let alpha = u32::from(a);
            rgb.push(((u32::from(r) * alpha + 255 * (255 - alpha)) / 255) as u8);
            rgb.push(((u32::from(g) * alpha + 255 * (255 - alpha)) / 255) as u8);
            rgb.push(((u32::from(b) * alpha + 255 * (255 - alpha)) / 255) as u8);
        }
        let raw = RawImage {
            pixels: RawImageData::U8(rgb),
            width: width as usize,
            height: height as usize,
            data_format: RawImageFormat::RGB8,
            tag: Vec::new(),
        };
        let id = self.pdf.add_image(&raw);
        self.images.insert(media.hash(), id.clone());
        Ok(id)
    }
}

/// A colour with its alpha composited over white: PDF/A forbids transparency, and a page is
/// white.
fn blend(colour: [u8; 4]) -> Color {
    let [r, g, b, a] = colour.map(u32::from);
    let mix = |c: u32| (c * a + 255 * (255 - a)) as f32 / 255.0 / 255.0;
    Color::Rgb(Rgb {
        r: mix(r),
        g: mix(g),
        b: mix(b),
        icc_profile: None,
    })
}

/// A filled rectangle given in page points from the top-left corner.
fn rect(
    ops: &mut Vec<Op>,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    colour: [u8; 4],
    page_height: f32,
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    ops.push(Op::SetFillColor { col: blend(colour) });
    ops.push(Op::DrawRectangle {
        rectangle: Rect {
            x: Pt(x),
            y: Pt(page_height - y - height),
            width: Pt(width),
            height: Pt(height),
            mode: Some(PaintMode::Fill),
            winding_order: None,
        },
    });
}

// ---------------------------------------------------------------------------------------
// PDF/A-2b

/// Turns the generated PDF into PDF/A-2b: an sRGB output intent, the XMP identification and
/// the removal of the PDF/X info key printpdf writes unconditionally.
fn archival_pdf(bytes: Vec<u8>, title: &str) -> Result<Vec<u8>, PdfError> {
    use lopdf::{Document as LoDocument, Object, Stream, dictionary};

    let mut document =
        LoDocument::load_mem(&bytes).map_err(|error| PdfError::Write(error.to_string()))?;
    document.version = "1.7".to_owned();

    let mut profile = Stream::new(
        dictionary! {
            "N" => 3,
            "Alternate" => Object::Name(b"DeviceRGB".to_vec()),
        },
        srgb_profile(),
    );
    let _ = profile.compress();
    let profile_id = document.add_object(profile);
    let output_intent = dictionary! {
        "Type" => Object::Name(b"OutputIntent".to_vec()),
        "S" => Object::Name(b"GTS_PDFA1".to_vec()),
        "OutputConditionIdentifier" => Object::string_literal("sRGB IEC61966-2.1"),
        "Info" => Object::string_literal("sRGB IEC61966-2.1"),
        "RegistryName" => Object::string_literal("http://www.color.org"),
        "DestOutputProfile" => Object::Reference(profile_id),
    };
    let intent_id = document.add_object(output_intent);
    let xmp = xmp_packet(title);
    let metadata_id = document.add_object(Stream::new(
        dictionary! {
            "Type" => Object::Name(b"Metadata".to_vec()),
            "Subtype" => Object::Name(b"XML".to_vec()),
        },
        xmp.into_bytes(),
    ));
    if let Ok(catalog) = document.catalog_mut() {
        catalog.set(
            "OutputIntents",
            Object::Array(vec![Object::Reference(intent_id)]),
        );
        catalog.set("Metadata", Object::Reference(metadata_id));
        catalog.set("Lang", Object::string_literal("en-GB"));
    }
    // PDF/X's key is not allowed in PDF/A.
    if let Ok(Object::Reference(info_id)) = document.trailer.get(b"Info")
        && let Some(Object::Dictionary(info)) = document.objects.get_mut(info_id)
    {
        info.remove(b"GTS_PDFXVersion");
    }

    let mut out = Vec::new();
    document
        .save_to(&mut out)
        .map_err(|error| PdfError::Write(error.to_string()))?;
    Ok(out)
}

/// The XMP identification PDF/A readers check.
fn xmp_packet(title: &str) -> String {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    let title = xml_escape(title);
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n\
 <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
  <rdf:Description rdf:about=\"\"\n\
    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"\n\
    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n\
    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n\
    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n\
   <pdfaid:part>2</pdfaid:part>\n\
   <pdfaid:conformance>B</pdfaid:conformance>\n\
   <dc:format>application/pdf</dc:format>\n\
   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{title}</rdf:li></rdf:Alt></dc:title>\n\
   <dc:creator><rdf:Seq><rdf:li>Libertum</rdf:li></rdf:Seq></dc:creator>\n\
   <xmp:CreatorTool>Libertum</xmp:CreatorTool>\n\
   <xmp:CreateDate>{now}</xmp:CreateDate>\n\
   <xmp:ModifyDate>{now}</xmp:ModifyDate>\n\
   <xmp:MetadataDate>{now}</xmp:MetadataDate>\n\
   <pdf:Producer>Libertum</pdf:Producer>\n\
  </rdf:Description>\n\
 </rdf:RDF>\n\
</x:xmpmeta>\n\
<?xpacket end=\"w\"?>"
    )
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A minimal sRGB profile (ICC v2, matrix/TRC) for the output intent. Written here rather
/// than shipped as a third-party binary: the header, tag table and nine tags below are the
/// whole profile, and no external file or licence is involved.
fn srgb_profile() -> Vec<u8> {
    let tags: [(&[u8; 4], Vec<u8>); 9] = [
        (b"desc", tag_description("Libertum sRGB")),
        (
            b"cprt",
            tag_text("Public domain sRGB profile generated for Libertum"),
        ),
        (b"wtpt", tag_xyz(0.964_2, 1.0, 0.824_9)),
        (b"rXYZ", tag_xyz(0.436_07, 0.222_49, 0.013_92)),
        (b"gXYZ", tag_xyz(0.385_15, 0.716_87, 0.097_08)),
        (b"bXYZ", tag_xyz(0.143_07, 0.060_61, 0.714_10)),
        (b"rTRC", tag_gamma(2.2)),
        (b"gTRC", tag_gamma(2.2)),
        (b"bTRC", tag_gamma(2.2)),
    ];
    let table_start = 128 + 4;
    let mut offsets = Vec::with_capacity(tags.len());
    let mut cursor = table_start + tags.len() * 12;
    for (_, data) in &tags {
        offsets.push(cursor);
        cursor += (data.len() + 3) & !3;
    }
    let total = cursor;
    let mut out = vec![0u8; table_start + tags.len() * 12];
    out[0..4].copy_from_slice(&(total as u32).to_be_bytes());
    out[8..12].copy_from_slice(&0x0210_0000u32.to_be_bytes());
    out[12..16].copy_from_slice(b"mntr");
    out[16..20].copy_from_slice(b"RGB ");
    out[20..24].copy_from_slice(b"XYZ ");
    // 2026-01-01 00:00:00.
    out[24..26].copy_from_slice(&2026u16.to_be_bytes());
    out[26..28].copy_from_slice(&1u16.to_be_bytes());
    out[28..30].copy_from_slice(&1u16.to_be_bytes());
    out[36..40].copy_from_slice(b"acsp");
    out[68..72].copy_from_slice(&0x0000_F6D6u32.to_be_bytes());
    out[72..76].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    out[76..80].copy_from_slice(&0x0000_D32Du32.to_be_bytes());
    out[128..132].copy_from_slice(&(tags.len() as u32).to_be_bytes());
    for (index, ((signature, data), offset)) in tags.iter().zip(offsets).enumerate() {
        let at = table_start + index * 12;
        out[at..at + 4].copy_from_slice(*signature);
        out[at + 4..at + 8].copy_from_slice(&(offset as u32).to_be_bytes());
        out[at + 8..at + 12].copy_from_slice(&(data.len() as u32).to_be_bytes());
        out.resize(offset + data.len(), 0);
        out[offset..offset + data.len()].copy_from_slice(data);
    }
    out.resize(total, 0);
    out
}

fn tag_header(kind: &[u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(kind);
    out.extend_from_slice(&[0, 0, 0, 0]);
    out
}

fn tag_xyz(x: f32, y: f32, z: f32) -> Vec<u8> {
    let mut out = tag_header(b"XYZ ");
    for value in [x, y, z] {
        out.extend_from_slice(&((value * 65_536.0).round() as i32).to_be_bytes());
    }
    out
}

fn tag_gamma(gamma: f32) -> Vec<u8> {
    let mut out = tag_header(b"curv");
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&((gamma * 256.0).round() as u16).to_be_bytes());
    out
}

fn tag_text(text: &str) -> Vec<u8> {
    let mut out = tag_header(b"text");
    out.extend_from_slice(text.as_bytes());
    out.push(0);
    out
}

fn tag_description(text: &str) -> Vec<u8> {
    let mut out = tag_header(b"desc");
    let ascii = text.as_bytes();
    out.extend_from_slice(&((ascii.len() + 1) as u32).to_be_bytes());
    out.extend_from_slice(ascii);
    out.push(0);
    out.extend_from_slice(&0u32.to_be_bytes()); // Unicode language code.
    out.extend_from_slice(&0u32.to_be_bytes()); // Unicode count.
    out.extend_from_slice(&0u16.to_be_bytes()); // ScriptCode code.
    out.push(0); // ScriptCode count.
    out.extend_from_slice(&[0u8; 67]); // ScriptCode characters.
    out
}

/// Table cell fills and borders of `tables`, under the text, on a page `page_height` high
/// whose margins are those of `geometry`.
fn table_rects<'a>(
    ops: &mut Vec<Op>,
    tables: impl Iterator<Item = &'a libertum_layout::TableDraw>,
    geometry: &PageGeometry,
    page_height: f32,
) {
    for table in tables {
        let top = geometry.margins.top + table.y;
        for cell in &table.cells {
            if let Some(fill) = cell.fill {
                rect(
                    ops,
                    geometry.margins.left + cell.x,
                    top,
                    cell.width,
                    table.height,
                    fill,
                    page_height,
                );
            }
        }
        if table.borders.is_visible() {
            let colour = table.borders.colour;
            let line = table.borders.width_pt.max(0.5);
            let x = geometry.margins.left + table.x;
            for cell in table.cells.iter().filter(|cell| cell.top) {
                rect(
                    ops,
                    geometry.margins.left + cell.x,
                    top,
                    cell.width,
                    line,
                    colour,
                    page_height,
                );
            }
            if table.bottom {
                rect(
                    ops,
                    x,
                    top + table.height - line,
                    table.width,
                    line,
                    colour,
                    page_height,
                );
            }
            if table.sides {
                rect(ops, x, top, line, table.height, colour, page_height);
                rect(
                    ops,
                    x + table.width - line,
                    top,
                    line,
                    table.height,
                    colour,
                    page_height,
                );
            }
            for cell in table.cells.iter().filter(|cell| cell.left) {
                rect(
                    ops,
                    geometry.margins.left + cell.x,
                    top,
                    line,
                    table.height,
                    colour,
                    page_height,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
