//! Reading a table of contents out of a Word document.
//!
//! Word (and LibreOffice, saving as DOCX) write a table of contents as a content control
//! (`w:sdt`, gallery "Table of Contents") holding a `TOC` field whose result is one paragraph
//! per entry, each with a hyperlink and a `PAGEREF` field for its page number. Older files and
//! some converters write the field without the control. Either way the entries are generated
//! text, so the reader does not keep them: it keeps the title and marks the place, and the
//! editor rebuilds the entries from the headings (see `libertum-formats-common::toc`).

use libertum_formats_common::toc::LIBERTUM_TOC_TAG;

use crate::xml::XmlNode;

/// The field markers inside a node and its descendants, in document order.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldMarks {
    pub begins: i32,
    pub ends: i32,
    /// The text of the first `w:instrText` found.
    pub first_instruction: Option<String>,
}

pub(crate) fn field_marks(node: &XmlNode) -> FieldMarks {
    let mut marks = FieldMarks::default();
    walk(node, &mut marks);
    marks
}

fn walk(node: &XmlNode, marks: &mut FieldMarks) {
    match node.name.as_str() {
        "fldChar" => match node.attr("fldCharType") {
            Some("begin") => marks.begins += 1,
            Some("end") => marks.ends += 1,
            _ => {}
        },
        "instrText" if marks.first_instruction.is_none() => {
            marks.first_instruction = Some(node.text().into_owned());
        }
        _ => {}
    }
    for child in node.elements() {
        walk(child, marks);
    }
}

/// True when `instruction` is a `TOC` field Libertum can stand in for: a contents list built
/// from the headings. A list built from other things (`\c` captions and tables of figures,
/// `\t` custom styles, `\f` entry fields, `\b` a bookmark's range, `\a` and `\s` and `\d`)
/// has no equivalent in the model, so it is not one.
pub(crate) fn is_supported_toc(instruction: &str) -> bool {
    let mut words = instruction.split_whitespace();
    if !words
        .next()
        .is_some_and(|word| word.eq_ignore_ascii_case("TOC"))
    {
        return false;
    }
    words.all(|word| {
        let Some(switch) = word.strip_prefix('\\') else {
            return true;
        };
        matches!(
            switch.chars().next().map(|c| c.to_ascii_lowercase()),
            Some('o' | 'u' | 'h' | 'z' | 'n' | 'p' | 'w' | 'x')
        )
    })
}

/// True when the content control is Word's "Table of Contents" gallery entry.
pub(crate) fn is_toc_control(sdt: &XmlNode) -> bool {
    sdt.child("sdtPr")
        .and_then(|pr| pr.child("docPartObj"))
        .and_then(|obj| obj.child("docPartGallery"))
        .and_then(|gallery| gallery.attr("val"))
        .is_some_and(|value| value.eq_ignore_ascii_case("Table of Contents"))
}

/// True when Libertum wrote this content control itself.
pub(crate) fn is_ours(sdt: &XmlNode) -> bool {
    sdt.child("sdtPr")
        .and_then(|pr| pr.child("tag"))
        .and_then(|tag| tag.attr("val"))
        == Some(LIBERTUM_TOC_TAG)
}

/// The paragraphs of a table-of-contents control, split into the title (the first paragraph
/// before the field starts, if there is one) and whether the control holds a `TOC` field
/// Libertum can stand in for.
pub(crate) fn split_control(sdt: &XmlNode) -> (Option<&XmlNode>, bool) {
    let Some(content) = sdt.child("sdtContent") else {
        return (None, false);
    };
    let mut title = None;
    let mut supported = false;
    for paragraph in content.children_named("p") {
        let marks = field_marks(paragraph);
        match &marks.first_instruction {
            Some(instruction) if is_supported_toc(instruction) => {
                supported = true;
                break;
            }
            Some(_) => break,
            None if marks.begins == 0 && title.is_none() => title = Some(paragraph),
            None => {}
        }
    }
    (title, supported)
}

/// The title above a table of contents that came without one (a bare field), so the entries
/// the editor builds always have a paragraph above them to keep apart from the headings.
pub(crate) fn empty_title(id: libertum_model::BlockId) -> libertum_model::Paragraph {
    libertum_model::Paragraph::new(id, "").with_props(libertum_model::ParaProps {
        style: libertum_model::StyleId::TITLE,
        ..libertum_model::ParaProps::default()
    })
}

/// The single placeholder entry that marks where a table of contents goes. The editor
/// replaces it with one entry per heading when the document is opened.
pub(crate) fn marker(id: libertum_model::BlockId) -> libertum_model::Paragraph {
    libertum_model::Paragraph::new(id, "").with_props(libertum_model::ParaProps {
        toc_target: Some(libertum_model::BlockId(0)),
        ..libertum_model::ParaProps::default()
    })
}
