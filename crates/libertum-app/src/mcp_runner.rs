//! The tool runner (developer mode, `docs/mcp-plan.md` phase 4): the writer picks another MCP
//! program, picks one of its tools, fills in a form made from the tool's schema and runs it by
//! hand. There is no assistant in the loop; nothing here runs without a button press.
//!
//! This module holds the pure parts (reading a command line, turning a JSON Schema into form
//! fields, turning the filled-in form back into arguments) and the runner's state. The window
//! and the messages are in `app.rs`.

use std::sync::Arc;

use libertum_mcp::{Connection, ServerSpec, ToolInfo, ToolOutput};
use serde_json::{Map, Number, Value};

/// Why a command line cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandLineError {
    Empty,
    /// A double quote was opened and never closed.
    UnclosedQuote,
    /// A control character, such as a line break, in the text.
    ControlCharacter,
}

/// Read a command line into a program and arguments. Words are split at spaces; a double quote
/// keeps spaces together (`"C:\Program Files\x\server.exe"`). There are no escapes, so a Windows
/// path with backslashes is taken literally, and nothing is expanded: no variables, no globs,
/// and no shell is involved.
pub fn parse_command_line(line: &str) -> Result<ServerSpec, CommandLineError> {
    if line.chars().any(char::is_control) {
        return Err(CommandLineError::ControlCharacter);
    }
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            ch if ch.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            ch => {
                current.push(ch);
                in_word = true;
            }
        }
    }
    if quoted {
        return Err(CommandLineError::UnclosedQuote);
    }
    if in_word {
        words.push(current);
    }
    let mut words = words.into_iter();
    let command = words.next().ok_or(CommandLineError::Empty)?;
    if command.is_empty() {
        return Err(CommandLineError::Empty);
    }
    Ok(ServerSpec {
        command,
        args: words.collect(),
        bearer: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Number,
    Integer,
    Boolean,
    /// Anything else (an object, an array, no declared type): typed as JSON.
    Json,
}

/// One input of a tool, as shown in the form.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub description: String,
    pub kind: FieldKind,
    pub required: bool,
    /// What is typed (every kind but Boolean).
    pub value: String,
    /// The tick box (Boolean only).
    pub checked: bool,
}

/// The form for a tool's input schema: required fields first, then the rest in name order.
/// A schema without properties gives an empty form.
pub fn fields_from_schema(schema: &Value) -> Vec<Field> {
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut fields: Vec<Field> = properties
        .iter()
        .map(|(name, property)| Field {
            name: name.clone(),
            description: property
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            kind: kind_of(property),
            required: required.contains(&name.as_str()),
            value: String::new(),
            checked: property
                .get("default")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect();
    fields.sort_by(|a, b| {
        b.required
            .cmp(&a.required)
            .then_with(|| a.name.cmp(&b.name))
    });
    fields
}

fn kind_of(property: &Value) -> FieldKind {
    // `type` is a name, or a list of names such as ["string", "null"].
    let declared = match property.get("type") {
        Some(Value::String(name)) => Some(name.as_str()),
        Some(Value::Array(names)) => names
            .iter()
            .filter_map(Value::as_str)
            .find(|name| *name != "null"),
        _ => None,
    };
    match declared {
        Some("string") => FieldKind::Text,
        Some("number") => FieldKind::Number,
        Some("integer") => FieldKind::Integer,
        Some("boolean") => FieldKind::Boolean,
        _ => FieldKind::Json,
    }
}

/// Why the form cannot be turned into arguments yet; the string is the field's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentError {
    Missing(String),
    NotANumber(String),
    NotAnInteger(String),
    NotJson(String),
}

/// The arguments the form describes. An empty optional field is left out and an empty required
/// one is an error. A tick box is sent when it is ticked or required (a required box that is
/// not ticked goes as `false`); an unticked optional box is left out.
pub fn build_arguments(fields: &[Field]) -> Result<Map<String, Value>, ArgumentError> {
    let mut out = Map::new();
    for field in fields {
        let typed = field.value.trim();
        let value = match field.kind {
            FieldKind::Boolean => {
                if field.checked || field.required {
                    Some(Value::Bool(field.checked))
                } else {
                    None
                }
            }
            _ if typed.is_empty() => {
                if field.required {
                    return Err(ArgumentError::Missing(field.name.clone()));
                }
                None
            }
            // Text is sent as typed, spaces and all.
            FieldKind::Text => Some(Value::String(field.value.clone())),
            FieldKind::Integer => Some(Value::Number(
                typed
                    .parse::<i64>()
                    .map_err(|_| ArgumentError::NotAnInteger(field.name.clone()))?
                    .into(),
            )),
            FieldKind::Number => Some(Value::Number(
                typed
                    .parse::<f64>()
                    .ok()
                    .and_then(Number::from_f64)
                    .ok_or_else(|| ArgumentError::NotANumber(field.name.clone()))?,
            )),
            FieldKind::Json => Some(
                serde_json::from_str(typed)
                    .map_err(|_| ArgumentError::NotJson(field.name.clone()))?,
            ),
        };
        if let Some(value) = value {
            out.insert(field.name.clone(), value);
        }
    }
    Ok(out)
}

/// What the runner window shows about the connection.
#[derive(Debug, Clone, Default)]
pub enum Link {
    #[default]
    Idle,
    /// Starting the program (a stale answer is told apart by the generation number).
    Connecting(usize),
    Connected {
        server: usize,
        connection: Arc<Connection>,
        /// The name the program announced.
        announced: String,
    },
}

/// The text a finished call left for the writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallResult {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}

impl From<ToolOutput> for CallResult {
    fn from(output: ToolOutput) -> Self {
        Self {
            text: output.text,
            is_error: output.is_error,
            truncated: output.truncated,
        }
    }
}

/// A recipe that is running: which step is next, and what it has kept so far.
#[derive(Debug)]
pub struct MacroRun {
    pub recipe: crate::macros::Recipe,
    /// Tells this run's tool answer from a stale one.
    pub serial: u64,
    pub next: usize,
    pub vars: std::collections::HashMap<String, String>,
    /// Where the tool answer being waited for is kept (`save_as`), if anywhere.
    pub pending_save: Option<String>,
    /// The text the last `get_selection` read; `replace_selection` insists it is unchanged.
    pub selection: Option<String>,
    /// Undo steps held before the run, and expected now. A difference means the writer edited
    /// the document meanwhile, and the run stops rather than edit on top of that.
    pub undo_before: usize,
    pub expected_undo: usize,
    pub edited: bool,
}

impl MacroRun {
    /// Keep a step's answer under `name`; fails when the recipe now holds too much text.
    pub fn keep(&mut self, name: String, value: String) -> Result<(), &'static str> {
        self.vars.insert(name, value);
        let total: usize = self.vars.values().map(String::len).sum();
        if total > crate::macros::MAX_VARIABLE_BYTES {
            Err(crate::strings::MACROS_ERR_TOO_MUCH)
        } else {
            Ok(())
        }
    }
}

/// The runner window's state. Closing the window drops it, which stops the program.
#[derive(Debug, Default)]
pub struct Runner {
    pub name_draft: String,
    pub command_draft: String,
    /// The optional token typed for a web address (never shown, cleared once saved).
    pub token_draft: String,
    /// The server row the writer last clicked (its command is shown for approval).
    pub selected: Option<usize>,
    pub link: Link,
    /// Bumped whenever the connection changes, so an answer to an older request is ignored.
    pub generation: u64,
    pub tools: Vec<ToolInfo>,
    pub tool: Option<usize>,
    pub fields: Vec<Field>,
    pub running: bool,
    pub result: Option<CallResult>,
    /// A problem to show, already in the writer's words.
    pub notice: Option<String>,
    /// The recipes found in the macros folder (read when the window opens or on Reload).
    pub macros: Vec<crate::macros::RecipeFile>,
    pub macro_run: Option<MacroRun>,
    /// How the last run ended, in the writer's words.
    pub macro_status: Option<String>,
    pub macro_serial: u64,
}

impl Runner {
    /// Forget the tools and the form (the connection ended or changed).
    pub fn clear_tools(&mut self) {
        self.tools.clear();
        self.tool = None;
        self.fields.clear();
        self.running = false;
        self.result = None;
    }

    /// Choose a tool and build its form.
    pub fn pick_tool(&mut self, index: usize) {
        let Some(tool) = self.tools.get(index) else {
            return;
        };
        self.fields = fields_from_schema(&tool.input_schema);
        self.tool = Some(index);
        self.result = None;
        self.notice = None;
    }

    pub fn connection(&self) -> Option<&Arc<Connection>> {
        match &self.link {
            Link::Connected { connection, .. } => Some(connection),
            _ => None,
        }
    }
}

/// Drop a connection without making the window wait: stopping the program can take a moment.
pub fn stop_in_background(connection: Arc<Connection>) {
    // Only the last holder's drop does the work; other holders (a call in flight) finish first.
    let _ = std::thread::Builder::new()
        .name("libertum-mcp-stop".into())
        .spawn(move || drop(connection));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_web_address_is_recognised_as_one() {
        let spec = parse_command_line("https://example.com/mcp").expect("parse");
        assert!(spec.is_http());
        assert!(spec.args.is_empty());
        assert!(
            !parse_command_line("npx -y server")
                .expect("parse")
                .is_http()
        );
    }

    #[test]
    fn command_lines_split_at_spaces_and_honour_quotes() {
        let spec = parse_command_line(r#"npx -y "my server" plain"#).expect("parse");
        assert_eq!(spec.command, "npx");
        assert_eq!(spec.args, ["-y", "my server", "plain"]);

        let spec =
            parse_command_line(r#""C:\Program Files\x\server.exe" --port 1"#).expect("parse");
        assert_eq!(spec.command, r"C:\Program Files\x\server.exe");
        assert_eq!(spec.args, ["--port", "1"]);

        // An empty quoted argument is kept; extra spaces are not arguments.
        let spec = parse_command_line(r#"tool   "" x"#).expect("parse");
        assert_eq!(spec.args, ["", "x"]);

        // A quote inside a word joins the pieces, as a shell would.
        let spec = parse_command_line(r#"tool a"b c"d"#).expect("parse");
        assert_eq!(spec.args, ["ab cd"]);
    }

    #[test]
    fn nothing_is_expanded_and_bad_lines_are_refused() {
        let spec = parse_command_line("tool $HOME %PATH% * ; rm").expect("parse");
        assert_eq!(spec.args, ["$HOME", "%PATH%", "*", ";", "rm"]);
        assert_eq!(parse_command_line(""), Err(CommandLineError::Empty));
        assert_eq!(parse_command_line("   "), Err(CommandLineError::Empty));
        assert_eq!(parse_command_line(r#""""#), Err(CommandLineError::Empty));
        assert_eq!(
            parse_command_line(r#"tool "open"#),
            Err(CommandLineError::UnclosedQuote)
        );
        assert_eq!(
            parse_command_line("tool\nother"),
            Err(CommandLineError::ControlCharacter)
        );
    }

    #[test]
    fn a_parsed_line_displays_back_to_the_same_command() {
        let line = r#"npx -y "my server""#;
        let spec = parse_command_line(line).expect("parse");
        assert_eq!(spec.display(), line);
        assert_eq!(parse_command_line(&spec.display()), Ok(spec));
    }

    fn sample_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "zeta": {"type": "string", "description": "last"},
                "limit": {"type": "integer"},
                "ratio": {"type": ["number", "null"]},
                "loud": {"type": "boolean", "default": true},
                "filter": {"type": "object"},
                "query": {"type": "string", "description": "What to look for"},
                "anything": {}
            },
            "required": ["query", "limit"]
        })
    }

    #[test]
    fn the_form_puts_required_fields_first_and_reads_types() {
        let fields = fields_from_schema(&sample_schema());
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "limit", "query", "anything", "filter", "loud", "ratio", "zeta"
            ]
        );
        let kind = |n: &str| fields.iter().find(|f| f.name == n).map(|f| f.kind);
        assert_eq!(kind("limit"), Some(FieldKind::Integer));
        assert_eq!(kind("ratio"), Some(FieldKind::Number));
        assert_eq!(kind("loud"), Some(FieldKind::Boolean));
        assert_eq!(kind("filter"), Some(FieldKind::Json));
        assert_eq!(kind("anything"), Some(FieldKind::Json));
        assert_eq!(kind("query"), Some(FieldKind::Text));
        let query = &fields[1];
        assert!(query.required && query.description == "What to look for");
        assert!(
            fields
                .iter()
                .find(|f| f.name == "loud")
                .is_some_and(|f| f.checked)
        );
        assert!(fields_from_schema(&json!({"type": "object"})).is_empty());
        assert!(fields_from_schema(&json!("not a schema")).is_empty());
    }

    fn set(fields: &mut [Field], name: &str, value: &str) {
        if let Some(f) = fields.iter_mut().find(|f| f.name == name) {
            f.value = value.to_owned();
        }
    }

    #[test]
    fn arguments_are_built_from_the_form() {
        let mut fields = fields_from_schema(&sample_schema());
        assert_eq!(
            build_arguments(&fields),
            Err(ArgumentError::Missing("limit".into())),
            "a required field is empty"
        );
        set(&mut fields, "limit", " 5 ");
        set(&mut fields, "query", "  two  words ");
        set(&mut fields, "ratio", "0.5");
        set(&mut fields, "filter", r#"{"a": [1, 2]}"#);
        let args = build_arguments(&fields).expect("arguments");
        assert_eq!(args["limit"], json!(5));
        assert_eq!(
            args["query"],
            json!("  two  words "),
            "text is sent as typed"
        );
        assert_eq!(args["ratio"], json!(0.5));
        assert_eq!(args["filter"], json!({"a": [1, 2]}));
        assert_eq!(args["loud"], json!(true));
        assert!(
            !args.contains_key("zeta"),
            "an empty optional field is left out"
        );
        assert!(!args.contains_key("anything"));
    }

    #[test]
    fn bad_numbers_and_json_are_named() {
        let mut fields = fields_from_schema(&sample_schema());
        set(&mut fields, "query", "q");
        set(&mut fields, "limit", "2.5");
        assert_eq!(
            build_arguments(&fields),
            Err(ArgumentError::NotAnInteger("limit".into()))
        );
        set(&mut fields, "limit", "2");
        set(&mut fields, "ratio", "NaN");
        assert_eq!(
            build_arguments(&fields),
            Err(ArgumentError::NotANumber("ratio".into()))
        );
        set(&mut fields, "ratio", "");
        set(&mut fields, "filter", "{not json");
        assert_eq!(
            build_arguments(&fields),
            Err(ArgumentError::NotJson("filter".into()))
        );
    }

    #[test]
    fn an_unticked_optional_box_is_left_out_and_a_required_one_is_sent() {
        let schema = json!({
            "properties": {"a": {"type": "boolean"}, "b": {"type": "boolean"}},
            "required": ["b"]
        });
        let fields = fields_from_schema(&schema);
        let args = build_arguments(&fields).expect("arguments");
        assert!(!args.contains_key("a"));
        assert_eq!(args["b"], json!(false));
    }

    #[test]
    fn picking_a_tool_builds_its_form_and_clears_the_old_result() {
        let mut runner = Runner {
            tools: vec![ToolInfo {
                name: "search".into(),
                description: String::new(),
                input_schema: sample_schema(),
            }],
            result: Some(CallResult {
                text: "old".into(),
                is_error: false,
                truncated: false,
            }),
            ..Runner::default()
        };
        runner.pick_tool(5); // no such tool: nothing changes
        assert!(runner.tool.is_none() && runner.result.is_some());
        runner.pick_tool(0);
        assert_eq!(runner.tool, Some(0));
        assert_eq!(runner.fields.len(), 7);
        assert!(runner.result.is_none());
        runner.clear_tools();
        assert!(runner.tools.is_empty() && runner.fields.is_empty());
    }
}
