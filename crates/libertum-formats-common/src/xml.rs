//! A tiny XML tree on top of quick-xml, enough for the document-format subsets Libertum maps.
//!
//! Element and attribute names are stored by their local name (`w:val` or `fo:margin` becomes
//! `val` / `margin`), which is unambiguous for the elements we read. Text is unescaped as it
//! is collected, including character and predefined entity references. Children keep their
//! order with the text between them ([`XmlContent`]), because ODF mixes text and elements in
//! one paragraph; the tree is built one element at a time from a streaming reader, so a caller
//! can walk a whole document without ever holding all of it.

use std::borrow::Cow;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// A parse or content failure, tagged with the part it happened in.
#[derive(Clone, Debug, thiserror::Error)]
pub enum XmlError {
    #[error("invalid XML in {part}: {message}")]
    Xml { part: String, message: String },
    #[error("unexpected content in {part}: {message}")]
    Content { part: String, message: String },
}

impl XmlError {
    pub fn part(&self) -> &str {
        match self {
            XmlError::Xml { part, .. } | XmlError::Content { part, .. } => part,
        }
    }
}

/// One child of an element, in document order: either text or a nested element.
#[derive(Clone, Debug)]
pub enum XmlContent {
    Text(String),
    Element(XmlNode),
}

#[derive(Clone, Debug, Default)]
pub struct XmlNode {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub content: Vec<XmlContent>,
}

impl XmlNode {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The element children, in document order, skipping text.
    pub fn elements(&self) -> impl Iterator<Item = &XmlNode> {
        self.content.iter().filter_map(|content| match content {
            XmlContent::Element(node) => Some(node),
            XmlContent::Text(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.elements().find(|child| child.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a XmlNode> + 'a {
        self.elements().filter(move |child| child.name == name)
    }

    /// This element's own text, in order, with nested elements' text excluded.
    pub fn text(&self) -> Cow<'_, str> {
        let mut segments = self.content.iter().filter_map(|content| match content {
            XmlContent::Text(text) => Some(text.as_str()),
            XmlContent::Element(_) => None,
        });
        let Some(first) = segments.next() else {
            return Cow::Borrowed("");
        };
        let mut rest = segments.peekable();
        if rest.peek().is_none() {
            return Cow::Borrowed(first);
        }
        let mut out = String::with_capacity(first.len() + 16);
        out.push_str(first);
        for segment in rest {
            out.push_str(segment);
        }
        Cow::Owned(out)
    }

    pub fn attr_f32(&self, name: &str) -> Option<f32> {
        self.attr(name).and_then(|value| value.trim().parse().ok())
    }

    pub fn attr_u32(&self, name: &str) -> Option<u32> {
        self.attr(name).and_then(|value| value.trim().parse().ok())
    }

    /// A WordprocessingML on/off property: an absent `val` means on.
    pub fn on(&self) -> bool {
        match self.attr("val") {
            None => true,
            Some(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "none"
            ),
        }
    }

    /// A boolean property written the several ways the formats allow (`true`, `1`, `on`).
    pub fn flag(&self, name: &str) -> Option<bool> {
        self.attr(name).map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "off" | "none"
            )
        })
    }
}

pub fn xml_error(part: &str, error: impl std::fmt::Display) -> XmlError {
    XmlError::Xml {
        part: part.to_owned(),
        message: error.to_string(),
    }
}

pub fn content_error(part: &str, message: impl Into<String>) -> XmlError {
    XmlError::Content {
        part: part.to_owned(),
        message: message.into(),
    }
}

/// Parses a whole part into a tree rooted at its document element.
pub fn parse(xml: &str, part: &str) -> Result<XmlNode, XmlError> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(|e| xml_error(part, e))? {
            Event::Start(start) => return read_node(&mut reader, &start, part),
            Event::Empty(start) => return node_from_start(&start, part),
            Event::Eof => return Err(content_error(part, "the XML part is empty")),
            _ => {}
        }
    }
}

/// Reads one element subtree. `start` must be the element's start tag, already read.
pub fn read_node(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
    part: &str,
) -> Result<XmlNode, XmlError> {
    let mut stack = vec![node_from_start(start, part)?];
    loop {
        match reader.read_event().map_err(|e| xml_error(part, e))? {
            Event::Start(child) => stack.push(node_from_start(&child, part)?),
            Event::Empty(child) => {
                let node = node_from_start(&child, part)?;
                if let Some(parent) = stack.last_mut() {
                    parent.content.push(XmlContent::Element(node));
                }
            }
            Event::End(_) => {
                let Some(done) = stack.pop() else {
                    return Err(content_error(part, "unbalanced XML element"));
                };
                match stack.last_mut() {
                    Some(parent) => parent.content.push(XmlContent::Element(done)),
                    None => return Ok(done),
                }
            }
            Event::Text(text) => {
                let raw = text.into_inner();
                let value = quick_xml::escape::unescape(&raw)
                    .map_err(|e| content_error(part, format!("bad text: {e}")))?;
                if let Some(node) = stack.last_mut() {
                    node.content.push(XmlContent::Text(value.into_owned()));
                }
            }
            Event::CData(cdata) => {
                let raw = cdata.into_inner();
                if let Some(node) = stack.last_mut() {
                    node.content.push(XmlContent::Text(raw.as_ref().to_owned()));
                }
            }
            Event::GeneralRef(reference) => {
                let name: &str = reference.as_ref();
                let replacement = resolve_reference(name)
                    .ok_or_else(|| content_error(part, format!("unknown entity &{name};")))?;
                if let Some(node) = stack.last_mut() {
                    node.content.push(XmlContent::Text(replacement.to_string()));
                }
            }
            Event::Eof => return Err(content_error(part, "unexpected end of the XML part")),
            _ => {}
        }
    }
}

fn node_from_start(start: &BytesStart<'_>, part: &str) -> Result<XmlNode, XmlError> {
    let mut node = XmlNode {
        name: start.local_name().as_ref().to_owned(),
        ..XmlNode::default()
    };
    for attribute in start.attributes() {
        let attribute = attribute.map_err(|e| content_error(part, e.to_string()))?;
        let key = attribute.key.local_name().as_ref().to_owned();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| content_error(part, e.to_string()))?
            .into_owned();
        node.attrs.push((key, value));
    }
    Ok(node)
}

fn resolve_reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Escapes element text.
pub fn escape_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// Escapes an attribute value.
pub fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_attributes_text_and_entities() {
        let xml = r#"<w:root xmlns:w="urn:w"><w:p w:val="1">a &amp; b &#65;<w:r/></w:p></w:root>"#;
        let root = parse(xml, "test").expect("parse");
        assert_eq!(root.name, "root");
        let paragraph = root.child("p").expect("p");
        assert_eq!(paragraph.attr("val"), Some("1"));
        assert_eq!(paragraph.text(), "a & b A");
        assert_eq!(paragraph.child("r").map(|r| r.name.as_str()), Some("r"));
    }

    #[test]
    fn content_keeps_text_and_elements_in_order() {
        let xml = r#"<p>Hello <span>bold</span> world</p>"#;
        let root = parse(xml, "test").expect("parse");
        let kinds: Vec<String> = root
            .content
            .iter()
            .map(|content| match content {
                XmlContent::Text(text) => format!("t:{text}"),
                XmlContent::Element(node) => format!("e:{}:{}", node.name, node.text()),
            })
            .collect();
        assert_eq!(kinds, vec!["t:Hello ", "e:span:bold", "t: world"]);
        assert_eq!(root.text(), "Hello  world", "only direct text");
    }

    #[test]
    fn flags_read_the_boolean_spellings() {
        let xml = r#"<r a="true" b="false" c="1" d="off"/>"#;
        let root = parse(xml, "test").expect("parse");
        assert_eq!(root.flag("a"), Some(true));
        assert_eq!(root.flag("b"), Some(false));
        assert_eq!(root.flag("c"), Some(true));
        assert_eq!(root.flag("d"), Some(false));
        assert_eq!(root.flag("missing"), None);
    }

    #[test]
    fn escaping_round_trips_through_parsing() {
        let text = "a < b & \"c\" > d";
        let xml = format!("<t>{}</t>", escape_text(text));
        assert_eq!(parse(&xml, "test").expect("parse").text(), text);
        let xml = format!(r#"<t a="{}"/>"#, escape_attr(text));
        assert_eq!(parse(&xml, "test").expect("parse").attr("a"), Some(text));
    }
}
