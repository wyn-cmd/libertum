//! The MCP server: an rmcp handler over Streamable HTTP, bound to the loopback address only
//! and guarded by a random bearer token.
//!
//! It runs on its own thread with its own small tokio runtime, so the application's Iced
//! executor and event loop never see it.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use crate::host::Host;
use crate::protocol::ClientInfo;
use crate::tools::{self, ToolError};

/// The port Libertum tries first. If it is taken the server uses any free port instead.
pub const DEFAULT_PORT: u16 = 49821;

/// The path clients connect to.
pub const ENDPOINT_PATH: &str = "/mcp";

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("could not open the local port: {0}")]
    Bind(#[from] std::io::Error),
    #[error("could not make a secret token: {0}")]
    Token(String),
}

struct LibertumServer {
    host: Arc<dyn Host>,
}

impl ServerHandler for LibertumServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("libertum", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Libertum is a word processor. These tools read the documents open in it and, \
                 only when the writer has turned on Allow edits and approved this app, change \
                 them (insert_text, replace_range, format_range, set_paragraph_style). \
                 Paragraphs are numbered from 0 and offsets are UTF-8 byte offsets within a \
                 paragraph. Read first, then edit: paragraph numbers shift when paragraphs are \
                 added or removed, and every edit is one undo step for the writer.",
            )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tool_list()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let host = Arc::clone(&self.host);
        // The name is whatever the client announced when it connected.
        let client = context
            .peer
            .peer_info()
            .map(|info| {
                ClientInfo::new(
                    info.client_info.name.clone(),
                    info.client_info.version.clone(),
                )
            })
            .filter(|client| !client.name.trim().is_empty())
            .unwrap_or_else(ClientInfo::unknown);
        let name = request.name.to_string();
        let args = request.arguments.unwrap_or_default();
        // The host may block while the application answers, so keep it off the async threads.
        let outcome =
            tokio::task::spawn_blocking(move || tools::call(&*host, &client, &name, &args))
                .await
                .map_err(|error| {
                    McpError::internal_error(format!("tool task failed: {error}"), None)
                })?;
        match outcome {
            Ok(text) => Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into()),
            Err(error @ (ToolError::InvalidParams(_) | ToolError::UnknownTool(_))) => {
                Err(McpError::invalid_params(error.to_string(), None))
            }
            Err(ToolError::Host(error)) => {
                Ok(CallToolResult::error(vec![ContentBlock::text(error.to_string())]).into())
            }
        }
    }
}

fn tool_list() -> Vec<Tool> {
    tools::definitions()
        .into_iter()
        .map(|def| {
            let schema = def.schema.as_object().cloned().unwrap_or_default();
            let mut tool = Tool::new(def.name, def.description, Arc::new(schema));
            tool.annotations = Some(if def.writes {
                // Edits are undoable, but a replace can remove text, so say so.
                ToolAnnotations::new()
                    .read_only(false)
                    .destructive(true)
                    .idempotent(false)
                    .open_world(false)
            } else {
                ToolAnnotations::new().read_only(true)
            });
            tool
        })
        .collect()
}

/// A random 256-bit token as 64 hex digits.
fn new_token() -> Result<String, ServerError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| ServerError::Token(error.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Compare without stopping at the first difference.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(State(token): State<Arc<str>>, request: Request, next: Next) -> Response {
    let allowed = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|given| same_secret(given.as_bytes(), token.as_bytes()));
    if allowed {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"))],
            "A bearer token is required.",
        )
            .into_response()
    }
}

/// A running server. Dropping it stops the server.
pub struct RunningServer {
    addr: SocketAddr,
    token: String,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl RunningServer {
    /// Start listening on `127.0.0.1:port` (`0` picks a free port). Fails if the port is taken
    /// or the thread cannot start; nothing keeps running in that case.
    pub fn start(host: Arc<dyn Host>, port: u16) -> Result<Self, ServerError> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let token = new_token()?;
        let (shutdown, stop) = tokio::sync::oneshot::channel::<()>();

        let secret: Arc<str> = Arc::from(token.as_str());
        let thread = std::thread::Builder::new()
            .name("libertum-mcp".into())
            .spawn(move || run(listener, host, secret, stop))?;

        tracing::info!(%addr, "MCP server listening");
        Ok(Self {
            addr,
            token,
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The URL an MCP client connects to.
    pub fn url(&self) -> String {
        format!("http://{}{}", self.addr, ENDPOINT_PATH)
    }

    /// The secret clients send as `Authorization: Bearer <token>`.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Stop the server and wait for its thread (at most a couple of seconds).
    pub fn stop(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("the MCP server thread panicked");
        }
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.shut_down();
    }
}

fn run(
    listener: TcpListener,
    host: Arc<dyn Host>,
    secret: Arc<str>,
    stop: tokio::sync::oneshot::Receiver<()>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "could not start the MCP runtime");
            return;
        }
    };
    runtime.block_on(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(error) => {
                tracing::error!(%error, "could not use the MCP port");
                return;
            }
        };
        let config = StreamableHttpServerConfig::default();
        let cancel = config.cancellation_token.clone();
        let service = StreamableHttpService::new(
            move || {
                Ok(LibertumServer {
                    host: Arc::clone(&host),
                })
            },
            Arc::new(LocalSessionManager::default()),
            config,
        );
        let app = Router::new()
            .nest_service(ENDPOINT_PATH, service)
            .layer(middleware::from_fn_with_state(secret, require_token));

        let finished = cancel.clone();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { finished.cancelled().await })
                .await
        });
        // A dropped sender counts as a stop too.
        let _ = stop.await;
        cancel.cancel();
        if tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .is_err()
        {
            tracing::warn!("the MCP server did not close its connections in time");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{HostError, HostResult, Request as HostRequest};
    use std::io::{Read, Write};
    use std::net::TcpStream;

    struct Nobody;
    impl Host for Nobody {
        fn request(&self, _client: &ClientInfo, _request: HostRequest) -> HostResult {
            Err(HostError::NoDocument)
        }
    }

    fn post(
        addr: SocketAddr,
        authorization: Option<&str>,
        host_header: &str,
        body: &str,
    ) -> String {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("timeout");
        let auth = authorization
            .map(|value| format!("Authorization: {value}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST {ENDPOINT_PATH} HTTP/1.1\r\nHost: {host_header}\r\n{auth}\
             Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("write");
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        // The reply may be an event stream that stays open; read what arrives.
        while let Ok(n) = stream.read(&mut chunk) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
            if String::from_utf8_lossy(&out).contains("\"serverInfo\"") {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;

    #[test]
    fn tokens_are_long_and_different() {
        let a = new_token().expect("token");
        let b = new_token().expect("token");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn secrets_compare_exactly() {
        assert!(same_secret(b"abc", b"abc"));
        assert!(!same_secret(b"abc", b"abd"));
        assert!(!same_secret(b"abc", b"abcd"));
        assert!(!same_secret(b"", b"a"));
    }

    #[test]
    fn only_the_edit_tools_are_marked_as_writing() {
        let tools = tool_list();
        let defs = tools::definitions();
        assert_eq!(tools.len(), defs.len());
        for (tool, def) in tools.iter().zip(&defs) {
            let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
            assert_eq!(read_only, Some(!def.writes), "{}", tool.name);
        }
        let writing: Vec<_> = defs.iter().filter(|d| d.writes).map(|d| d.name).collect();
        assert_eq!(
            writing,
            [
                "insert_text",
                "replace_range",
                "format_range",
                "set_paragraph_style"
            ]
        );
    }

    #[test]
    fn the_server_listens_on_loopback_and_demands_the_token() {
        let server = RunningServer::start(Arc::new(Nobody), 0).expect("start");
        let addr = server.addr();
        assert!(addr.ip().is_loopback(), "only the loopback address");
        let authority = addr.to_string();

        let none = post(addr, None, &authority, INITIALIZE);
        assert!(none.starts_with("HTTP/1.1 401"), "{none}");

        let wrong = post(addr, Some("Bearer not-the-token"), &authority, INITIALIZE);
        assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong}");

        let right = format!("Bearer {}", server.token());
        let ok = post(addr, Some(&right), &authority, INITIALIZE);
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("\"libertum\""), "the server names itself: {ok}");

        // A page in a browser cannot be tricked into talking to us under another name.
        let rebinding = post(addr, Some(&right), "evil.example", INITIALIZE);
        assert!(!rebinding.starts_with("HTTP/1.1 200"), "{rebinding}");

        server.stop();
        assert!(
            TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_err(),
            "the port is closed after stop"
        );
    }

    /// Send one JSON-RPC message in a session and return the status line, the headers and the
    /// body read until `until` appears (the reply may be a server-sent event stream).
    fn rpc(
        addr: SocketAddr,
        token: &str,
        session: Option<&str>,
        body: &str,
        until: &str,
    ) -> (String, String) {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("timeout");
        let session = session
            .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST {ENDPOINT_PATH} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n{session}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("write");
        let mut out = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Ok(n) = stream.read(&mut chunk) {
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
            if until.is_empty() {
                break;
            }
            // A long reply (the tool list) arrives in several reads: stop only once the event
            // that holds `until` is complete, that is, its `data:` line parses as JSON.
            let so_far = String::from_utf8_lossy(&out);
            if so_far.contains(until)
                && so_far.lines().any(|line| {
                    line.strip_prefix("data:").is_some_and(|d| {
                        serde_json::from_str::<serde_json::Value>(d.trim()).is_ok()
                    })
                })
            {
                break;
            }
        }
        let text = String::from_utf8_lossy(&out).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        (head.to_owned(), body.to_owned())
    }

    /// The first `data:` payload of an event stream, or the body itself if it is plain JSON.
    fn json_of(body: &str) -> serde_json::Value {
        let line = body
            .lines()
            .find_map(|line| {
                line.strip_prefix("data:")
                    .filter(|d| d.trim().starts_with('{'))
            })
            .unwrap_or(body);
        serde_json::from_str(line.trim()).unwrap_or_else(|e| panic!("json {e}: {body}"))
    }

    #[test]
    fn a_client_can_list_tools_and_call_one_over_the_wire() {
        struct OneDocument;
        impl Host for OneDocument {
            fn request(&self, _client: &ClientInfo, request: HostRequest) -> HostResult {
                match request {
                    HostRequest::ListDocuments => {
                        Ok(crate::Reply::Documents(vec![crate::DocumentInfo {
                            id: 7,
                            title: "Wire test".into(),
                            path: None,
                            format: "odt".into(),
                            paragraphs: 3,
                            dirty: false,
                            active: true,
                        }]))
                    }
                    _ => Err(HostError::NoDocument),
                }
            }
        }
        let server = RunningServer::start(Arc::new(OneDocument), 0).expect("start");
        let addr = server.addr();
        let token = server.token().to_owned();

        let (head, body) = rpc(addr, &token, None, INITIALIZE, "\"serverInfo\"");
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let session = head
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("mcp-session-id")
                    .then(|| value.trim().to_owned())
            })
            .expect("a session id");
        assert_eq!(json_of(&body)["result"]["serverInfo"]["name"], "libertum");

        let (head, _) = rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "",
        );
        assert!(head.starts_with("HTTP/1.1 202"), "{head}");

        let (_, body) = rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\"tools\"",
        );
        let listed = json_of(&body);
        let names: Vec<_> = listed["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| t["name"].as_str().expect("name").to_owned())
            .collect();
        for expected in [
            "list_documents",
            "read_text",
            "get_outline",
            "get_styles",
            "get_selection",
            "search_text",
            "insert_text",
            "replace_range",
            "format_range",
            "set_paragraph_style",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "{expected} in {names:?}"
            );
        }
        assert_eq!(
            listed["result"]["tools"][0]["annotations"]["readOnlyHint"],
            true
        );

        let (_, body) = rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_documents","arguments":{}}}"#,
            "Wire test",
        );
        let called = json_of(&body);
        let text = called["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        assert!(text.contains("\"Wire test\""), "{text}");
        assert_eq!(called["result"]["isError"], false);

        // A host failure is a tool error the client can read, not a protocol failure.
        let (_, body) = rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_outline","arguments":{}}}"#,
            "no document",
        );
        let failed = json_of(&body);
        assert_eq!(failed["result"]["isError"], true, "{failed}");

        // A bad argument is a protocol error.
        let (_, body) = rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"search_text","arguments":{}}}"#,
            "query",
        );
        assert!(json_of(&body).get("error").is_some(), "{body}");

        server.stop();
    }

    #[test]
    fn an_edit_reaches_the_host_with_the_clients_own_name() {
        struct Echo;
        impl Host for Echo {
            fn request(&self, client: &ClientInfo, request: HostRequest) -> HostResult {
                assert!(request.is_edit());
                Err(HostError::Other(format!("edit from {}", client.name)))
            }
        }
        let server = RunningServer::start(Arc::new(Echo), 0).expect("start");
        let addr = server.addr();
        let token = server.token().to_owned();
        let (head, _) = rpc(addr, &token, None, INITIALIZE, "\"serverInfo\"");
        let session = head
            .lines()
            .find_map(|l| {
                let (name, value) = l.split_once(':')?;
                name.eq_ignore_ascii_case("mcp-session-id")
                    .then(|| value.trim().to_owned())
            })
            .expect("a session id");
        rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "",
        );
        let (_, body) = rpc(
            addr,
            &token,
            Some(&session),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"insert_text","arguments":{"paragraph":0,"offset":0,"text":"hi"}}}"#,
            "edit from",
        );
        let reply = json_of(&body);
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        // INITIALIZE announces itself as "test".
        assert!(
            reply["result"]["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("edit from test"),
            "{reply}"
        );
        server.stop();
    }

    #[test]
    fn a_busy_port_is_an_error_not_a_hang() {
        let first = RunningServer::start(Arc::new(Nobody), 0).expect("start");
        let taken = first.addr().port();
        assert!(matches!(
            RunningServer::start(Arc::new(Nobody), taken),
            Err(ServerError::Bind(_))
        ));
    }
}
