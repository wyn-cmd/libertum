//! Searchable-text PDF export.
//!
//! The pages come from the same [`LayoutState`] the screen uses, so pagination, tables,
//! headers, footers, page numbers and TOC leaders land where they do on screen. Text is
//! written as real glyph IDs from parley, not as images: printpdf embeds (and subsets) the
//! fonts, builds a `ToUnicode` map, and therefore the text can be selected, searched and
//! copied. The archival variant adds the sRGB output intent and XMP metadata PDF/A-2b needs.

use std::collections::{BTreeMap, HashMap};

use libertum_layout::parley::PositionedLayoutItem;
use libertum_layout::{Decoration, LayoutOptions, LayoutState, PageGeometry, ParagraphLayout};
use libertum_model::Document;
use printpdf::{
    Actions, BorderArray, Codepoint, Color, Destination, FontId, LinkAnnotation, Mm, Op, PaintMode,
    ParsedFont, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, PdfWarnMsg, Point, Pt,
    RawImage, RawImageData, RawImageFormat, Rect, Rgb, TextItem, TextMatrix, TextRenderingMode,
    XObjectId, XObjectTransform,
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
        images: HashMap::new(),
    };
    let mut pages = Vec::with_capacity(layout.page_count());
    for index in 0..layout.page_count() {
        let geometry = layout.page_geometry(index);
        let mut ops = Vec::new();
        exporter.page(&layout, index, &geometry, &mut ops)?;
        pages.push(PdfPage::new(
            Mm::from(Pt(geometry.width_pt)),
            Mm::from(Pt(geometry.height_pt)),
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
    images: HashMap<u64, XObjectId>,
}

impl Exporter<'_> {
    fn font(&mut self, data: &[u8], data_id: u64, index: u32) -> Option<&FontInfo> {
        let key = (data_id, index);
        if !self.fonts.contains_key(&key) {
            let mut warnings = Vec::new();
            let parsed = ParsedFont::from_bytes(data, index as usize, &mut warnings)?;
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
        ops: &mut Vec<Op>,
    ) -> Result<(), PdfError> {
        let Some(page) = layout.pages().get(page_index) else {
            return Ok(());
        };
        let page_height = geometry.height_pt;

        // A page colour fills the whole sheet, under everything else (as the editor shows it).
        if let Some(colour) = layout.page_colour(page_index) {
            rect(
                ops,
                0.0,
                0.0,
                geometry.width_pt,
                page_height,
                colour,
                page_height,
            );
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

        // Table cell fills and borders, under the text.
        for table in &page.tables {
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

        // Body and table-cell paragraphs.
        for slice in &page.slices {
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
            ops.push(Op::LinkAnnotation {
                link: LinkAnnotation::new(
                    Rect {
                        x: Pt(link.x),
                        y: Pt(page_height - (link.y + link.height)),
                        width: Pt(link.width),
                        height: Pt(link.height),
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

        // Page numbers, TOC entries, headers and footers, inline images and rules.
        for decoration in layout.page_decorations(page_index) {
            match decoration {
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
                let Some(info) = self.font(font.data.data(), font.data.id(), font.index) else {
                    return Err(PdfError::Font(format!("data {}", font.data.id())));
                };
                let positions: Vec<_> = run.positioned_glyphs().collect();
                let Some(first) = positions.first() else {
                    continue;
                };
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
                let mut glyphs = Vec::with_capacity(positions.len());
                for (at, glyph) in positions.iter().enumerate() {
                    let gid = glyph.id.min(u32::from(u16::MAX)) as u16;
                    let natural = info.widths.get(&gid).copied().unwrap_or(0) as f32 * size
                        / f32::from(info.units_per_em);
                    let desired = positions
                        .get(at + 1)
                        .map_or(natural, |next| next.x - glyph.x);
                    let offset = (natural - desired) * 1000.0 / size.max(0.01);
                    glyphs.push(match unicode.get(&gid) {
                        Some(text) => Codepoint::with_cid(gid, offset, text.clone()),
                        None => Codepoint::new(gid, offset),
                    });
                }
                let font_id = info.id.clone();
                let colour = turned.map_or_else(|| blend(brush.color), |t| t.colour.clone());
                let bold = text_run.synthesis().embolden();
                let baseline = dy + first.y - rise;
                ops.push(Op::StartTextSection);
                ops.push(Op::SetFillColor {
                    col: colour.clone(),
                });
                ops.push(Op::SetFont {
                    font: PdfFontHandle::External(font_id),
                    size: Pt(size),
                });
                if bold {
                    ops.push(Op::SetTextRenderingMode {
                        mode: TextRenderingMode::FillStroke,
                    });
                    ops.push(Op::SetOutlineColor { col: colour });
                    ops.push(Op::SetOutlineThickness {
                        pt: Pt(size * 0.03),
                    });
                } else {
                    ops.push(Op::SetTextRenderingMode {
                        mode: TextRenderingMode::Fill,
                    });
                }
                match turned {
                    None => ops.push(Op::SetTextCursor {
                        pos: Point {
                            x: Pt(dx + first.x),
                            y: Pt(page_height - baseline),
                        },
                    }),
                    Some(turn) => {
                        // Turn the baseline's start about the page's centre; the matrix
                        // turns the glyphs the same way (PDF's y axis points up).
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
                ops.push(Op::ShowText {
                    items: vec![TextItem::GlyphIds(glyphs)],
                });
                ops.push(Op::EndTextSection);

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

#[cfg(test)]
mod tests;
