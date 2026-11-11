//! XML helpers for WordprocessingML, shared with the other format crates through
//! [`libertum_formats_common::xml`]. Errors are mapped onto [`DocxError`] so the rest of this
//! crate keeps its own error vocabulary.

use quick_xml::Reader;
use quick_xml::events::BytesStart;

use libertum_formats_common::xml as common;

pub(crate) use libertum_formats_common::xml::{XmlNode, escape_attr, escape_text};

use crate::DocxError;

pub(crate) fn xml_error(part: &str, error: impl std::fmt::Display) -> DocxError {
    DocxError::Xml {
        part: part.to_owned(),
        message: error.to_string(),
    }
}

/// Parses a whole part into a tree rooted at its document element.
pub(crate) fn parse(xml: &str, part: &str) -> Result<XmlNode, DocxError> {
    common::parse(xml, part).map_err(into_docx)
}

/// Reads one element subtree. `start` must be the element's start tag, already read.
pub(crate) fn read_node(
    reader: &mut Reader<&[u8]>,
    start: &BytesStart<'_>,
    part: &str,
) -> Result<XmlNode, DocxError> {
    common::read_node(reader, start, part).map_err(into_docx)
}

fn into_docx(error: common::XmlError) -> DocxError {
    match error {
        common::XmlError::Xml { part, message } => DocxError::Xml { part, message },
        common::XmlError::Content { part, message } => DocxError::Content { part, message },
    }
}
