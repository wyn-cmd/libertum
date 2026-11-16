//! The MCP client against a real child process (`examples/stdio_fixture.rs`).

use std::path::PathBuf;
use std::time::Duration;

use libertum_mcp::{ClientError, Connection, MAX_OUTPUT_BYTES, ServerSpec};
use serde_json::{Map, Value, json};

/// `cargo test` builds the examples into `<profile dir>/examples`. Depending on cargo's layout
/// the test executable can sit several directories below that, so look upwards for it.
fn fixture() -> ServerSpec {
    let exe = std::env::current_exe().expect("test executable path");
    let name = if cfg!(windows) {
        "stdio_fixture.exe"
    } else {
        "stdio_fixture"
    };
    let path: PathBuf = exe
        .ancestors()
        .map(|dir| dir.join("examples").join(name))
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| panic!("fixture not built; searched above {}", exe.display()));
    ServerSpec {
        command: path.to_string_lossy().into_owned(),
        args: Vec::new(),
        bearer: None,
    }
}

fn args(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[test]
fn connects_lists_tools_and_calls_one() {
    let connection = Connection::start(&fixture()).expect("start");
    assert_eq!(connection.server_name(), "fixture");

    let tools = connection.tools().expect("tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(names, ["echo", "fail", "big", "hang"]);
    let echo = &tools[0];
    assert_eq!(echo.description, "Returns the text it is given.");
    assert_eq!(echo.input_schema["properties"]["text"]["type"], "string");

    let output = connection
        .call("echo", args(json!({"text": "héllo"})), None)
        .expect("call");
    assert_eq!(output.text, "héllo");
    assert!(!output.is_error);
    assert!(!output.truncated);
    connection.close();
}

#[test]
fn an_error_result_is_reported_not_raised() {
    let connection = Connection::start(&fixture()).expect("start");
    let output = connection.call("fail", Map::new(), None).expect("call");
    assert!(output.is_error);
    assert_eq!(output.text, "it went wrong");
}

#[test]
fn a_protocol_error_comes_back_as_a_call_error() {
    let connection = Connection::start(&fixture()).expect("start");
    // `echo` needs `text`.
    match connection.call("echo", Map::new(), None) {
        Err(ClientError::Call(message)) => assert!(message.contains("text"), "{message}"),
        other => panic!("expected a call error, got {other:?}"),
    }
    // The connection still works afterwards.
    assert!(connection.call("fail", Map::new(), None).is_ok());
}

#[test]
fn long_output_is_cut() {
    let connection = Connection::start(&fixture()).expect("start");
    let output = connection.call("big", Map::new(), None).expect("call");
    assert!(output.truncated);
    assert_eq!(output.text.len(), MAX_OUTPUT_BYTES);
}

#[test]
fn a_slow_call_times_out_and_the_connection_survives() {
    let connection = Connection::start(&fixture()).expect("start");
    let started = std::time::Instant::now();
    match connection.call("hang", Map::new(), Some(Duration::from_millis(300))) {
        Err(ClientError::Timeout) => {}
        other => panic!("expected a timeout, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    let output = connection
        .call("echo", args(json!({"text": "still here"})), None)
        .expect("call after timeout");
    assert_eq!(output.text, "still here");
}

#[test]
fn a_program_that_is_not_an_mcp_server_fails_the_handshake() {
    // A program that exits at once: Windows and Unix both have one that needs no arguments.
    let spec = if cfg!(windows) {
        ServerSpec {
            command: "cmd".into(),
            args: vec!["/C".into(), "exit".into(), "0".into()],
            bearer: None,
        }
    } else {
        ServerSpec {
            command: "true".into(),
            args: Vec::new(),
            bearer: None,
        }
    };
    match Connection::start(&spec) {
        Err(ClientError::Handshake(_) | ClientError::Timeout) => {}
        other => panic!(
            "expected the handshake to fail, got {:?}",
            other.map(|_| ())
        ),
    }
}

/// Windows tools such as `npx` are `.cmd` files; naming them without the extension must work.
#[cfg(windows)]
#[test]
fn a_cmd_file_is_found_without_its_extension() {
    let fixture = fixture();
    let dir = std::env::temp_dir().join(format!("libertum-cmd-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let wrapper = dir.join("wrapper.cmd");
    std::fs::write(
        &wrapper,
        format!("@echo off\r\n\"{}\" %*\r\n", fixture.command),
    )
    .expect("write wrapper");
    let spec = ServerSpec {
        command: dir.join("wrapper").to_string_lossy().into_owned(),
        args: Vec::new(),
        bearer: None,
    };
    let result = Connection::start(&spec).map(|c| c.server_name().to_owned());
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(result.expect("start through the .cmd file"), "fixture");
}

/// Runs the fixture as an HTTP server and returns it with its port; dropping it kills the child.
struct HttpFixture {
    child: std::process::Child,
    port: u16,
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn http_fixture() -> HttpFixture {
    http_fixture_with(&[])
}

fn http_fixture_with(extra: &[&str]) -> HttpFixture {
    use std::io::{BufRead, BufReader};
    let spec = fixture();
    let mut child = std::process::Command::new(&spec.command)
        .arg("--http")
        .args(extra)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("start the fixture");
    let stdout = child.stdout.take().expect("fixture output");
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .expect("fixture port line");
    let port = line
        .trim()
        .strip_prefix("port ")
        .and_then(|n| n.parse().ok())
        .expect("a port number");
    HttpFixture { child, port }
}

#[test]
fn a_bearer_token_is_sent_and_a_missing_or_wrong_one_is_refused() {
    let server = http_fixture_with(&["--token", "s3cret-token"]);
    let url = format!("http://127.0.0.1:{}/mcp", server.port);
    assert!(Connection::start(&ServerSpec::http(url.clone())).is_err());
    assert!(Connection::start(&ServerSpec::http(url.clone()).with_bearer("wrong")).is_err());
    let connection = Connection::start(&ServerSpec::http(url).with_bearer("s3cret-token"))
        .expect("connect with the token");
    let out = connection
        .call("echo", args(json!({"text": "authorised"})), None)
        .expect("call");
    assert_eq!(out.text, "authorised");
    connection.close();
}

#[test]
fn a_token_is_never_sent_over_plain_http_to_another_computer() {
    let spec = ServerSpec::http("http://example.com/mcp").with_bearer("s3cret-token");
    match Connection::start(&spec) {
        Err(ClientError::Spawn(message)) => assert!(message.contains("https"), "{message}"),
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
    let spaced = ServerSpec::http("https://example.com/mcp").with_bearer("two words");
    assert!(matches!(
        Connection::start(&spaced),
        Err(ClientError::Spawn(_))
    ));
}

#[test]
fn a_server_spec_never_prints_its_token() {
    let spec = ServerSpec::http("https://example.com/mcp").with_bearer("s3cret-token");
    assert!(!format!("{spec:?}").contains("s3cret-token"));
}

#[test]
fn connects_over_http_lists_tools_and_calls_one() {
    let server = http_fixture();
    let spec = ServerSpec::http(format!("http://127.0.0.1:{}/mcp", server.port));
    let connection = Connection::start(&spec).expect("connect over http");
    assert_eq!(connection.server_name(), "fixture");
    let tools = connection.tools().expect("tools");
    assert_eq!(tools.len(), 4);
    let out = connection
        .call("echo", args(json!({"text": "over the wire"})), None)
        .expect("call");
    assert_eq!(out.text, "over the wire");
    assert!(!out.is_error);
    connection.close();
}

#[test]
fn http_to_another_computer_without_tls_is_refused_before_connecting() {
    match Connection::start(&ServerSpec::http("http://example.com/mcp")) {
        Err(ClientError::Spawn(message)) => assert!(message.contains("https"), "{message}"),
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
}

#[test]
fn an_http_server_that_is_not_there_is_a_handshake_error() {
    // Port 9 (discard) is not listening on a normal machine.
    match Connection::start(&ServerSpec::http("http://127.0.0.1:9/mcp")) {
        Err(ClientError::Handshake(_)) | Err(ClientError::Timeout) => {}
        other => panic!("expected a connection error, got {:?}", other.map(|_| ())),
    }
}
