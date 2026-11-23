//! Headers and footers in DOCX.
//!
//! The model keeps one plain paragraph of text per header or footer variant (default, first
//! page, even pages), with `{page}` and `{pages}` standing for the `PAGE` and `NUMPAGES`
//! fields. Word's headers can hold far more (fonts, pictures, tables), so the original parts
//! stay in the package untouched:
//!
//! - **Reading** pulls each header's and footer's text, alignment and page fields out of the
//!   parts a section refers to, so the app and the PDF export show them. A section with no
//!   reference of its own for a variant inherits the previous section's, as in Word.
//! - **Writing** compares the model with what reading the original package would give. A
//!   variant that has not changed keeps its original part, rich content and all; one that was
//!   edited (or has no part yet) gets a new plain part, and the section's reference is
//!   pointed at it.
//! - `evenAndOddHeaders` in `settings.xml` is the document-wide even-page switch.

use libertum_model::{Align, HeaderFooter, HeadersFooters, HfKind, HfVariant, Section};

use crate::package::Package;
use crate::watermark::Edits;
use crate::write::{CT_SETTINGS, R_NS, W_NS, XML_DECL, ensure_content_type};
use crate::xml::{self, XmlNode, escape_text};
use crate::{Extras, SectionExtras};

const TOKENS: [(&str, &str); 2] = [("{pages}", " NUMPAGES "), ("{page}", " PAGE ")];

fn variant_name(variant: HfVariant) -> &'static str {
    match variant {
        HfVariant::Default => "default",
        HfVariant::First => "first",
        HfVariant::Even => "even",
    }
}

fn reference_name(kind: HfKind) -> &'static str {
    match kind {
        HfKind::Header => "headerReference",
        HfKind::Footer => "footerReference",
    }
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Whether the package turns on separate even-page headers.
fn even_and_odd(package: &Package) -> bool {
    package
        .related_part("settings", "word/settings.xml")
        .and_then(|part| package.text(&part))
        .is_some_and(|text| text.contains("evenAndOddHeaders"))
}

/// The headers and footers of every section, as the package describes them.
fn model(package: &Package, extras: &Extras, sections: usize) -> Vec<[HeadersFooters; 2]> {
    let relationships = package.document_relationships();
    let even = even_and_odd(package);
    let mut all: Vec<[HeadersFooters; 2]> = Vec::with_capacity(sections);
    for index in 0..sections {
        let mut pair = all.last().cloned().unwrap_or_default();
        for (slot, kind) in [HfKind::Header, HfKind::Footer].into_iter().enumerate() {
            let set = &mut pair[slot];
            set.different_even_odd = even;
            // First-page and even-page variants are the section's own; only the default is
            // inherited whole, which is how Word links sections.
            let Some(extra) = extras.get(index) else {
                continue;
            };
            for (name, attrs) in &extra.elements {
                if name != reference_name(kind) {
                    continue;
                }
                let variant = match attr(attrs, "type") {
                    Some("first") => HfVariant::First,
                    Some("even") => HfVariant::Even,
                    _ => HfVariant::Default,
                };
                let found = attr(attrs, "id")
                    .and_then(|id| relationships.iter().find(|rel| rel.id == id))
                    .and_then(|rel| rel.part.as_deref())
                    .and_then(|part| Some((part, package.text(part)?)))
                    .and_then(|(part, text)| xml::parse(&text, part).ok())
                    .map(|root| extract(&root));
                if let Some(found) = found {
                    *set.get_mut(variant) = Some(found);
                }
            }
        }
        all.push(pair);
    }
    all
}

/// Fills the sections' headers and footers from the package.
pub(crate) fn read(package: &Package, extras: &Extras, sections: &mut [Section]) {
    let found = model(package, extras, sections.len());
    for (section, [headers, footers]) in sections.iter_mut().zip(found) {
        let (first_header, first_footer) = (
            section.headers.different_first,
            section.footers.different_first,
        );
        section.headers = headers;
        section.footers = footers;
        section.headers.different_first = first_header;
        section.footers.different_first = first_footer;
    }
}

/// The text, alignment and page fields of a header or footer part's paragraphs.
fn extract(root: &XmlNode) -> HeaderFooter {
    let mut text = String::new();
    let mut align = None;
    for paragraph in root.children_named("p") {
        let mut line = String::new();
        let mut field = FieldState::default();
        collect(paragraph, &mut line, &mut field);
        if line.trim().is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(line.trim());
        if align.is_none() {
            align = paragraph
                .child("pPr")
                .and_then(|properties| properties.child("jc"))
                .and_then(|jc| jc.attr("val"))
                .map(|value| match value {
                    "center" => Align::Centre,
                    "right" | "end" => Align::End,
                    "both" | "distribute" => Align::Justify,
                    _ => Align::Start,
                });
        }
    }
    HeaderFooter {
        text,
        align: align.unwrap_or_default(),
    }
}

/// Where a complex field is: after its `begin`, collecting the instruction, then skipping the
/// stored result up to `end`.
#[derive(Default)]
struct FieldState {
    instruction: Option<String>,
    skipping: bool,
}

fn collect(node: &XmlNode, out: &mut String, field: &mut FieldState) {
    for child in node.elements() {
        match child.name.as_str() {
            "t" if !field.skipping => out.push_str(&child.text()),
            "tab" if !field.skipping => out.push(' '),
            "instrText" => {
                if let Some(instruction) = field.instruction.as_mut() {
                    instruction.push_str(&child.text());
                }
            }
            "fldChar" => match child.attr("fldCharType") {
                Some("begin") => {
                    field.instruction = Some(String::new());
                    field.skipping = false;
                }
                Some("separate") => {
                    if let Some(instruction) = field.instruction.take() {
                        out.push_str(&token_for(&instruction));
                    }
                    field.skipping = true;
                }
                Some("end") => {
                    if let Some(instruction) = field.instruction.take() {
                        out.push_str(&token_for(&instruction));
                    }
                    field.skipping = false;
                }
                _ => {}
            },
            "fldSimple" => {
                if let Some(instruction) = child.attr("instr") {
                    out.push_str(&token_for(instruction));
                }
            }
            // A watermark or picture run has no text of its own; everything else is
            // searched for runs (hyperlinks, smart tags, content controls).
            _ => collect(child, out, field),
        }
    }
}

fn token_for(instruction: &str) -> String {
    let word = instruction.split_whitespace().next().unwrap_or("");
    match word.to_ascii_uppercase().as_str() {
        "PAGE" => "{page}".to_owned(),
        "NUMPAGES" => "{pages}".to_owned(),
        _ => String::new(),
    }
}

/// The paragraph XML of a header or footer.
fn paragraph(hf: &HeaderFooter) -> String {
    let jc = match hf.align {
        Align::Start => "",
        Align::Centre => "<w:pPr><w:jc w:val=\"center\"/></w:pPr>",
        Align::End => "<w:pPr><w:jc w:val=\"right\"/></w:pPr>",
        Align::Justify => "<w:pPr><w:jc w:val=\"both\"/></w:pPr>",
    };
    let mut runs = String::new();
    let mut rest = hf.text.as_str();
    loop {
        let next = TOKENS
            .iter()
            .filter_map(|(token, instruction)| Some((rest.find(token)?, token.len(), *instruction)))
            .min_by_key(|(at, _, _)| *at);
        let Some((at, length, instruction)) = next else {
            text_runs(rest, &mut runs);
            break;
        };
        text_runs(&rest[..at], &mut runs);
        runs.push_str(&format!(
            "<w:fldSimple w:instr=\"{instruction}\"><w:r><w:t>1</w:t></w:r></w:fldSimple>"
        ));
        rest = &rest[at + length..];
    }
    format!("<w:p>{jc}{runs}</w:p>")
}

fn text_runs(text: &str, out: &mut String) {
    for (index, piece) in text.split('\t').enumerate() {
        if index > 0 {
            out.push_str("<w:r><w:tab/></w:r>");
        }
        if !piece.is_empty() {
            out.push_str(&format!(
                "<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>",
                escape_text(piece)
            ));
        }
    }
}

/// A whole header or footer part.
fn part_xml(kind: HfKind, hf: Option<&HeaderFooter>) -> String {
    let root = match kind {
        HfKind::Header => "hdr",
        HfKind::Footer => "ftr",
    };
    let body = hf.map_or_else(|| "<w:p/>".to_owned(), paragraph);
    format!("{XML_DECL}<w:{root} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\">{body}</w:{root}>")
}

fn unused_part_name(origin: Option<&Package>, edits: &Edits, kind: HfKind, from: usize) -> String {
    let stem = match kind {
        HfKind::Header => "header",
        HfKind::Footer => "footer",
    };
    (from..)
        .map(|number| format!("word/{stem}{number}.xml"))
        .find(|name| !origin.is_some_and(|package| package.contains(name)) && !edits.has_part(name))
        .unwrap_or_else(|| format!("word/{stem}-libertum.xml"))
}

/// Works out the edits for the model's headers and footers, and points the sections'
/// references at the new parts.
pub(crate) fn plan(
    document: &libertum_model::Document,
    extras: &mut Extras,
    origin: Option<&Package>,
) -> Edits {
    let mut edits = Edits::default();
    if extras.len() < document.sections.len() {
        extras.resize_with(document.sections.len(), SectionExtras::default);
    }
    let baseline = origin.map_or_else(
        || vec![<[HeadersFooters; 2]>::default(); document.sections.len()],
        |package| model(package, extras, document.sections.len()),
    );
    let mut created = 0usize;
    for (index, section) in document.sections.iter().enumerate() {
        for (slot, kind) in [HfKind::Header, HfKind::Footer].into_iter().enumerate() {
            let (current, before) = match kind {
                HfKind::Header => (&section.headers, &baseline[index][slot]),
                HfKind::Footer => (&section.footers, &baseline[index][slot]),
            };
            for variant in HfVariant::ALL {
                let (now, was) = (current.get(variant), before.get(variant));
                if now == was {
                    continue;
                }
                let kind_name = variant_name(variant);
                let extra = &mut extras[index];
                extra.elements.retain(|(name, attrs)| {
                    !(name == reference_name(kind)
                        && attr(attrs, "type").unwrap_or("default") == kind_name)
                });
                // A variant taken away still needs an (empty) part where an earlier section
                // would otherwise be inherited.
                if now.is_none() && index == 0 {
                    continue;
                }
                created += 1;
                let part = unused_part_name(origin, &edits, kind, created);
                let id = format!("rIdLbtHf{created}");
                edits.add_part(part.clone(), part_xml(kind, now));
                edits.add_relationship(id.clone(), part);
                extra.elements.insert(
                    0,
                    (
                        reference_name(kind).to_owned(),
                        vec![
                            ("type".to_owned(), kind_name.to_owned()),
                            ("id".to_owned(), id),
                        ],
                    ),
                );
            }
        }
    }
    if document
        .sections
        .iter()
        .any(|section| section.headers.different_even_odd || section.footers.different_even_odd)
    {
        edits.require_even_and_odd();
    }
    edits
}

/// The document variable that holds a document's second language. Word has no setting for a
/// second proofing language of this kind, and a document variable is how a file carries a
/// value of its own without Word reading it as anything else.
const LANGUAGE2_VAR: &str = "LibertumLanguage2";

/// The second language written by [`set_language2`], from `settings.xml`.
pub(crate) fn read_language2(package: &Package) -> Option<String> {
    let name = package
        .related_part("settings", "word/settings.xml")
        .unwrap_or_else(|| "word/settings.xml".to_owned());
    let text = package.text(&name)?;
    let at = text.find(&format!("w:name=\"{LANGUAGE2_VAR}\""))?;
    let rest = &text[at..];
    let end = rest.find("/>")?;
    let value = rest[..end].split("w:val=\"").nth(1)?.split('"').next()?;
    libertum_model::language_tag(value)
}

/// Writes (or, with `None`, removes) the second language as a document variable in
/// `settings.xml`, creating the part when there is none.
pub(crate) fn set_language2(package: &mut Package, tag: Option<&str>) {
    const NAME: &str = "word/settings.xml";
    let name = package
        .related_part("settings", NAME)
        .unwrap_or_else(|| NAME.to_owned());
    let var = tag.map(|tag| format!("<w:docVar w:name=\"{LANGUAGE2_VAR}\" w:val=\"{tag}\"/>"));
    let Some(mut text) = package.text(&name) else {
        let Some(var) = var else {
            return;
        };
        package.set_text(
            NAME,
            format!(
                "{XML_DECL}<w:settings xmlns:w=\"{W_NS}\"><w:docVars>{var}</w:docVars></w:settings>"
            ),
        );
        ensure_content_type(package, NAME, CT_SETTINGS);
        crate::write::ensure_relationship(package, "settings.xml", "settings");
        return;
    };
    // The one an earlier save left goes first.
    if let Some(at) = text.find(&format!("<w:docVar w:name=\"{LANGUAGE2_VAR}\""))
        && let Some(end) = text[at..].find("/>")
    {
        text.replace_range(at..at + end + 2, "");
    }
    if let Some(var) = var {
        if let Some(close) = text.find("</w:docVars>") {
            text.insert_str(close, &var);
        } else {
            let at = [
                "<w:rsids",
                "<m:mathPr",
                "<w:themeFontLang",
                "<w:clrSchemeMapping",
                "<w:decimalSymbol",
                "<w:listSeparator",
                "</w:settings>",
            ]
            .iter()
            .filter_map(|marker| text.find(marker))
            .min()
            .unwrap_or(text.len());
            text.insert_str(at, &format!("<w:docVars>{var}</w:docVars>"));
        }
    }
    package.set_text(&name, text);
}

/// Puts `<w:evenAndOddHeaders/>` into `settings.xml`, creating the part when there is none.
pub(crate) fn turn_on_even_and_odd(package: &mut Package) {
    // The element sits before these in the schema's order; the first one found marks the
    // place, and the end of the part is the fallback.
    turn_on_setting(package, "evenAndOddHeaders", &EVEN_AND_ODD_LATER);
}

/// Puts `<w:displayBackgroundShape/>` into `settings.xml` (Word shows a page colour only
/// with it), creating the part when there is none.
pub(crate) fn turn_on_display_background(package: &mut Package) {
    const LATER: [&str; 22] = [
        "<w:printPostScriptOverText",
        "<w:printFractionalCharacterWidth",
        "<w:printFormsData",
        "<w:embedTrueTypeFonts",
        "<w:embedSystemFonts",
        "<w:saveSubsetFonts",
        "<w:saveFormsData",
        "<w:mirrorMargins",
        "<w:alignBordersAndEdges",
        "<w:gutterAtTop",
        "<w:hideSpellingErrors",
        "<w:hideGrammaticalErrors",
        "<w:activeWritingStyle",
        "<w:proofState",
        "<w:attachedTemplate",
        "<w:documentProtection",
        "<w:defaultTabStop",
        "<w:evenAndOddHeaders",
        "<w:characterSpacingControl",
        "<w:compat",
        "<w:rsids",
        "</w:settings>",
    ];
    turn_on_setting(package, "displayBackgroundShape", &LATER);
}

const EVEN_AND_ODD_LATER: [&str; 13] = [
    "<w:bookFoldRevPrinting",
    "<w:characterSpacingControl",
    "<w:footnotePr",
    "<w:endnotePr",
    "<w:compat",
    "<w:docVars",
    "<w:rsids",
    "<m:mathPr",
    "<w:themeFontLang",
    "<w:clrSchemeMapping",
    "<w:decimalSymbol",
    "<w:listSeparator",
    "</w:settings>",
];

/// Puts the empty on/off element `<w:{element}/>` into `settings.xml`, before the first of
/// `later` found (the schema's order), creating the part when there is none.
fn turn_on_setting(package: &mut Package, element: &str, later: &[&str]) {
    const NAME: &str = "word/settings.xml";
    let name = package
        .related_part("settings", NAME)
        .unwrap_or_else(|| NAME.to_owned());
    let tag = format!("<w:{element}/>");
    match package.text(&name) {
        Some(text) if text.contains(&format!("<w:{element}")) => {}
        Some(text) => {
            let at = later
                .iter()
                .filter_map(|marker| text.find(marker))
                .min()
                .unwrap_or(text.len());
            let mut edited = text;
            edited.insert_str(at, &tag);
            package.set_text(&name, edited);
        }
        None => {
            package.set_text(
                NAME,
                format!("{XML_DECL}<w:settings xmlns:w=\"{W_NS}\">{tag}</w:settings>"),
            );
            ensure_content_type(package, NAME, CT_SETTINGS);
            crate::write::ensure_relationship(package, "settings.xml", "settings");
        }
    }
}
