//! The tools, their schemas, argument checking and output limits.
//!
//! Everything here is independent of the network and of rmcp's handler types, so it is tested
//! with a fake [`Host`].

use serde_json::{Map, Value, json};

use crate::host::Host;
use crate::protocol::{ClientInfo, FormatPatch, HostError, Position, Reply, Request};

/// Most paragraphs one `read_text` call returns.
pub const MAX_PARAGRAPHS: usize = 200;
/// Most bytes of text one call returns, whatever the paragraph count.
pub const MAX_TEXT_BYTES: usize = 200_000;
/// Most matches one `search_text` call returns.
pub const MAX_MATCHES: usize = 200;
/// Longest search query accepted.
pub const MAX_QUERY_CHARS: usize = 1_000;

/// Most bytes of text one edit may insert.
pub const MAX_EDIT_BYTES: usize = 100_000;

const DEFAULT_PARAGRAPHS: usize = 50;
const DEFAULT_MATCHES: usize = 50;

/// A tool as plain data; the server turns it into rmcp's `Tool`.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: Value,
    /// Whether the tool changes a document. These only work with Allow edits on, and each
    /// client's first edit needs the writer's approval.
    pub writes: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The caller got the arguments wrong (a protocol-level error).
    #[error("{0}")]
    InvalidParams(String),
    /// No such tool.
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    /// The tool ran but the application could not do it (shown to the caller as a tool error).
    #[error(transparent)]
    Host(#[from] HostError),
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn document_property() -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": "A document id from list_documents. Leave out for the document the writer is looking at."
    })
}

/// Every tool the server offers. All of them only read.
pub fn definitions() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "list_documents",
            description: "List the documents open in Libertum: id, title, path, format, paragraph count, whether there are unsaved changes, and which one is active.",
            schema: object(json!({}), &[]),
            writes: false,
        },
        ToolDef {
            name: "read_text",
            description: "Read a run of paragraphs as plain text. Paragraphs are numbered from 0. At most 200 paragraphs or 200,000 bytes are returned; `truncated` says whether more follow.",
            schema: object(
                json!({
                    "document": document_property(),
                    "start_paragraph": { "type": "integer", "minimum": 0, "description": "First paragraph to read (default 0)." },
                    "count": { "type": "integer", "minimum": 1, "maximum": MAX_PARAGRAPHS, "description": "How many paragraphs (default 50, at most 200)." }
                }),
                &[],
            ),
            writes: false,
        },
        ToolDef {
            name: "get_outline",
            description: "List the headings (paragraph number, level 1 to 9 and text), which is the document's structure.",
            schema: object(json!({ "document": document_property() }), &[]),
            writes: false,
        },
        ToolDef {
            name: "get_styles",
            description: "List the paragraph styles the document defines, with the style each one is based on.",
            schema: object(json!({ "document": document_property() }), &[]),
            writes: false,
        },
        ToolDef {
            name: "get_selection",
            description: "Report the writer's selection or caret: both ends as paragraph number and UTF-8 byte offset, and the selected text (cut at a size limit).",
            schema: object(json!({ "document": document_property() }), &[]),
            writes: false,
        },
        ToolDef {
            name: "search_text",
            description: "Find text in the document. Returns the paragraph number, byte offset and the surrounding text of each match, in document order.",
            schema: object(
                json!({
                    "document": document_property(),
                    "query": { "type": "string", "minLength": 1, "maxLength": MAX_QUERY_CHARS, "description": "The text to find." },
                    "case_sensitive": { "type": "boolean", "description": "Default false." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_MATCHES, "description": "Most matches to return (default 50, at most 200)." }
                }),
                &["query"],
            ),
            writes: false,
        },
        ToolDef {
            name: "insert_text",
            description: "Insert text at a position (paragraph number and UTF-8 byte offset). A newline in the text starts a new paragraph. Needs the writer's permission; one undo step.",
            schema: object(
                json!({
                    "document": document_property(),
                    "paragraph": { "type": "integer", "minimum": 0, "description": "Paragraph to insert into." },
                    "offset": { "type": "integer", "minimum": 0, "description": "Byte offset in that paragraph, on a character boundary." },
                    "text": { "type": "string", "minLength": 1, "maxLength": MAX_EDIT_BYTES, "description": "The text to insert." }
                }),
                &["paragraph", "offset", "text"],
            ),
            writes: true,
        },
        ToolDef {
            name: "replace_range",
            description: "Replace the text from one position to another with new text; an empty `text` deletes the range. Positions are paragraph number and UTF-8 byte offset, and the range cannot cross a table cell. Needs the writer's permission; one undo step.",
            schema: object(
                json!({
                    "document": document_property(),
                    "start_paragraph": { "type": "integer", "minimum": 0 },
                    "start_offset": { "type": "integer", "minimum": 0 },
                    "end_paragraph": { "type": "integer", "minimum": 0 },
                    "end_offset": { "type": "integer", "minimum": 0 },
                    "text": { "type": "string", "maxLength": MAX_EDIT_BYTES, "description": "The replacement; empty deletes." }
                }),
                &[
                    "start_paragraph",
                    "start_offset",
                    "end_paragraph",
                    "end_offset",
                    "text",
                ],
            ),
            writes: true,
        },
        ToolDef {
            name: "format_range",
            description: "Turn bold, italic, underline or strikethrough on or off over a range (give at least one). Needs the writer's permission; one undo step.",
            schema: object(
                json!({
                    "document": document_property(),
                    "start_paragraph": { "type": "integer", "minimum": 0 },
                    "start_offset": { "type": "integer", "minimum": 0 },
                    "end_paragraph": { "type": "integer", "minimum": 0 },
                    "end_offset": { "type": "integer", "minimum": 0 },
                    "bold": { "type": "boolean" },
                    "italic": { "type": "boolean" },
                    "underline": { "type": "boolean" },
                    "strikethrough": { "type": "boolean" }
                }),
                &[
                    "start_paragraph",
                    "start_offset",
                    "end_paragraph",
                    "end_offset",
                ],
            ),
            writes: true,
        },
        ToolDef {
            name: "set_paragraph_style",
            description: "Give one paragraph a paragraph style, by the id or name that get_styles lists (for example a heading style). Needs the writer's permission; one undo step.",
            schema: object(
                json!({
                    "document": document_property(),
                    "paragraph": { "type": "integer", "minimum": 0 },
                    "style": { "type": "string", "minLength": 1, "maxLength": 200, "description": "A style id or name from get_styles." }
                }),
                &["paragraph", "style"],
            ),
            writes: true,
        },
    ]
}

fn optional_uint(args: &Map<String, Value>, key: &str) -> Result<Option<u64>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or_else(|| {
            ToolError::InvalidParams(format!("`{key}` must be a non-negative whole number"))
        }),
    }
}

fn optional_usize(
    args: &Map<String, Value>,
    key: &str,
    default: usize,
    max: usize,
) -> Result<usize, ToolError> {
    match optional_uint(args, key)? {
        None => Ok(default),
        Some(0) if key != "start_paragraph" => Err(ToolError::InvalidParams(format!(
            "`{key}` must be at least 1"
        ))),
        Some(n) => Ok(usize::try_from(n).unwrap_or(usize::MAX).min(max)),
    }
}

/// Cut `text` to at most `max` bytes on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    &text[..text.floor_char_boundary(max)]
}

fn required_usize(args: &Map<String, Value>, key: &str) -> Result<usize, ToolError> {
    match args.get(key) {
        Some(value) => value
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| {
                ToolError::InvalidParams(format!("`{key}` must be a non-negative whole number"))
            }),
        None => Err(ToolError::InvalidParams(format!("`{key}` is required"))),
    }
}

fn required_text<'a>(
    args: &'a Map<String, Value>,
    key: &str,
    allow_empty: bool,
) -> Result<&'a str, ToolError> {
    match args.get(key) {
        Some(Value::String(text)) if allow_empty || !text.is_empty() => {
            if text.len() > MAX_EDIT_BYTES {
                return Err(ToolError::InvalidParams(format!(
                    "`{key}` is longer than {MAX_EDIT_BYTES} bytes"
                )));
            }
            Ok(text)
        }
        _ => Err(ToolError::InvalidParams(if allow_empty {
            format!("`{key}` is required and must be a string")
        } else {
            format!("`{key}` is required and must be a non-empty string")
        })),
    }
}

fn range(args: &Map<String, Value>) -> Result<(Position, Position), ToolError> {
    Ok((
        Position {
            paragraph: required_usize(args, "start_paragraph")?,
            offset: required_usize(args, "start_offset")?,
        },
        Position {
            paragraph: required_usize(args, "end_paragraph")?,
            offset: required_usize(args, "end_offset")?,
        },
    ))
}

fn optional_flag(args: &Map<String, Value>, key: &str) -> Result<Option<bool>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(ToolError::InvalidParams(format!(
            "`{key}` must be true or false"
        ))),
    }
}

/// Run a tool as `client` and return its result as JSON text.
pub fn call(
    host: &dyn Host,
    client: &ClientInfo,
    name: &str,
    args: &Map<String, Value>,
) -> Result<String, ToolError> {
    let request = match name {
        "list_documents" => Request::ListDocuments,
        "read_text" => Request::ReadText {
            document: optional_uint(args, "document")?,
            start_paragraph: optional_usize(args, "start_paragraph", 0, usize::MAX)?,
            count: optional_usize(args, "count", DEFAULT_PARAGRAPHS, MAX_PARAGRAPHS)?,
        },
        "get_outline" => Request::Outline {
            document: optional_uint(args, "document")?,
        },
        "get_styles" => Request::Styles {
            document: optional_uint(args, "document")?,
        },
        "get_selection" => Request::Selection {
            document: optional_uint(args, "document")?,
        },
        "search_text" => {
            let query = match args.get("query") {
                Some(Value::String(q)) if !q.is_empty() => q.clone(),
                _ => {
                    return Err(ToolError::InvalidParams(
                        "`query` is required and must be a non-empty string".into(),
                    ));
                }
            };
            if query.chars().count() > MAX_QUERY_CHARS {
                return Err(ToolError::InvalidParams(format!(
                    "`query` is longer than {MAX_QUERY_CHARS} characters"
                )));
            }
            Request::Search {
                document: optional_uint(args, "document")?,
                query,
                case_sensitive: optional_flag(args, "case_sensitive")?.unwrap_or(false),
                limit: optional_usize(args, "limit", DEFAULT_MATCHES, MAX_MATCHES)?,
            }
        }
        "insert_text" => {
            let at = Position {
                paragraph: required_usize(args, "paragraph")?,
                offset: required_usize(args, "offset")?,
            };
            Request::ReplaceRange {
                document: optional_uint(args, "document")?,
                start: at,
                end: at,
                text: required_text(args, "text", false)?.to_owned(),
            }
        }
        "replace_range" => {
            let (start, end) = range(args)?;
            Request::ReplaceRange {
                document: optional_uint(args, "document")?,
                start,
                end,
                text: required_text(args, "text", true)?.to_owned(),
            }
        }
        "format_range" => {
            let (start, end) = range(args)?;
            let format = FormatPatch {
                bold: optional_flag(args, "bold")?,
                italic: optional_flag(args, "italic")?,
                underline: optional_flag(args, "underline")?,
                strikethrough: optional_flag(args, "strikethrough")?,
            };
            if format.is_empty() {
                return Err(ToolError::InvalidParams(
                    "give at least one of bold, italic, underline or strikethrough".into(),
                ));
            }
            Request::FormatRange {
                document: optional_uint(args, "document")?,
                start,
                end,
                format,
            }
        }
        "set_paragraph_style" => Request::SetParagraphStyle {
            document: optional_uint(args, "document")?,
            paragraph: required_usize(args, "paragraph")?,
            style: required_text(args, "style", false)?.to_owned(),
        },
        other => return Err(ToolError::UnknownTool(other.to_owned())),
    };
    let reply = host.request(client, request)?;
    Ok(render(limit(reply)))
}

/// Enforce the output limits on whatever the host returned, so a careless host cannot send an
/// unbounded document to a client.
fn limit(reply: Reply) -> Reply {
    match reply {
        Reply::Text(mut slice) => {
            slice.paragraphs.truncate(MAX_PARAGRAPHS);
            let mut used = 0usize;
            let mut keep = 0usize;
            for paragraph in &slice.paragraphs {
                if used + paragraph.len() > MAX_TEXT_BYTES && keep > 0 {
                    break;
                }
                used += paragraph.len();
                keep += 1;
            }
            slice.paragraphs.truncate(keep);
            if let Some(first) = slice.paragraphs.first_mut()
                && first.len() > MAX_TEXT_BYTES
            {
                *first = cut(first, MAX_TEXT_BYTES).to_owned();
            }
            slice.truncated = slice.truncated
                || slice.start_paragraph + slice.paragraphs.len() < slice.total_paragraphs;
            Reply::Text(slice)
        }
        Reply::Selection(mut selection) => {
            if selection.text.len() > MAX_TEXT_BYTES {
                selection.text = cut(&selection.text, MAX_TEXT_BYTES).to_owned();
                selection.truncated = true;
            }
            Reply::Selection(selection)
        }
        Reply::Search(mut results) => {
            if results.matches.len() > MAX_MATCHES {
                results.matches.truncate(MAX_MATCHES);
                results.truncated = true;
            }
            for found in &mut results.matches {
                if found.context.len() > 400 {
                    found.context = cut(&found.context, 400).to_owned();
                }
            }
            Reply::Search(results)
        }
        other => other,
    }
}

fn render(reply: Reply) -> String {
    let value = match reply {
        Reply::Documents(documents) => json!({ "documents": documents }),
        Reply::Text(slice) => json!(slice),
        Reply::Outline { document, headings } => {
            json!({ "document": document, "headings": headings })
        }
        Reply::Styles { document, styles } => json!({ "document": document, "styles": styles }),
        Reply::Selection(selection) => json!(selection),
        Reply::Search(results) => json!(results),
        Reply::Edited { document, message } => json!({ "document": document, "message": message }),
    };
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{
        DocumentInfo, HostResult, Position, SearchMatch, SearchResults, SelectionInfo, TextSlice,
    };
    use std::sync::Mutex;

    /// Records the requests it receives and answers from a closure.
    struct Fake {
        seen: Mutex<Vec<Request>>,
        answer: Box<dyn Fn(&Request) -> HostResult + Send + Sync>,
    }

    impl Fake {
        fn new(answer: impl Fn(&Request) -> HostResult + Send + Sync + 'static) -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                answer: Box::new(answer),
            }
        }
        fn last(&self) -> Request {
            self.seen
                .lock()
                .expect("lock")
                .last()
                .cloned()
                .expect("a request")
        }
    }

    impl Host for Fake {
        fn request(&self, _client: &ClientInfo, request: Request) -> HostResult {
            let result = (self.answer)(&request);
            self.seen.lock().expect("lock").push(request);
            result
        }
    }

    fn who() -> ClientInfo {
        ClientInfo::new("test client", "1")
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("an object")
    }

    #[test]
    fn every_tool_has_an_object_schema_and_a_unique_name() {
        let tools = definitions();
        let mut names: Vec<_> = tools.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), tools.len(), "names are unique");
        for tool in &tools {
            assert_eq!(tool.schema["type"], "object", "{}", tool.name);
            assert!(!tool.description.is_empty());
        }
    }

    #[test]
    fn read_text_defaults_and_clamps() {
        let fake = Fake::new(|_| Err(HostError::NoDocument));
        let _ = call(&fake, &who(), "read_text", &args(json!({})));
        assert_eq!(
            fake.last(),
            Request::ReadText {
                document: None,
                start_paragraph: 0,
                count: 50
            }
        );
        let _ = call(
            &fake,
            &who(),
            "read_text",
            &args(json!({ "document": 3, "start_paragraph": 7, "count": 99999 })),
        );
        assert_eq!(
            fake.last(),
            Request::ReadText {
                document: Some(3),
                start_paragraph: 7,
                count: MAX_PARAGRAPHS
            }
        );
    }

    #[test]
    fn bad_arguments_are_rejected_before_the_host_is_asked() {
        let fake = Fake::new(|_| panic!("the host must not be asked"));
        for (tool, bad) in [
            ("read_text", json!({ "document": "one" })),
            ("read_text", json!({ "count": 0 })),
            ("read_text", json!({ "start_paragraph": -1 })),
            ("search_text", json!({})),
            ("search_text", json!({ "query": "" })),
            ("search_text", json!({ "query": 5 })),
            (
                "search_text",
                json!({ "query": "x", "case_sensitive": "yes" }),
            ),
            (
                "search_text",
                json!({ "query": "x".repeat(MAX_QUERY_CHARS + 1) }),
            ),
        ] {
            let result = call(&fake, &who(), tool, &args(bad.clone()));
            assert!(
                matches!(result, Err(ToolError::InvalidParams(_))),
                "{tool} {bad}"
            );
        }
    }

    #[test]
    fn an_unknown_tool_is_named() {
        let fake = Fake::new(|_| Err(HostError::NoDocument));
        let result = call(&fake, &who(), "delete_everything", &args(json!({})));
        assert!(matches!(result, Err(ToolError::UnknownTool(name)) if name == "delete_everything"));
    }

    #[test]
    fn list_documents_returns_json() {
        let fake = Fake::new(|_| {
            Ok(Reply::Documents(vec![DocumentInfo {
                id: 1,
                title: "Book".into(),
                path: None,
                format: "odt".into(),
                paragraphs: 12,
                dirty: true,
                active: true,
            }]))
        });
        let text = call(&fake, &who(), "list_documents", &args(json!({}))).expect("ok");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["documents"][0]["title"], "Book");
        assert_eq!(value["documents"][0]["paragraphs"], 12);
    }

    #[test]
    fn a_greedy_host_cannot_exceed_the_text_limits() {
        let big = "x".repeat(MAX_TEXT_BYTES / 2 + 1);
        let fake = Fake::new(move |_| {
            Ok(Reply::Text(TextSlice {
                document: 1,
                start_paragraph: 0,
                paragraphs: vec![big.clone(), big.clone(), big.clone()],
                total_paragraphs: 3,
                truncated: false,
            }))
        });
        let text = call(&fake, &who(), "read_text", &args(json!({}))).expect("ok");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["paragraphs"].as_array().expect("array").len(), 1);
        assert_eq!(value["truncated"], true, "the cut is reported");

        let many = Fake::new(|_| {
            Ok(Reply::Text(TextSlice {
                document: 1,
                start_paragraph: 0,
                paragraphs: vec!["a".into(); 500],
                total_paragraphs: 500,
                truncated: false,
            }))
        });
        let text = call(&many, &who(), "read_text", &args(json!({}))).expect("ok");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            value["paragraphs"].as_array().expect("array").len(),
            MAX_PARAGRAPHS
        );
        assert_eq!(value["truncated"], true);
    }

    #[test]
    fn one_huge_paragraph_is_cut_on_a_character_boundary() {
        let fake = Fake::new(|_| {
            Ok(Reply::Text(TextSlice {
                document: 1,
                start_paragraph: 0,
                // Multi-byte characters, so a naive byte cut would split one.
                paragraphs: vec!["é".repeat(MAX_TEXT_BYTES)],
                total_paragraphs: 1,
                truncated: false,
            }))
        });
        let text = call(&fake, &who(), "read_text", &args(json!({}))).expect("ok");
        let value: Value = serde_json::from_str(&text).expect("json");
        let paragraph = value["paragraphs"][0].as_str().expect("string");
        assert!(paragraph.len() <= MAX_TEXT_BYTES);
        assert!(paragraph.chars().all(|c| c == 'é'));
    }

    #[test]
    fn selection_and_search_are_limited() {
        let fake = Fake::new(|_| {
            Ok(Reply::Selection(SelectionInfo {
                document: 1,
                anchor: Position {
                    paragraph: 0,
                    offset: 0,
                },
                head: Position {
                    paragraph: 9,
                    offset: 3,
                },
                is_empty: false,
                text: "y".repeat(MAX_TEXT_BYTES + 10),
                truncated: false,
            }))
        });
        let text = call(&fake, &who(), "get_selection", &args(json!({}))).expect("ok");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["truncated"], true);
        assert_eq!(
            value["text"].as_str().expect("string").len(),
            MAX_TEXT_BYTES
        );

        let search = Fake::new(|_| {
            Ok(Reply::Search(SearchResults {
                document: 1,
                matches: vec![
                    SearchMatch {
                        paragraph: 0,
                        offset: 0,
                        context: "c".repeat(1000)
                    };
                    MAX_MATCHES + 5
                ],
                truncated: false,
            }))
        });
        let text = call(
            &search,
            &who(),
            "search_text",
            &args(json!({ "query": "c" })),
        )
        .expect("ok");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(
            value["matches"].as_array().expect("array").len(),
            MAX_MATCHES
        );
        assert_eq!(value["truncated"], true);
        assert!(
            value["matches"][0]["context"]
                .as_str()
                .expect("string")
                .len()
                <= 400
        );
    }

    #[test]
    fn host_errors_come_back_as_errors() {
        let fake = Fake::new(|_| Err(HostError::UnknownDocument(9)));
        let result = call(
            &fake,
            &who(),
            "get_outline",
            &args(json!({ "document": 9 })),
        );
        assert!(matches!(
            result,
            Err(ToolError::Host(HostError::UnknownDocument(9)))
        ));
    }

    #[test]
    fn edit_tools_become_requests_and_carry_the_client() {
        let fake = Fake::new(|_| {
            Ok(Reply::Edited {
                document: 1,
                message: "ok".into(),
            })
        });
        let at = Position {
            paragraph: 2,
            offset: 5,
        };
        call(
            &fake,
            &who(),
            "insert_text",
            &args(json!({ "paragraph": 2, "offset": 5, "text": "hi" })),
        )
        .expect("ok");
        assert_eq!(
            fake.last(),
            Request::ReplaceRange {
                document: None,
                start: at,
                end: at,
                text: "hi".into()
            }
        );
        // An empty replacement is a deletion, and is allowed.
        call(
            &fake,
            &who(),
            "replace_range",
            &args(json!({
                "document": 4, "start_paragraph": 0, "start_offset": 1,
                "end_paragraph": 1, "end_offset": 2, "text": ""
            })),
        )
        .expect("ok");
        assert!(matches!(
            fake.last(),
            Request::ReplaceRange { document: Some(4), text, .. } if text.is_empty()
        ));
        call(
            &fake,
            &who(),
            "format_range",
            &args(json!({
                "start_paragraph": 0, "start_offset": 0,
                "end_paragraph": 0, "end_offset": 3, "bold": true, "italic": false
            })),
        )
        .expect("ok");
        assert!(matches!(
            fake.last(),
            Request::FormatRange { format, .. }
                if format.bold == Some(true) && format.italic == Some(false) && format.underline.is_none()
        ));
        call(
            &fake,
            &who(),
            "set_paragraph_style",
            &args(json!({ "paragraph": 3, "style": "Heading 1" })),
        )
        .expect("ok");
        assert!(fake.last().is_edit());
    }

    #[test]
    fn bad_edit_arguments_are_rejected_before_the_host_is_asked() {
        let fake = Fake::new(|_| panic!("the host must not be asked"));
        let range = json!({
            "start_paragraph": 0, "start_offset": 0, "end_paragraph": 0, "end_offset": 1
        });
        let with = |extra: Value| {
            let mut m = range.as_object().cloned().expect("object");
            m.extend(extra.as_object().cloned().expect("object"));
            Value::Object(m)
        };
        for (tool, bad) in [
            ("insert_text", json!({ "paragraph": 0, "offset": 0 })),
            (
                "insert_text",
                json!({ "paragraph": 0, "offset": 0, "text": "" }),
            ),
            (
                "insert_text",
                json!({ "paragraph": -1, "offset": 0, "text": "x" }),
            ),
            (
                "insert_text",
                json!({ "paragraph": 0, "offset": 0, "text": "x".repeat(MAX_EDIT_BYTES + 1) }),
            ),
            ("replace_range", json!({})),
            ("replace_range", range.clone()),
            ("replace_range", with(json!({ "text": 5 }))),
            ("format_range", range.clone()),
            ("format_range", with(json!({ "bold": "yes" }))),
            ("set_paragraph_style", json!({ "paragraph": 0 })),
            (
                "set_paragraph_style",
                json!({ "paragraph": 0, "style": "" }),
            ),
        ] {
            let result = call(&fake, &who(), tool, &args(bad.clone()));
            assert!(
                matches!(result, Err(ToolError::InvalidParams(_))),
                "{tool} {bad}"
            );
        }
    }
}
