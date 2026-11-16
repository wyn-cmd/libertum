//! The questions the MCP server can ask the application, and the answers it gets back.
//!
//! These are plain data. The server never sees the editor or the document model: the
//! application implements [`crate::Host`] and answers from its own thread, so a request
//! costs nothing until a client actually asks for something.

use serde::Serialize;

/// A position in the document: a paragraph (counted from 0) and a UTF-8 byte offset into
/// that paragraph's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Position {
    pub paragraph: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocumentInfo {
    /// Stable for as long as the document stays open.
    pub id: u64,
    pub title: String,
    pub path: Option<String>,
    pub format: String,
    pub paragraphs: usize,
    pub dirty: bool,
    /// The document the writer is looking at.
    pub active: bool,
}

/// A run of whole paragraphs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TextSlice {
    pub document: u64,
    pub start_paragraph: usize,
    pub paragraphs: Vec<String>,
    pub total_paragraphs: usize,
    /// More paragraphs follow the ones returned (the request or a size limit stopped here).
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Heading {
    pub paragraph: usize,
    pub level: u8,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StyleInfo {
    pub id: String,
    pub name: String,
    /// The style this one inherits from, if any.
    pub base: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelectionInfo {
    pub document: u64,
    pub anchor: Position,
    pub head: Position,
    pub is_empty: bool,
    /// The selected text, cut at a size limit (see `truncated`).
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchMatch {
    pub paragraph: usize,
    pub offset: usize,
    /// The paragraph's text around the match, for orientation.
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResults {
    pub document: u64,
    pub matches: Vec<SearchMatch>,
    /// There were more matches than `limit`.
    pub truncated: bool,
}

/// Who is asking, as the client announced itself when it connected. The name is the client's own
/// claim, not proof of identity: the bearer token is what keeps strangers out.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl ClientInfo {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }

    /// For a client that sent no usable name.
    pub fn unknown() -> Self {
        Self::new("unknown client", "")
    }
}

/// Character formatting an edit lays over a range. `None` leaves a property as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FormatPatch {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strikethrough: Option<bool>,
}

impl FormatPatch {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// What the server asks. `document: None` means the active document.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    ListDocuments,
    ReadText {
        document: Option<u64>,
        start_paragraph: usize,
        count: usize,
    },
    Outline {
        document: Option<u64>,
    },
    Styles {
        document: Option<u64>,
    },
    Selection {
        document: Option<u64>,
    },
    Search {
        document: Option<u64>,
        query: String,
        case_sensitive: bool,
        limit: usize,
    },
    /// Replace `start..end` with `text`; an empty `text` deletes, an empty range inserts.
    ReplaceRange {
        document: Option<u64>,
        start: Position,
        end: Position,
        text: String,
    },
    FormatRange {
        document: Option<u64>,
        start: Position,
        end: Position,
        format: FormatPatch,
    },
    /// `style` is a style's id or its name (see `get_styles`).
    SetParagraphStyle {
        document: Option<u64>,
        paragraph: usize,
        style: String,
    },
}

impl Request {
    /// Whether this changes a document (so it needs the Allow edits switch and approval).
    pub fn is_edit(&self) -> bool {
        matches!(
            self,
            Self::ReplaceRange { .. } | Self::FormatRange { .. } | Self::SetParagraphStyle { .. }
        )
    }

    /// The document the request names (`None` means the active one, or no document at all).
    pub fn document(&self) -> Option<u64> {
        match self {
            Self::ListDocuments => None,
            Self::ReadText { document, .. }
            | Self::Outline { document }
            | Self::Styles { document }
            | Self::Selection { document }
            | Self::Search { document, .. }
            | Self::ReplaceRange { document, .. }
            | Self::FormatRange { document, .. }
            | Self::SetParagraphStyle { document, .. } => *document,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Documents(Vec<DocumentInfo>),
    Text(TextSlice),
    Outline {
        document: u64,
        headings: Vec<Heading>,
    },
    Styles {
        document: u64,
        styles: Vec<StyleInfo>,
    },
    Selection(SelectionInfo),
    Search(SearchResults),
    /// An edit was made (as one undo step).
    Edited {
        document: u64,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("no document is open")]
    NoDocument,
    #[error("no open document has id {0}")]
    UnknownDocument(u64),
    /// The application did not answer: it is busy, closing, or MCP was switched off.
    #[error("Libertum did not answer: {0}")]
    Unavailable(String),
    /// The writer has not switched on Allow edits.
    #[error(
        "editing through MCP is switched off. Ask the writer to turn on Allow edits in Settings"
    )]
    EditsDisabled,
    /// The writer refused this edit (or this client).
    #[error("the writer did not allow this edit")]
    Denied,
    #[error("{0}")]
    Other(String),
}

pub type HostResult = Result<Reply, HostError>;
