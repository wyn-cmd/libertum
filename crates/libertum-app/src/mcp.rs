//! The application's side of the MCP server (dev mode): answers its questions from the open
//! document, and owns the server's lifetime.
//!
//! Everything here runs on the UI thread and only on a client's request, so it adds nothing to
//! the keystroke path. Edits are applied by [`apply_edit`] only after the app has checked the
//! Allow edits switch and the writer's approval (see `App::answer_mcp`).

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::futures::channel::mpsc::{UnboundedReceiver, unbounded};

use libertum_editor::{DocPosition, EditorError, SearchOptions, matches_in};
use libertum_mcp::{
    ChannelHost, DocumentInfo, FormatPatch, Heading, HostError, HostResult, Pending, Position,
    Reply, Request, RunningServer, SearchMatch, SearchResults, SelectionInfo, StyleInfo, TextSlice,
};
use libertum_model::{CharFormat, StyleId};

use crate::docview::DocView;
use crate::strings as s;

/// How long the server waits for the UI thread before telling the client Libertum is busy.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an edit may wait for the writer to approve it before the client is told it failed.
const EDIT_TIMEOUT: Duration = Duration::from_secs(120);

/// What the app knows about the open document that the model does not hold.
pub struct DocMeta<'a> {
    pub title: &'a str,
    pub path: Option<&'a Path>,
    pub format: &'a str,
    pub dirty: bool,
}

/// Answer one request from the open document (`None` when no document is open).
pub fn answer(request: &Request, view: Option<&DocView>, meta: &DocMeta<'_>) -> HostResult {
    if request.is_edit() {
        // Edits never come through here: they need the switch and the writer's approval.
        return Err(HostError::Other("edits are not read requests".into()));
    }
    if matches!(request, Request::ListDocuments) {
        return Ok(Reply::Documents(
            view.map(|view| info(view, meta)).into_iter().collect(),
        ));
    }
    let view = view.ok_or(HostError::NoDocument)?;
    if let Some(id) = request.document()
        && id != view.id()
    {
        return Err(HostError::UnknownDocument(id));
    }
    let document = view.document();
    let id = view.id();
    match request {
        Request::ListDocuments => unreachable!("handled above"),
        Request::ReplaceRange { .. }
        | Request::FormatRange { .. }
        | Request::SetParagraphStyle { .. } => unreachable!("edits are refused above"),
        Request::ReadText {
            start_paragraph,
            count,
            ..
        } => {
            let total = document.paragraph_count();
            let start = (*start_paragraph).min(total);
            let end = start.saturating_add(*count).min(total);
            let paragraphs = (start..end)
                .filter_map(|i| document.paragraph_at(i))
                .map(|p| p.text().to_owned())
                .collect();
            Ok(Reply::Text(TextSlice {
                document: id,
                start_paragraph: start,
                paragraphs,
                total_paragraphs: total,
                truncated: end < total,
            }))
        }
        Request::Outline { .. } => {
            let headings = document
                .paragraphs()
                .enumerate()
                .filter_map(|(index, paragraph)| {
                    paragraph.props.outline_level.map(|level| Heading {
                        paragraph: index,
                        level,
                        text: paragraph.text().to_owned(),
                    })
                })
                .collect();
            Ok(Reply::Outline {
                document: id,
                headings,
            })
        }
        Request::Styles { .. } => {
            let sheet = &document.styles;
            let styles = sheet
                .paragraph
                .iter()
                .map(|style| StyleInfo {
                    id: style.name.clone(),
                    name: style.name.clone(),
                    base: style
                        .base
                        .and_then(|base| sheet.get(base))
                        .map(|base| base.name.clone()),
                })
                .collect();
            Ok(Reply::Styles {
                document: id,
                styles,
            })
        }
        Request::Selection { .. } => {
            let editor = view.editor();
            let selection = editor.selection();
            let text = editor.selected_text();
            Ok(Reply::Selection(SelectionInfo {
                document: id,
                anchor: position(view, selection.anchor),
                head: position(view, selection.head),
                is_empty: !editor.has_selection(),
                text: text.unwrap_or_default(),
                truncated: false,
            }))
        }
        Request::Search {
            query,
            case_sensitive,
            limit,
            ..
        } => {
            let options = SearchOptions {
                match_case: *case_sensitive,
                whole_word: false,
            };
            let mut matches = Vec::new();
            let mut truncated = false;
            'paragraphs: for (index, paragraph) in document.paragraphs().enumerate() {
                for range in matches_in(paragraph.text(), query, options) {
                    if matches.len() >= *limit {
                        truncated = true;
                        break 'paragraphs;
                    }
                    matches.push(SearchMatch {
                        paragraph: index,
                        offset: range.start,
                        context: context_around(paragraph.text(), range.start, range.end),
                    });
                }
            }
            Ok(Reply::Search(SearchResults {
                document: id,
                matches,
                truncated,
            }))
        }
    }
}

fn host_error(error: EditorError) -> HostError {
    HostError::Other(error.to_string())
}

fn no_such_paragraph(view: &DocView, paragraph: usize) -> HostError {
    HostError::Other(format!(
        "paragraph {paragraph} does not exist: the document has {} paragraphs, numbered from 0",
        view.document().paragraph_count()
    ))
}

/// A paragraph number as the model's position, or an error naming the document's size.
fn resolve(view: &DocView, at: Position) -> Result<DocPosition, HostError> {
    view.document()
        .paragraph_at(at.paragraph)
        .map(|p| DocPosition {
            block: p.id,
            offset: at.offset,
        })
        .ok_or_else(|| no_such_paragraph(view, at.paragraph))
}

fn place(position: Position) -> String {
    format!(
        "paragraph {}, offset {}",
        position.paragraph, position.offset
    )
}

/// Make an edit a client asked for, as one undo step. The caller has already checked that edits
/// are allowed and that the writer approved this client; this only validates and applies. A
/// bad range or style is an error and leaves the document untouched.
pub fn apply_edit(request: &Request, view: Option<&mut DocView>) -> HostResult {
    let view = view.ok_or(HostError::NoDocument)?;
    if let Some(id) = request.document()
        && id != view.id()
    {
        return Err(HostError::UnknownDocument(id));
    }
    let id = view.id();
    let message = match request {
        Request::ReplaceRange {
            start, end, text, ..
        } => {
            let (from, to) = (resolve(view, *start)?, resolve(view, *end)?);
            view.edit(|editor| editor.replace_range(from, to, text))
                .map_err(host_error)?;
            if text.is_empty() {
                format!("Deleted the text from {} to {}", place(*start), place(*end))
            } else if start == end {
                format!("Inserted {} bytes at {}", text.len(), place(*start))
            } else {
                format!(
                    "Replaced the text from {} to {} with {} bytes",
                    place(*start),
                    place(*end),
                    text.len()
                )
            }
        }
        Request::FormatRange {
            start, end, format, ..
        } => {
            let (from, to) = (resolve(view, *start)?, resolve(view, *end)?);
            let patch = char_format(*format);
            view.edit(|editor| editor.format_range(from, to, &patch))
                .map_err(host_error)?;
            format!(
                "Changed the formatting from {} to {}",
                place(*start),
                place(*end)
            )
        }
        Request::SetParagraphStyle {
            paragraph, style, ..
        } => {
            let (style_id, name) = find_style(view, style)?;
            if view.document().paragraph_at(*paragraph).is_none() {
                return Err(no_such_paragraph(view, *paragraph));
            }
            view.edit(|editor| editor.set_paragraph_style_at(*paragraph, style_id))
                .map_err(host_error)?;
            format!("Gave paragraph {paragraph} the style \"{name}\"")
        }
        _ => return Err(HostError::Other("that is not an edit".into())),
    };
    Ok(Reply::Edited {
        document: id,
        message,
    })
}

fn char_format(patch: FormatPatch) -> CharFormat {
    CharFormat {
        weight: patch.bold.map(|bold| {
            if bold {
                libertum_model::BOLD
            } else {
                libertum_model::REGULAR
            }
        }),
        italic: patch.italic,
        underline: patch.underline,
        strikethrough: patch.strikethrough,
        ..CharFormat::default()
    }
}

/// The paragraph style called `wanted` (ignoring case), with its proper name.
fn find_style(view: &DocView, wanted: &str) -> Result<(StyleId, String), HostError> {
    let styles = &view.document().styles.paragraph;
    styles
        .iter()
        .enumerate()
        .find(|(_, style)| style.name.eq_ignore_ascii_case(wanted))
        .and_then(|(index, style)| {
            u32::try_from(index)
                .ok()
                .map(|i| (StyleId(i), style.name.clone()))
        })
        .ok_or_else(|| {
            HostError::Other(format!(
                "no paragraph style is called \"{wanted}\"; get_styles lists them"
            ))
        })
}

/// What the approval prompt shows about an edit: a one-line action and the text involved.
pub struct EditSummary {
    pub action: String,
    pub detail: String,
}

/// Cut `text` to about `max` characters for display, marking the cut.
fn shorten(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

fn span(verb: &str, start: &Position, end: &Position) -> String {
    format!(
        "{verb} {} {}, {} {} {} {}, {} {}",
        s::MCP_EDIT_FROM,
        start.paragraph,
        s::MCP_EDIT_OFFSET,
        start.offset,
        s::MCP_EDIT_TO,
        end.paragraph,
        s::MCP_EDIT_OFFSET,
        end.offset
    )
}

/// Describe an edit for the writer, with the text it would replace when that is in one paragraph.
pub fn describe(request: &Request, view: Option<&DocView>) -> EditSummary {
    match request {
        Request::ReplaceRange {
            start, end, text, ..
        } => {
            let old = view.and_then(|view| {
                if start.paragraph != end.paragraph || start.offset >= end.offset {
                    return None;
                }
                let paragraph = view.document().paragraph_at(start.paragraph)?;
                paragraph
                    .text()
                    .get(start.offset..end.offset)
                    .map(|t| shorten(t, 200))
            });
            let action = if start == end {
                format!(
                    "{} {} {}, {} {}",
                    s::MCP_EDIT_INSERT,
                    s::MCP_EDIT_AT,
                    start.paragraph,
                    s::MCP_EDIT_OFFSET,
                    start.offset
                )
            } else {
                let verb = if text.is_empty() {
                    s::MCP_EDIT_DELETE
                } else {
                    s::MCP_EDIT_REPLACE
                };
                span(verb, start, end)
            };
            let mut detail = String::new();
            if let Some(old) = old {
                detail.push_str(&format!("{} {old}\n", s::MCP_EDIT_OLD_TEXT));
            }
            if !text.is_empty() {
                detail.push_str(&format!("{} {}", s::MCP_EDIT_NEW_TEXT, shorten(text, 300)));
            }
            EditSummary { action, detail }
        }
        Request::FormatRange {
            start, end, format, ..
        } => {
            let parts: Vec<String> = [
                ("bold", format.bold),
                ("italic", format.italic),
                ("underline", format.underline),
                ("strikethrough", format.strikethrough),
            ]
            .into_iter()
            .filter_map(|(name, value)| {
                value.map(|on| format!("{name} {}", if on { "on" } else { "off" }))
            })
            .collect();
            EditSummary {
                action: span(s::MCP_EDIT_FORMAT, start, end),
                detail: parts.join(", "),
            }
        }
        Request::SetParagraphStyle {
            paragraph, style, ..
        } => EditSummary {
            action: format!("{} {} {paragraph}", s::MCP_EDIT_STYLE, s::MCP_EDIT_AT),
            detail: format!("{} {}", s::MCP_EDIT_STYLE_NAMED, shorten(style, 80)),
        },
        _ => EditSummary {
            action: String::new(),
            detail: String::new(),
        },
    }
}

/// A client's self-declared name made safe to show and to remember: control characters out,
/// whitespace tidied and the length bounded.
pub fn display_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let tidy = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if tidy.is_empty() {
        "an unnamed tool".to_owned()
    } else {
        shorten(&tidy, 60)
    }
}

fn info(view: &DocView, meta: &DocMeta<'_>) -> DocumentInfo {
    DocumentInfo {
        id: view.id(),
        title: meta.title.to_owned(),
        path: meta.path.map(|p| p.display().to_string()),
        format: meta.format.to_owned(),
        paragraphs: view.document().paragraph_count(),
        dirty: meta.dirty,
        active: true,
    }
}

fn position(view: &DocView, at: DocPosition) -> Position {
    Position {
        paragraph: view.document().index_of(at.block).unwrap_or(0),
        offset: at.offset,
    }
}

/// Up to about 60 bytes either side of a match, cut on character boundaries.
fn context_around(text: &str, start: usize, end: usize) -> String {
    const SIDE: usize = 60;
    let from = text.floor_char_boundary(start.saturating_sub(SIDE));
    let to = text.ceil_char_boundary((end + SIDE).min(text.len()));
    text[from..to].to_owned()
}

/// One question from a client, handed to the UI thread. Cloneable because Iced messages are;
/// only the first `take` gets it.
#[derive(Clone)]
pub struct McpRequest(Arc<Mutex<Option<Pending>>>);

impl McpRequest {
    pub fn new(pending: Pending) -> Self {
        Self(Arc::new(Mutex::new(Some(pending))))
    }

    pub fn take(&self) -> Option<Pending> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl std::fmt::Debug for McpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpRequest")
    }
}

/// Where the UI thread's subscription reads the server's questions from. Two feeds are the
/// same subscription when their ids match.
#[derive(Clone)]
pub struct McpFeed {
    id: u64,
    receiver: Arc<Mutex<Option<Receiver<Pending>>>>,
}

impl Hash for McpFeed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl McpFeed {
    /// The stream for `Subscription::run_with`: a thread moves each question from the server's
    /// queue into the UI's message stream, and ends when the server stops.
    pub fn stream(&self) -> UnboundedReceiver<McpRequest> {
        let (tx, rx) = unbounded();
        let taken = self
            .receiver
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(queue) = taken {
            let spawned = std::thread::Builder::new()
                .name("libertum-mcp-feed".into())
                .spawn(move || {
                    while let Ok(pending) = queue.recv() {
                        let request = McpRequest::new(pending);
                        if tx.unbounded_send(request).is_err() {
                            return;
                        }
                    }
                });
            if let Err(error) = spawned {
                tracing::warn!(%error, "could not start the MCP request feed");
            }
        }
        rx
    }
}

static NEXT_FEED_ID: AtomicU64 = AtomicU64::new(1);

/// The running server and the feed the UI thread drains. Dropping it stops the server.
pub struct McpService {
    server: RunningServer,
    feed: McpFeed,
}

impl McpService {
    /// Start the server on the default port, or on any free port if that one is taken.
    pub fn start() -> Result<Self, libertum_mcp::ServerError> {
        let (host, receiver): (Arc<ChannelHost>, _) =
            libertum_mcp::channel(ANSWER_TIMEOUT, EDIT_TIMEOUT);
        let server = match RunningServer::start(host.clone(), libertum_mcp::DEFAULT_PORT) {
            Ok(server) => server,
            Err(libertum_mcp::ServerError::Bind(error)) => {
                tracing::info!(%error, "the default MCP port is busy; using a free one");
                RunningServer::start(host, 0)?
            }
            Err(error) => return Err(error),
        };
        let feed = McpFeed {
            id: NEXT_FEED_ID.fetch_add(1, Ordering::Relaxed),
            receiver: Arc::new(Mutex::new(Some(receiver))),
        };
        Ok(Self { server, feed })
    }

    pub fn feed(&self) -> McpFeed {
        self.feed.clone()
    }

    pub fn url(&self) -> String {
        self.server.url()
    }

    pub fn token(&self) -> &str {
        self.server.token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libertum_model::{BlockId, Document, Paragraph};

    fn meta() -> DocMeta<'static> {
        DocMeta {
            title: "Book",
            path: None,
            format: "odt",
            dirty: true,
        }
    }

    fn view(lines: &[&str]) -> DocView {
        // Ids of 0 are reallocated by `from_paragraphs`.
        let paragraphs = lines
            .iter()
            .map(|t| Paragraph::new(BlockId(0), *t))
            .collect();
        DocView::open(Document::from_paragraphs(paragraphs)).0
    }

    #[test]
    fn with_no_document_listing_is_empty_and_the_rest_fail() {
        let Ok(Reply::Documents(documents)) = answer(&Request::ListDocuments, None, &meta()) else {
            panic!("listing works without a document");
        };
        assert!(documents.is_empty());
        let outline = answer(&Request::Outline { document: None }, None, &meta());
        assert_eq!(outline, Err(HostError::NoDocument));
    }

    #[test]
    fn lists_the_open_document_with_its_own_id() {
        let a = view(&["one", "two", "three"]);
        let b = view(&["x"]);
        assert_ne!(a.id(), b.id(), "each open document has its own id");
        let Ok(Reply::Documents(documents)) = answer(&Request::ListDocuments, Some(&a), &meta())
        else {
            panic!("a listing");
        };
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].id, a.id());
        assert_eq!(documents[0].paragraphs, 3);
        assert!(documents[0].dirty && documents[0].active);
    }

    #[test]
    fn a_stale_document_id_is_refused() {
        let a = view(&["one"]);
        let result = answer(
            &Request::ReadText {
                document: Some(a.id() + 1000),
                start_paragraph: 0,
                count: 5,
            },
            Some(&a),
            &meta(),
        );
        assert_eq!(result, Err(HostError::UnknownDocument(a.id() + 1000)));
    }

    #[test]
    fn reads_a_window_of_paragraphs_and_says_when_more_follow() {
        let a = view(&["p0", "p1", "p2", "p3", "p4"]);
        let read = |start, count| {
            let Ok(Reply::Text(slice)) = answer(
                &Request::ReadText {
                    document: None,
                    start_paragraph: start,
                    count,
                },
                Some(&a),
                &meta(),
            ) else {
                panic!("text");
            };
            slice
        };
        let middle = read(1, 2);
        assert_eq!(middle.paragraphs, vec!["p1", "p2"]);
        assert!(middle.truncated);
        assert_eq!(middle.total_paragraphs, 5);
        let tail = read(3, 50);
        assert_eq!(tail.paragraphs, vec!["p3", "p4"]);
        assert!(!tail.truncated);
        let past = read(99, 5);
        assert!(past.paragraphs.is_empty());
        assert_eq!(past.start_paragraph, 5, "clamped to the end");
    }

    #[test]
    fn the_outline_lists_headings_with_their_paragraph_numbers() {
        let mut document = Document::from_paragraphs(vec![
            Paragraph::new(BlockId(0), "Chapter One"),
            Paragraph::new(BlockId(0), "Some text."),
            Paragraph::new(BlockId(0), "Section"),
        ]);
        for (index, level) in [(0usize, 1u8), (2, 2)] {
            let id = document.paragraph_ids().nth(index).expect("paragraph");
            let mut props = document.para_props(id).expect("props");
            props.outline_level = Some(level);
            document
                .apply(libertum_model::Operation::SetParaProps { block: id, props })
                .expect("set the level");
        }
        let (view, _events) = DocView::open(document);
        let Ok(Reply::Outline { headings, .. }) =
            answer(&Request::Outline { document: None }, Some(&view), &meta())
        else {
            panic!("an outline");
        };
        assert_eq!(headings.len(), 2);
        assert_eq!((headings[0].paragraph, headings[0].level), (0, 1));
        assert_eq!(headings[0].text, "Chapter One");
        assert_eq!((headings[1].paragraph, headings[1].level), (2, 2));
    }

    #[test]
    fn styles_name_their_base() {
        let a = view(&["x"]);
        let Ok(Reply::Styles { styles, .. }) =
            answer(&Request::Styles { document: None }, Some(&a), &meta())
        else {
            panic!("styles");
        };
        assert!(!styles.is_empty());
        let heading = styles
            .iter()
            .find(|s| s.name.starts_with("Heading"))
            .expect("a heading style");
        assert!(styles.iter().all(|s| !s.name.is_empty()), "{heading:?}");
    }

    #[test]
    fn the_selection_is_reported_with_its_text() {
        let mut a = view(&["hello world", "second"]);
        let first = a.document().first_id().expect("first");
        a.move_caret(|editor| {
            editor.set_selection(
                DocPosition {
                    block: first,
                    offset: 0,
                },
                DocPosition {
                    block: first,
                    offset: 5,
                },
            );
        });
        let Ok(Reply::Selection(selection)) =
            answer(&Request::Selection { document: None }, Some(&a), &meta())
        else {
            panic!("a selection");
        };
        assert!(!selection.is_empty);
        assert_eq!(selection.text, "hello");
        assert_eq!(
            selection.anchor,
            Position {
                paragraph: 0,
                offset: 0
            }
        );
        assert_eq!(
            selection.head,
            Position {
                paragraph: 0,
                offset: 5
            }
        );
    }

    #[test]
    fn search_finds_matches_and_honours_case_and_limit() {
        let a = view(&["Apple pie", "apple tart", "cherry"]);
        let search = |case_sensitive, limit| {
            let Ok(Reply::Search(results)) = answer(
                &Request::Search {
                    document: None,
                    query: "apple".into(),
                    case_sensitive,
                    limit,
                },
                Some(&a),
                &meta(),
            ) else {
                panic!("results");
            };
            results
        };
        let loose = search(false, 10);
        assert_eq!(loose.matches.len(), 2);
        assert_eq!(loose.matches[1].paragraph, 1);
        assert!(!loose.truncated);
        let exact = search(true, 10);
        assert_eq!(exact.matches.len(), 1, "capital A does not match");
        let limited = search(false, 1);
        assert_eq!(limited.matches.len(), 1);
        assert!(limited.truncated);
    }

    #[test]
    fn context_stays_on_character_boundaries() {
        let text = format!("{}needle{}", "é".repeat(100), "ü".repeat(100));
        let start = "é".len() * 100;
        let context = context_around(&text, start, start + 6);
        assert!(context.contains("needle"));
        assert!(context.len() < text.len());
    }
}
