//! Text watermarks in DOCX.
//!
//! Word draws a watermark as a VML WordArt shape (`PowerPlusWaterMarkObject…`) inside the
//! header of every page it appears on. Libertum keeps headers as opaque parts, so the shape is
//! read out of them on open and written back into them on save:
//!
//! - **Reading** looks through the headers each section refers to for such a shape and
//!   returns its text and whether it is turned (diagonal) or level.
//! - **Writing** first removes any watermark shape already in those headers (so a changed or
//!   removed watermark never leaves the old one behind, and nothing is doubled), then adds the
//!   model's watermark to the default header, to the first-page header when the section has
//!   one, and to the even-page header when it has one. A section with no header part for a
//!   page kind that needs one gets a new, otherwise empty one.

use std::collections::HashSet;

use libertum_model::{Document, Section, Watermark};

use crate::package::Package;
use crate::write::{
    CT_HEADER, R_NS, REL_NS, REL_TYPE, W_NS, XML_DECL, ensure_content_type, paper_points,
};
use crate::xml::{self, XmlNode, escape_attr};
use crate::{Extras, SectionExtras};

/// Part of the shape's `id` that marks it as a watermark, as Word and LibreOffice both name it.
const MARKER: &str = "PowerPlusWaterMarkObject";

const VML_NS: &str = "urn:schemas-microsoft-com:vml";
const OFFICE_NS: &str = "urn:schemas-microsoft-com:office:office";
const WORD10_NS: &str = "urn:schemas-microsoft-com:office:word";

/// The watermark of a document just read: the first one found in a section's headers. The app
/// keeps one watermark for the whole document, so it applies to every section.
pub(crate) fn read(package: &Package, extras: &Extras) -> Option<Watermark> {
    let relationships = package.document_relationships();
    for extra in extras {
        for (name, attrs) in &extra.elements {
            if name != "headerReference" {
                continue;
            }
            let Some(id) = attr(attrs, "id") else {
                continue;
            };
            let Some(part) = relationships
                .iter()
                .find(|relationship| relationship.id == id)
                .and_then(|relationship| relationship.part.as_deref())
            else {
                continue;
            };
            let Some(text) = package.text(part) else {
                continue;
            };
            let Ok(root) = xml::parse(&text, part) else {
                continue;
            };
            if let Some(found) = find_shape(&root) {
                return Some(found);
            }
        }
    }
    None
}

fn find_shape(node: &XmlNode) -> Option<Watermark> {
    if node.name == "shape" && node.attr("id").is_some_and(|id| id.contains(MARKER)) {
        let text = node.child("textpath")?.attr("string")?;
        let rotated = node
            .attr("style")
            .and_then(rotation)
            .is_some_and(|degrees| degrees.rem_euclid(360.0).abs() > 0.5);
        return Watermark::new(text, rotated);
    }
    node.elements().find_map(find_shape)
}

/// The `rotation:` value of a VML style string, in degrees.
fn rotation(style: &str) -> Option<f32> {
    let start = style.find("rotation:")? + "rotation:".len();
    let number: String = style[start..]
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.'))
        .collect();
    number.parse().ok()
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// What a save must add to the package for the watermarks.
#[derive(Default)]
pub(crate) struct Edits {
    /// Header parts to write: their name and new text.
    parts: Vec<(String, String)>,
    /// Relationships from `document.xml` to header parts created here: id and part name.
    relationships: Vec<(String, String)>,
}

impl Edits {
    /// Writes the edits into the package.
    pub fn apply(self, package: &mut Package) {
        for (name, text) in self.parts {
            if !package.contains(&name) {
                ensure_content_type(package, &name, CT_HEADER);
            }
            package.set_text(&name, text);
        }
        for (id, part) in self.relationships {
            add_relationship(package, &id, &part);
        }
    }
}

/// Works out the header edits for the document's watermarks. Header references for headers
/// created here are added to `extras`, which `sectPr` is written from.
pub(crate) fn plan(document: &Document, extras: &mut Extras, origin: Option<&Package>) -> Edits {
    let mut edits = Edits::default();
    if extras.len() < document.sections.len() {
        extras.resize_with(document.sections.len(), SectionExtras::default);
    }
    let relationships = origin
        .map(Package::document_relationships)
        .unwrap_or_default();
    let mut handled: HashSet<String> = HashSet::new();
    let mut created = 0usize;

    for (index, section) in document.sections.iter().enumerate() {
        let extra = &mut extras[index];
        let watermark = section.watermark.as_ref();
        let mut kinds: Vec<String> = Vec::new();

        // Headers this section already refers to: take the old watermark out, put the
        // current one in.
        let references: Vec<(String, String)> = extra
            .elements
            .iter()
            .filter(|(name, _)| name == "headerReference")
            .filter_map(|(_, attrs)| {
                Some((
                    attr(attrs, "type").unwrap_or("default").to_owned(),
                    attr(attrs, "id")?.to_owned(),
                ))
            })
            .collect();
        for (kind, id) in references {
            kinds.push(kind);
            let Some(part) = relationships
                .iter()
                .find(|relationship| relationship.id == id)
                .and_then(|relationship| relationship.part.clone())
            else {
                continue;
            };
            if !handled.insert(part.clone()) {
                continue;
            }
            let Some(text) = origin.and_then(|package| package.text(&part)) else {
                continue;
            };
            let stripped = strip(&text);
            let edited = match watermark {
                Some(watermark) => inject(&stripped, &shape(watermark, section, index)),
                None => stripped,
            };
            if edited != text {
                edits.parts.push((part, edited));
            }
        }

        // Page kinds that show a watermark but have no header of their own to carry it.
        let Some(watermark) = watermark else {
            continue;
        };
        let first_page = extra.elements.iter().any(|(name, _)| name == "titlePg")
            || section.headers.different_first;
        let mut wanted = vec!["default"];
        if first_page {
            wanted.push("first");
        }
        for kind in wanted {
            if kinds.iter().any(|have| have == kind) {
                continue;
            }
            created += 1;
            let part = unused_part_name(origin, &edits, created);
            let id = format!("rIdLbtWm{created}");
            let run = shape(watermark, section, index * 4 + created);
            edits.parts.push((part.clone(), new_header(&run)));
            edits.relationships.push((id.clone(), part));
            extra.elements.insert(
                0,
                (
                    "headerReference".to_owned(),
                    vec![("type".to_owned(), kind.to_owned()), ("id".to_owned(), id)],
                ),
            );
        }
    }
    edits
}

fn unused_part_name(origin: Option<&Package>, edits: &Edits, from: usize) -> String {
    (from..)
        .map(|number| format!("word/header{number}.xml"))
        .find(|name| {
            !origin.is_some_and(|package| package.contains(name))
                && !edits.parts.iter().any(|(part, _)| part == name)
        })
        .unwrap_or_else(|| "word/header-watermark.xml".to_owned())
}

fn new_header(run: &str) -> String {
    format!("{XML_DECL}<w:hdr xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:p>{run}</w:p></w:hdr>")
}

fn add_relationship(package: &mut Package, id: &str, part: &str) {
    const NAME: &str = "word/_rels/document.xml.rels";
    let target = part.strip_prefix("word/").unwrap_or(part);
    let entry = format!(
        "<Relationship Id=\"{id}\" Type=\"{REL_TYPE}/header\" Target=\"{target}\"/></Relationships>"
    );
    match package.text(NAME) {
        Some(text) if text.contains(&format!("Id=\"{id}\"")) => {}
        Some(text) => package.set_text(NAME, text.replacen("</Relationships>", &entry, 1)),
        None => package.set_text(
            NAME,
            format!("{XML_DECL}<Relationships xmlns=\"{REL_NS}\">{}", entry),
        ),
    }
}

/// The run holding the VML watermark shape, as Word writes it.
fn shape(watermark: &Watermark, section: &Section, salt: usize) -> String {
    let (page_width, page_height) = paper_points(section.page);
    let chars = watermark.text.chars().count().max(1) as f32;
    // WordArt stretches the text to fill the shape, so the shape's proportions must be the
    // text's: about 0.8 em per character on a line one em tall.
    let (span, rotation) = if watermark.diagonal {
        let angle = page_height.atan2(page_width).to_degrees();
        (
            (page_width * page_width + page_height * page_height).sqrt() * 0.62,
            format!("rotation:{:.0};", 360.0 - angle),
        )
    } else {
        (page_width * 0.8, String::new())
    };
    let width = span.min(chars * 0.8 * 160.0).max(24.0);
    let height = (width / (chars * 0.8)).max(6.0);
    let unique = 357_476_642 + salt;
    format!(
        "<w:r><w:rPr><w:noProof/></w:rPr>\
<w:pict xmlns:v=\"{VML_NS}\" xmlns:o=\"{OFFICE_NS}\" xmlns:w10=\"{WORD10_NS}\">\
<v:shapetype id=\"_x0000_t136\" coordsize=\"21600,21600\" o:spt=\"136\" adj=\"10800\" path=\"m@7,l@8,m@5,21600l@6,21600e\">\
<v:formulas><v:f eqn=\"sum #0 0 10800\"/><v:f eqn=\"prod #0 2 1\"/><v:f eqn=\"sum 21600 0 @1\"/><v:f eqn=\"sum 0 0 @2\"/><v:f eqn=\"sum 21600 0 @3\"/><v:f eqn=\"if @0 @3 0\"/><v:f eqn=\"if @0 21600 @1\"/><v:f eqn=\"if @0 0 @2\"/><v:f eqn=\"if @0 @4 21600\"/><v:f eqn=\"mid @5 @6\"/><v:f eqn=\"mid @8 @5\"/><v:f eqn=\"mid @7 @8\"/><v:f eqn=\"mid @6 @7\"/><v:f eqn=\"sum @6 0 @5\"/></v:formulas>\
<v:path textpathok=\"t\" o:connecttype=\"custom\" o:connectlocs=\"@9,0;@10,10800;@11,21600;@12,10800\" o:connectangles=\"270,180,90,0\"/>\
<v:textpath on=\"t\" fitshape=\"t\"/>\
<v:handles><v:h position=\"#0,bottomRight\" xrange=\"6629,14971\"/></v:handles>\
<o:lock v:ext=\"edit\" text=\"t\" shapetype=\"t\"/></v:shapetype>\
<v:shape id=\"{MARKER}{unique}\" o:spid=\"_x0000_s{spid}\" type=\"#_x0000_t136\" \
style=\"position:absolute;margin-left:0;margin-top:0;width:{width:.2}pt;height:{height:.2}pt;{rotation}z-index:-251658752;mso-position-horizontal:center;mso-position-horizontal-relative:margin;mso-position-vertical:center;mso-position-vertical-relative:margin\" \
o:allowincell=\"f\" fillcolor=\"#808080\" stroked=\"f\">\
<v:fill opacity=\".31\"/>\
<v:textpath style=\"font-family:&quot;Calibri&quot;;font-size:1pt\" string=\"{text}\"/>\
<w10:wrap anchorx=\"margin\" anchory=\"margin\"/></v:shape></w:pict></w:r>",
        spid = 2049 + salt,
        text = escape_attr(&watermark.text),
    )
}

/// `xml` with every watermark run taken out.
fn strip(xml: &str) -> String {
    let mut text = xml.to_owned();
    while let Some(at) = text.find(MARKER) {
        let start = [text[..at].rfind("<w:r>"), text[..at].rfind("<w:r ")]
            .into_iter()
            .flatten()
            .max();
        let end = text[at..]
            .find("</w:r>")
            .map(|offset| at + offset + "</w:r>".len());
        match (start, end) {
            (Some(start), Some(end)) => text.replace_range(start..end, ""),
            // A shape outside a run: leave it rather than cut something else.
            _ => break,
        }
    }
    text
}

/// `xml` with `run` added at the start of the header's first paragraph.
fn inject(xml: &str, run: &str) -> String {
    let paragraph = xml.match_indices("<w:p").find(|(at, tag)| {
        xml[at + tag.len()..]
            .chars()
            .next()
            .is_some_and(|next| matches!(next, '>' | ' ' | '/'))
    });
    if let Some((start, _)) = paragraph {
        if let Some(close) = xml[start..].find('>').map(|offset| start + offset) {
            if xml[..close].ends_with('/') {
                // `<w:p/>`: open it up around the run.
                let open = &xml[start..close - 1];
                return format!("{}{}>{run}</w:p>{}", &xml[..start], open, &xml[close + 1..]);
            }
            let mut at = close + 1;
            // Properties come first in a paragraph.
            let rest = &xml[at..];
            if rest.starts_with("<w:pPr/>") {
                at += "<w:pPr/>".len();
            } else if rest.starts_with("<w:pPr>") || rest.starts_with("<w:pPr ") {
                if let Some(end) = rest.find("</w:pPr>") {
                    at += end + "</w:pPr>".len();
                }
            }
            return format!("{}{run}{}", &xml[..at], &xml[at..]);
        }
    }
    match xml.rfind("</w:hdr>") {
        Some(end) => format!("{}<w:p>{run}</w:p>{}", &xml[..end], &xml[end..]),
        None => xml.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_removes_only_the_watermark_run() {
        let header = "<w:hdr><w:p><w:r><w:t>Title</w:t></w:r><w:r><w:pict><v:shape id=\"PowerPlusWaterMarkObject1\"/></w:pict></w:r></w:p></w:hdr>";
        assert_eq!(
            strip(header),
            "<w:hdr><w:p><w:r><w:t>Title</w:t></w:r></w:p></w:hdr>"
        );
    }

    #[test]
    fn inject_goes_after_paragraph_properties_and_handles_empty_paragraphs() {
        assert_eq!(
            inject(
                "<w:hdr><w:p><w:pPr><w:jc/></w:pPr><w:r/></w:p></w:hdr>",
                "RUN"
            ),
            "<w:hdr><w:p><w:pPr><w:jc/></w:pPr>RUN<w:r/></w:p></w:hdr>"
        );
        assert_eq!(
            inject("<w:hdr><w:p/></w:hdr>", "RUN"),
            "<w:hdr><w:p>RUN</w:p></w:hdr>"
        );
        assert_eq!(
            inject("<w:hdr></w:hdr>", "RUN"),
            "<w:hdr><w:p>RUN</w:p></w:hdr>"
        );
    }
}
