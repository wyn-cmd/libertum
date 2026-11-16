//! A tiny MCP server over standard input and output, used by `tests/client.rs` to exercise the
//! client against a real child process. Tools: `echo` (returns its `text` argument), `fail`
//! (reports an error result), `big` (returns far more text than the client keeps) and `hang`
//! (never answers within a test's patience).

use std::sync::Arc;
use std::time::Duration;

use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::ServiceExt;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use serde_json::json;

struct Fixture;

impl ServerHandler for Fixture {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fixture", "0.0.1"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let schema =
            |value: serde_json::Value| Arc::new(value.as_object().cloned().unwrap_or_default());
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new(
                "echo",
                "Returns the text it is given.",
                schema(json!({
                    "type": "object",
                    "properties": { "text": { "type": "string", "description": "What to repeat" } },
                    "required": ["text"]
                })),
            ),
            Tool::new(
                "fail",
                "Always reports an error.",
                schema(json!({"type": "object"})),
            ),
            Tool::new(
                "big",
                "Returns a very long text.",
                schema(json!({"type": "object"})),
            ),
            Tool::new("hang", "Takes too long.", schema(json!({"type": "object"}))),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let text = |text: String| vec![ContentBlock::text(text)];
        match request.name.as_ref() {
            "echo" => {
                let said = request
                    .arguments
                    .as_ref()
                    .and_then(|args| args.get("text"))
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| McpError::invalid_params("text is required", None))?;
                Ok(CallToolResult::success(text(said.to_string())).into())
            }
            "fail" => Ok(CallToolResult::error(text("it went wrong".into())).into()),
            "big" => Ok(CallToolResult::success(text("x".repeat(500_000))).into()),
            "hang" => {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(CallToolResult::success(text("late".into())).into())
            }
            other => Err(McpError::invalid_params(format!("no tool {other}"), None)),
        }
    }
}

fn main() {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    // `--http` serves the same tools over HTTP on a free local port (printed as `port <n>`),
    // with no token, for the HTTP client test. Otherwise it speaks MCP on stdin and stdout.
    if std::env::args().any(|arg| arg == "--http") {
        runtime.block_on(serve_http());
        return;
    }
    runtime.block_on(async {
        if let Ok(service) = Fixture.serve(rmcp::transport::stdio()).await {
            let _ = service.waiting().await;
        }
    });
}

async fn serve_http() {
    use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
    use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("bind failed: {error}");
            return;
        }
    };
    let Ok(addr) = listener.local_addr() else {
        return;
    };
    println!("port {}", addr.port());
    let service = StreamableHttpService::new(
        || Ok(Fixture),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let mut app = axum::Router::new().nest_service("/mcp", service);
    // `--token <t>` makes the server refuse any request without `Authorization: Bearer <t>`.
    let args: Vec<String> = std::env::args().collect();
    if let Some(token) = args
        .iter()
        .position(|a| a == "--token")
        .and_then(|i| args.get(i + 1))
    {
        let expected = format!("Bearer {token}");
        app = app.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let ok = request
                    .headers()
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    == Some(expected.as_str());
                async move {
                    if ok {
                        next.run(request).await
                    } else {
                        axum::response::IntoResponse::into_response(
                            axum::http::StatusCode::UNAUTHORIZED,
                        )
                    }
                }
            },
        ));
    }
    let _ = axum::serve(listener, app).await;
}
