//! XML helpers for ODF. The tree and escaping live in `libertum-formats-common`; this module
//! re-exports them so the rest of the crate has one path for XML.

pub(crate) use libertum_formats_common::xml::{
    XmlContent, XmlError, XmlNode, escape_attr, escape_text, read_node,
};

/// Parses a whole part into a tree rooted at its document element.
pub(crate) fn parse(text: &str, part: &str) -> Result<XmlNode, XmlError> {
    libertum_formats_common::xml::parse(text, part)
}
