//! The MCP client: starts another program that speaks MCP over standard input and output, lists
//! its tools and calls them on the writer's behalf (`docs/mcp-plan.md`, phase 4).
//!
//! Nothing here decides whether a program may be started; the application asks the writer first
//! and only then calls [`Connection::start`]. The API is blocking, so the application can call it
//! from a worker thread; the protocol itself runs on a thread of this crate with its own small
//! tokio runtime, as the server does.

use std::thread::JoinHandle;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, ClientCapabilities, ClientConfig, ContentBlock, Implementation,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::{ServiceExt, service::RunningService};
use serde_json::{Map, Value};
use tokio::sync::{mpsc, oneshot};

/// Longest a server may take to start and answer the opening handshake.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Longest a single call may run before the writer gets an error instead.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// A tool result is cut to this many bytes before it reaches the writer.
pub const MAX_OUTPUT_BYTES: usize = 200_000;
/// Windows flag that keeps a console window from flashing up for the child program.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// How to reach a server: a program started with its arguments, or a web address, exactly as the
/// writer approved it. A command that begins with `http://` or `https://` is a web address.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    /// A bearer token sent as `Authorization: Bearer <token>`, for a web address only. It is
    /// sent only where [`check_http_url`] allows (https, or this computer), and never shown.
    pub bearer: Option<String>,
}

impl std::fmt::Debug for ServerSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerSpec")
            .field("command", &self.command)
            .field("args", &self.args)
            .field("bearer", &self.bearer.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

impl ServerSpec {
    /// A server reached over HTTP at `url`.
    pub fn http(url: impl Into<String>) -> Self {
        Self {
            command: url.into(),
            args: Vec::new(),
            bearer: None,
        }
    }

    /// The same server, sending `token` as a bearer token (a web address only).
    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }

    /// Whether this names a web address instead of a program.
    pub fn is_http(&self) -> bool {
        let lower = self.command.to_ascii_lowercase();
        lower.starts_with("http://") || lower.starts_with("https://")
    }

    /// The command as one line, as it is shown for approval and compared against it.
    pub fn display(&self) -> String {
        std::iter::once(self.command.as_str())
            .chain(self.args.iter().map(String::as_str))
            .map(|part| {
                if part.is_empty() || part.contains(char::is_whitespace) {
                    format!("\"{part}\"")
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// One tool a server offers.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// The tool's JSON Schema for its arguments (an object schema).
    pub input_schema: Value,
}

/// What a tool call returned, flattened to text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    /// The server said the call failed; `text` then holds its explanation.
    pub is_error: bool,
    /// The text was cut at [`MAX_OUTPUT_BYTES`].
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not start the program: {0}")]
    Spawn(String),
    #[error("the program did not complete the MCP handshake: {0}")]
    Handshake(String),
    #[error("the program did not answer in time")]
    Timeout,
    #[error("the connection is closed")]
    Closed,
    #[error("{0}")]
    Call(String),
}

enum Command {
    ListTools(oneshot::Sender<Result<Vec<ToolInfo>, ClientError>>),
    CallTool {
        name: String,
        arguments: Map<String, Value>,
        timeout: Duration,
        reply: oneshot::Sender<Result<ToolOutput, ClientError>>,
    },
    Close,
}

/// A running connection to one server program. Dropping it stops the program.
pub struct Connection {
    commands: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    server_name: String,
}

impl Connection {
    /// Start the program and complete the handshake. Blocks for at most [`CONNECT_TIMEOUT`].
    /// Call this only after the writer has approved `spec`.
    pub fn start(spec: &ServerSpec) -> Result<Self, ClientError> {
        let (commands, mut inbox) = mpsc::channel::<Command>(8);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String, ClientError>>();
        let spec = spec.clone();
        let thread = std::thread::Builder::new()
            .name("libertum-mcp-client".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(ClientError::Spawn(error.to_string())));
                        return;
                    }
                };
                runtime.block_on(async move {
                    let service = match connect(&spec).await {
                        Ok(service) => service,
                        Err(error) => {
                            let _ = ready_tx.send(Err(error));
                            return;
                        }
                    };
                    let name = service
                        .peer_info()
                        .and_then(|info| info.server_info.as_ref().map(|s| s.name.clone()))
                        .unwrap_or_default();
                    let _ = ready_tx.send(Ok(name));
                    while let Some(command) = inbox.recv().await {
                        match command {
                            Command::ListTools(reply) => {
                                let _ = reply.send(list_tools(&service).await);
                            }
                            Command::CallTool {
                                name,
                                arguments,
                                timeout,
                                reply,
                            } => {
                                let _ =
                                    reply.send(call_tool(&service, name, arguments, timeout).await);
                            }
                            Command::Close => break,
                        }
                    }
                    // Closing ends the child's input; a program that ignores that is killed when
                    // the transport is dropped.
                    let _ = tokio::time::timeout(Duration::from_secs(3), service.cancel()).await;
                });
            })
            .map_err(|error| ClientError::Spawn(error.to_string()))?;
        match ready_rx.recv_timeout(CONNECT_TIMEOUT + Duration::from_secs(2)) {
            Ok(Ok(server_name)) => Ok(Self {
                commands,
                thread: Some(thread),
                server_name,
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                // The thread cannot be stopped from here; telling it to close makes it exit as
                // soon as the handshake gives up.
                let _ = commands.try_send(Command::Close);
                Err(ClientError::Timeout)
            }
        }
    }

    /// The name the server announced, which may be empty.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// The tools the server offers.
    pub fn tools(&self) -> Result<Vec<ToolInfo>, ClientError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .blocking_send(Command::ListTools(reply))
            .map_err(|_| ClientError::Closed)?;
        answer.blocking_recv().map_err(|_| ClientError::Closed)?
    }

    /// Call one tool. `timeout` of `None` means [`CALL_TIMEOUT`].
    pub fn call(
        &self,
        name: &str,
        arguments: Map<String, Value>,
        timeout: Option<Duration>,
    ) -> Result<ToolOutput, ClientError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .blocking_send(Command::CallTool {
                name: name.to_string(),
                arguments,
                timeout: timeout.unwrap_or(CALL_TIMEOUT),
                reply,
            })
            .map_err(|_| ClientError::Closed)?;
        answer.blocking_recv().map_err(|_| ClientError::Closed)?
    }

    /// Stop the program and wait for the connection thread (a few seconds at most).
    pub fn close(mut self) {
        self.shut_down();
    }

    fn shut_down(&mut self) {
        let _ = self.commands.try_send(Command::Close);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("the MCP client thread panicked");
        }
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.shut_down();
    }
}

type Service = RunningService<rmcp::RoleClient, ClientConfig>;

/// Start the program with piped input and output.
fn spawn(program: &str, args: &[String]) -> std::io::Result<TokioChildProcess> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    TokioChildProcess::new(command)
}

/// Check that `url` may be contacted: https anywhere, plain http only to this computer (so
/// nothing a writer sends travels unencrypted), and no user name or password in the address.
pub fn check_http_url(url: &str) -> Result<(), ClientError> {
    let bad = |why: &str| ClientError::Spawn(why.to_string());
    let lower = url.to_ascii_lowercase();
    let (secure, rest) = if let Some(rest) = lower.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = lower.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(bad("the address must start with https://"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() {
        return Err(bad("the address has no host name"));
    }
    if authority.contains('@') {
        return Err(bad("the address must not contain a user name or password"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(bad("the address contains spaces or control characters"));
    }
    let host = if let Some(inner) = authority.strip_prefix('[') {
        inner.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if !secure && !local {
        return Err(bad(
            "plain http is only allowed for this computer (localhost); use https",
        ));
    }
    Ok(())
}

async fn connect(spec: &ServerSpec) -> Result<Service, ClientError> {
    let config = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("libertum", env!("CARGO_PKG_VERSION")),
    );
    // The two transports are different types, so the handshake is written once as a macro.
    macro_rules! handshake {
        ($transport:expr) => {
            match tokio::time::timeout(CONNECT_TIMEOUT, config.serve($transport)).await {
                Ok(Ok(service)) => Ok(service),
                Ok(Err(error)) => Err(ClientError::Handshake(error.to_string())),
                Err(_) => Err(ClientError::Timeout),
            }
        };
    }
    if spec.is_http() {
        if !spec.args.is_empty() {
            return Err(ClientError::Spawn(
                "a web address takes no further words".to_string(),
            ));
        }
        check_http_url(&spec.command)?;
        let mut http = StreamableHttpClientTransportConfig::with_uri(spec.command.clone());
        if let Some(token) = spec.bearer.as_deref().filter(|t| !t.is_empty()) {
            if token.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(ClientError::Spawn(
                    "the token contains spaces or control characters".to_string(),
                ));
            }
            http = http.auth_header(token);
        }
        return handshake!(StreamableHttpClientTransport::from_config(http));
    }
    let transport = match spawn(&spec.command, &spec.args) {
        Ok(transport) => transport,
        // On Windows most command-line tools (npx among them) are `.cmd` files, which a bare
        // name does not find. Try the same name with `.cmd`, searched on PATH like any other.
        #[cfg(windows)]
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && std::path::Path::new(&spec.command).extension().is_none() =>
        {
            spawn(&format!("{}.cmd", spec.command), &spec.args)
                .map_err(|_| ClientError::Spawn(error.to_string()))?
        }
        Err(error) => return Err(ClientError::Spawn(error.to_string())),
    };
    handshake!(transport)
}

async fn list_tools(service: &Service) -> Result<Vec<ToolInfo>, ClientError> {
    let tools = tokio::time::timeout(CALL_TIMEOUT, service.list_all_tools())
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(|error| ClientError::Call(error.to_string()))?;
    Ok(tools
        .into_iter()
        .map(|tool| ToolInfo {
            name: tool.name.to_string(),
            description: tool
                .description
                .map(|text| text.to_string())
                .unwrap_or_default(),
            input_schema: Value::Object((*tool.input_schema).clone()),
        })
        .collect())
}

async fn call_tool(
    service: &Service,
    name: String,
    arguments: Map<String, Value>,
    timeout: Duration,
) -> Result<ToolOutput, ClientError> {
    let mut params = CallToolRequestParams::new(name);
    if !arguments.is_empty() {
        params.arguments = Some(arguments);
    }
    let result = tokio::time::timeout(timeout, service.call_tool(params))
        .await
        .map_err(|_| ClientError::Timeout)?
        .map_err(|error| ClientError::Call(error.to_string()))?;
    let mut text = String::new();
    for block in &result.content {
        if !text.is_empty() {
            text.push('\n');
        }
        match block {
            ContentBlock::Text(content) => text.push_str(&content.text),
            ContentBlock::Image(_) => text.push_str("[an image was returned and left out]"),
            ContentBlock::Audio(_) => text.push_str("[audio was returned and left out]"),
            _ => text.push_str("[other content was returned and left out]"),
        }
    }
    if text.is_empty()
        && let Some(structured) = &result.structured_content
    {
        text = structured.to_string();
    }
    let truncated = truncate_text(&mut text, MAX_OUTPUT_BYTES);
    Ok(ToolOutput {
        text,
        is_error: result.is_error.unwrap_or(false),
        truncated,
    })
}

/// Cut `text` to at most `max` bytes on a character boundary; true if anything was removed.
fn truncate_text(text: &mut String, max: usize) -> bool {
    if text.len() <= max {
        return false;
    }
    text.truncate(text.floor_char_boundary(max));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_quotes_only_parts_that_need_it() {
        let spec = ServerSpec {
            command: "npx".into(),
            args: vec!["-y".into(), "my server".into(), String::new()],
            bearer: None,
        };
        assert_eq!(spec.display(), "npx -y \"my server\" \"\"");
    }

    #[test]
    fn truncation_keeps_whole_characters() {
        let mut text = "aé".repeat(4); // 12 bytes, é takes two
        assert!(truncate_text(&mut text, 4));
        assert_eq!(text, "aéa");
        let mut short = "abc".to_string();
        assert!(!truncate_text(&mut short, 10));
        assert_eq!(short, "abc");
    }

    #[test]
    fn web_addresses_are_checked_before_any_connection() {
        let ok = |url: &str| check_http_url(url).is_ok();
        assert!(ok("https://example.com/mcp"));
        assert!(ok("HTTPS://Example.com:8443/mcp?x=1"));
        assert!(ok("http://localhost:3000/mcp"));
        assert!(ok("http://127.0.0.1:3000/mcp"));
        assert!(ok("http://[::1]:3000/mcp"));
        // Plain http to another computer would send the writer's text unencrypted.
        assert!(!ok("http://example.com/mcp"));
        assert!(!ok("http://localhost.example.com/mcp"));
        // A look-alike host hidden behind a user name.
        assert!(!ok("https://localhost@evil.example/mcp"));
        assert!(!ok("http://127.0.0.1@evil.example/mcp"));
        assert!(!ok("https://"));
        assert!(!ok("https://exa mple.com"));
        assert!(!ok("ftp://example.com"));
    }

    #[test]
    fn a_command_starting_with_http_is_a_web_address() {
        assert!(ServerSpec::http("https://example.com/mcp").is_http());
        assert!(
            !ServerSpec {
                command: "npx".into(),
                args: vec!["https://x".into()],
                bearer: None,
            }
            .is_http()
        );
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let spec = ServerSpec {
            command: "libertum-no-such-program-0f3a".into(),
            args: Vec::new(),
            bearer: None,
        };
        match Connection::start(&spec) {
            Err(ClientError::Spawn(_)) => {}
            other => panic!("expected a spawn error, got {:?}", other.map(|_| ())),
        }
    }
}
