//! Recipe macros (developer mode, `docs/mcp-plan.md` phase 5): a recipe is a small TOML file
//! that reads the selection, calls tools of one MCP program and puts the answer into the
//! document. This module is the pure part: reading, checking and filling in recipes. Running
//! them is in `app/runner.rs`.
//!
//! ```toml
//! name = "Summarise selection"
//! server = "Notes"            # the saved program's name; it must be connected in the runner
//!
//! [[steps]]
//! kind = "get_selection"
//! save_as = "text"
//!
//! [[steps]]
//! kind = "call_tool"
//! tool = "summarise"
//! save_as = "summary"
//! [steps.args]
//! text = "{{text}}"
//!
//! [[steps]]
//! kind = "insert"            # after the selection; "replace_selection" swaps it instead
//! text = "{{summary}}"
//! ```
//!
//! A recipe cannot run a program, read a file or reach the network by itself: the only way out
//! is a tool of the connected MCP program, which the writer approved. Everything is checked
//! when the file is read, so a mistake shows up before anything runs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Map, Value};

/// Most steps one recipe may have.
pub const MAX_STEPS: usize = 50;
/// Most recipe files read from the folder.
pub const MAX_RECIPES: usize = 100;
/// Largest recipe file read, in bytes.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// Largest total size of the values a run keeps, in bytes.
pub const MAX_VARIABLE_BYTES: usize = 2_000_000;
const MAX_NAME_CHARS: usize = 60;
const MAX_VARIABLE_NAME_CHARS: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Remember the selected text under `save_as`.
    GetSelection { save_as: String },
    /// Call a tool of the recipe's program; its text answer is kept under `save_as` if given.
    CallTool {
        tool: String,
        args: Map<String, Value>,
        save_as: Option<String>,
    },
    /// Run a short Koto script over the values kept so far; its answer is kept under `save_as`.
    Script { code: String, save_as: String },
    /// Put `text` into the document just after the selection (or at the caret).
    Insert { text: String },
    /// Replace the selection with `text`, but only if it is still the text `get_selection` read.
    ReplaceSelection { text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub name: String,
    pub description: String,
    /// The saved program's name; `None` for a recipe that only edits the document.
    pub server: Option<String>,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecipeError {
    #[error("the file is not valid TOML: {0}")]
    Syntax(String),
    #[error("the recipe needs a name of 1 to {MAX_NAME_CHARS} characters")]
    Name,
    #[error("a recipe needs 1 to {MAX_STEPS} steps")]
    StepCount,
    #[error("step {0}: {1}")]
    Step(usize, String),
    #[error("the file is larger than {} KB", MAX_FILE_BYTES / 1024)]
    TooLarge,
    #[error("could not read the file: {0}")]
    Read(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRecipe {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    server: Option<String>,
    #[serde(default)]
    steps: Vec<RawStep>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RawStep {
    GetSelection {
        save_as: String,
    },
    CallTool {
        tool: String,
        #[serde(default)]
        args: Value,
        #[serde(default)]
        save_as: Option<String>,
    },
    Script {
        code: String,
        save_as: String,
    },
    Insert {
        text: String,
    },
    ReplaceSelection {
        text: String,
    },
}

fn valid_variable(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().count() <= MAX_VARIABLE_NAME_CHARS
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The variable names a template mentions, or why it is malformed. `{{name}}` is always a
/// substitution: there is no escape, and a `{{` without a matching `}}` is an error.
fn template_names(template: &str) -> Result<Vec<&str>, String> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            return Err("a {{ is never closed".to_string());
        };
        let name = after[..close].trim();
        if !valid_variable(name) {
            return Err(format!("{{{{{name}}}}} is not a variable name"));
        }
        names.push(name);
        rest = &after[close + 2..];
    }
    Ok(names)
}

/// Fill the `{{name}}` places in `template` from `vars`.
pub fn render(template: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            return Err("a {{ is never closed".to_string());
        };
        let name = after[..close].trim();
        let value = vars
            .get(name)
            .ok_or_else(|| format!("{name} has no value yet"))?;
        out.push_str(value);
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// `render` over every string inside a JSON value (object keys are left as written).
pub fn render_value(value: &Value, vars: &HashMap<String, String>) -> Result<Value, String> {
    Ok(match value {
        Value::String(text) => Value::String(render(text, vars)?),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| render_value(item, vars))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| Ok((key.clone(), render_value(item, vars)?)))
                .collect::<Result<_, String>>()?,
        ),
        other => other.clone(),
    })
}

fn value_names<'a>(value: &'a Value, out: &mut Vec<&'a str>) -> Result<(), String> {
    match value {
        Value::String(text) => out.extend(template_names(text)?),
        Value::Array(items) => {
            for item in items {
                value_names(item, out)?;
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                value_names(item, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Read one recipe from TOML text and check it.
pub fn parse(text: &str) -> Result<Recipe, RecipeError> {
    let raw: RawRecipe = toml::from_str(text).map_err(|e| {
        RecipeError::Syntax(e.message().lines().next().unwrap_or_default().to_string())
    })?;
    let name = raw.name.trim().to_string();
    if name.is_empty()
        || name.chars().count() > MAX_NAME_CHARS
        || name.chars().any(char::is_control)
    {
        return Err(RecipeError::Name);
    }
    if raw.steps.is_empty() || raw.steps.len() > MAX_STEPS {
        return Err(RecipeError::StepCount);
    }
    let server = raw
        .server
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let mut defined: HashSet<String> = HashSet::new();
    let mut selection_read = false;
    let mut steps = Vec::new();
    for (index, raw_step) in raw.steps.into_iter().enumerate() {
        let number = index + 1;
        let fail = |why: String| RecipeError::Step(number, why);
        let check_uses = |names: Vec<&str>, defined: &HashSet<String>| -> Result<(), RecipeError> {
            for used in names {
                if !defined.contains(used) {
                    return Err(fail(format!(
                        "{used} is used before an earlier step saves it"
                    )));
                }
            }
            Ok(())
        };
        let check_save = |name: &str| -> Result<(), RecipeError> {
            if valid_variable(name) {
                Ok(())
            } else {
                Err(fail(format!(
                    "save_as {name:?} must be letters, digits and underscores, not starting with a digit"
                )))
            }
        };
        match raw_step {
            RawStep::GetSelection { save_as } => {
                check_save(&save_as)?;
                defined.insert(save_as.clone());
                selection_read = true;
                steps.push(Step::GetSelection { save_as });
            }
            RawStep::CallTool {
                tool,
                args,
                save_as,
            } => {
                if server.is_none() {
                    return Err(fail(
                        "call_tool needs the recipe to name a server".to_string(),
                    ));
                }
                if tool.trim().is_empty() {
                    return Err(fail("the tool name is empty".to_string()));
                }
                let args_map = match args {
                    Value::Null => Map::new(),
                    Value::Object(map) => map,
                    _ => return Err(fail("args must be a table".to_string())),
                };
                let mut used = Vec::new();
                let args_value = Value::Object(args_map.clone());
                value_names(&args_value, &mut used).map_err(&fail)?;
                check_uses(used, &defined)?;
                if let Some(name) = &save_as {
                    check_save(name)?;
                    defined.insert(name.clone());
                }
                steps.push(Step::CallTool {
                    tool,
                    args: args_map,
                    save_as,
                });
            }
            RawStep::Script { code, save_as } => {
                check_save(&save_as)?;
                crate::script::check(&code).map_err(&fail)?;
                defined.insert(save_as.clone());
                steps.push(Step::Script { code, save_as });
            }
            RawStep::Insert { text } => {
                let used = template_names(&text).map_err(&fail)?;
                check_uses(used, &defined)?;
                steps.push(Step::Insert { text });
            }
            RawStep::ReplaceSelection { text } => {
                if !selection_read {
                    return Err(fail(
                        "replace_selection needs an earlier get_selection, so it can tell the selection has not changed"
                            .to_string(),
                    ));
                }
                let used = template_names(&text).map_err(&fail)?;
                check_uses(used, &defined)?;
                steps.push(Step::ReplaceSelection { text });
            }
        }
    }
    Ok(Recipe {
        name,
        description: raw.description.trim().to_string(),
        server,
        steps,
    })
}

/// One file in the recipes folder: the recipe, or why it could not be used.
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeFile {
    pub file: String,
    pub recipe: Result<Recipe, RecipeError>,
}

/// Read every `.toml` file in `dir` (at most [`MAX_RECIPES`], in name order). A missing folder
/// is an empty list.
pub fn load_dir(dir: &Path) -> Vec<RecipeFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        })
        .collect();
    paths.sort();
    paths.truncate(MAX_RECIPES);
    paths
        .into_iter()
        .map(|path| {
            let file = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let recipe = read_file(&path);
            RecipeFile { file, recipe }
        })
        .collect()
}

fn read_file(path: &Path) -> Result<Recipe, RecipeError> {
    let size = std::fs::metadata(path)
        .map_err(|e| RecipeError::Read(e.to_string()))?
        .len();
    if size > MAX_FILE_BYTES {
        return Err(RecipeError::TooLarge);
    }
    let text = std::fs::read_to_string(path).map_err(|e| RecipeError::Read(e.to_string()))?;
    // Notepad on Windows can save a byte-order mark first; TOML does not allow one.
    parse(text.trim_start_matches('\u{feff}'))
}

/// Where recipes live: `macros` in the data folder, or `LIBERTUM_MACROS` when set.
pub fn macros_dir() -> Option<PathBuf> {
    if let Some(folder) = std::env::var_os("LIBERTUM_MACROS")
        && !folder.is_empty()
    {
        return Some(PathBuf::from(folder));
    }
    crate::recovery::data_dir().map(|dir| dir.join("macros"))
}

/// Text from a tool or a recipe made safe to show: control characters become spaces and the
/// result is cut at `limit` characters with an ellipsis.
pub fn one_line(text: &str, limit: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if flat.chars().count() > limit {
        let cut: String = flat.chars().take(limit).collect();
        format!("{cut}…")
    } else {
        flat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMMARISE: &str = r#"
name = "Summarise selection"
server = "Notes"

[[steps]]
kind = "get_selection"
save_as = "text"

[[steps]]
kind = "call_tool"
tool = "summarise"
save_as = "summary"
[steps.args]
text = "{{text}}"
options = { style = "short", notes = ["a {{ text }}", 3] }

[[steps]]
kind = "insert"
text = "Summary: {{summary}}"
"#;

    #[test]
    fn a_recipe_reads_and_keeps_its_structure() {
        let recipe = parse(SUMMARISE).expect("parse");
        assert_eq!(recipe.name, "Summarise selection");
        assert_eq!(recipe.server.as_deref(), Some("Notes"));
        assert_eq!(recipe.steps.len(), 3);
        let Step::CallTool {
            tool,
            args,
            save_as,
        } = &recipe.steps[1]
        else {
            panic!("second step is a call");
        };
        assert_eq!(tool, "summarise");
        assert_eq!(save_as.as_deref(), Some("summary"));
        assert_eq!(args["text"], "{{text}}");
        assert_eq!(args["options"]["notes"][1], 3);
    }

    #[test]
    fn templates_fill_in_recursively_and_only_from_known_values() {
        let recipe = parse(SUMMARISE).expect("parse");
        let Step::CallTool { args, .. } = &recipe.steps[1] else {
            panic!("call");
        };
        let vars = HashMap::from([("text".to_string(), "hello".to_string())]);
        let filled = render_value(&Value::Object(args.clone()), &vars).expect("render");
        assert_eq!(filled["text"], "hello");
        assert_eq!(filled["options"]["notes"][0], "a hello");
        assert_eq!(filled["options"]["notes"][1], 3);
        assert!(render("{{missing}}", &vars).is_err());
        // A value that itself contains braces is not filled in a second time.
        let vars = HashMap::from([("a".to_string(), "{{b}}".to_string())]);
        assert_eq!(render("[{{a}}]", &vars).expect("render"), "[{{b}}]");
    }

    #[test]
    fn mistakes_are_caught_when_the_file_is_read() {
        let bad = |text: &str| parse(text).expect_err("should be rejected");
        // A variable used before any step saves it.
        assert!(matches!(
            bad("name='x'\n[[steps]]\nkind='insert'\ntext='{{nope}}'"),
            RecipeError::Step(1, _)
        ));
        // A tool call with no server named.
        assert!(matches!(
            bad("name='x'\n[[steps]]\nkind='call_tool'\ntool='t'"),
            RecipeError::Step(1, _)
        ));
        // Replacing the selection without having read it.
        assert!(matches!(
            bad("name='x'\n[[steps]]\nkind='replace_selection'\ntext='a'"),
            RecipeError::Step(1, _)
        ));
        // An unknown step kind, an unknown field, a bad variable name, an unclosed brace.
        assert!(matches!(
            bad("name='x'\n[[steps]]\nkind='run_program'\ntext='a'"),
            RecipeError::Syntax(_)
        ));
        assert!(matches!(
            bad("name='x'\nshell='rm'\n[[steps]]\nkind='insert'\ntext='a'"),
            RecipeError::Syntax(_)
        ));
        assert!(matches!(
            bad("name='x'\n[[steps]]\nkind='get_selection'\nsave_as='1bad'"),
            RecipeError::Step(1, _)
        ));
        assert!(matches!(
            bad("name='x'\n[[steps]]\nkind='insert'\ntext='{{oops'"),
            RecipeError::Step(1, _)
        ));
        // A script that imports, is empty, or is not valid Koto; one with no save_as.
        assert!(matches!(
            bad("name='x'
[[steps]]
kind='script'
save_as='a'
code='import io'"),
            RecipeError::Step(1, _)
        ));
        assert!(matches!(
            bad("name='x'
[[steps]]
kind='script'
save_as='a'
code='x = ('"),
            RecipeError::Step(1, _)
        ));
        assert!(matches!(
            bad("name='x'
[[steps]]
kind='script'
code='1'"),
            RecipeError::Syntax(_)
        ));
        // No name, no steps, too many steps.
        assert_eq!(
            bad("name=' '\n[[steps]]\nkind='insert'\ntext='a'"),
            RecipeError::Name
        );
        assert_eq!(bad("name='x'"), RecipeError::StepCount);
        let many = "[[steps]]\nkind='insert'\ntext='a'\n".repeat(MAX_STEPS + 1);
        assert_eq!(bad(&format!("name='x'\n{many}")), RecipeError::StepCount);
        assert!(matches!(bad("name = "), RecipeError::Syntax(_)));
    }

    #[test]
    fn a_recipe_saved_with_a_byte_order_mark_still_loads() {
        let dir = std::env::temp_dir().join(format!("libertum-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let text =
            "\u{feff}name = \"Bom\"\nserver = \"S\"\n[[steps]]\nkind = \"insert\"\ntext = \"hi\"\n";
        std::fs::write(dir.join("bom.toml"), text).expect("write");
        let loaded = load_dir(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(loaded[0].recipe.is_ok(), "{:?}", loaded[0].recipe);
    }

    #[test]
    fn a_script_step_reads_and_saves_a_value() {
        let recipe = parse(
            "name='Trim'
[[steps]]
kind='get_selection'
save_as='t'
             [[steps]]
kind='script'
save_as='u'
code='t.trim().to_uppercase()'
             [[steps]]
kind='insert'
text='{{u}}'",
        )
        .expect("parse");
        assert!(matches!(recipe.steps[1], Step::Script { .. }));
        assert_eq!(recipe.server, None);
    }

    #[test]
    fn a_recipe_with_only_document_steps_needs_no_server() {
        let recipe = parse(
            "name='Shout'\n[[steps]]\nkind='get_selection'\nsave_as='t'\n[[steps]]\nkind='replace_selection'\ntext='{{t}}!'",
        )
        .expect("parse");
        assert_eq!(recipe.server, None);
    }

    #[test]
    fn the_folder_lists_good_and_bad_files_in_order() {
        let dir = std::env::temp_dir().join(format!("libertum-macros-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("b-good.toml"), SUMMARISE).expect("write");
        std::fs::write(dir.join("a-bad.toml"), "name = ").expect("write");
        std::fs::write(dir.join("notes.txt"), "ignored").expect("write");
        std::fs::write(
            dir.join("c-huge.toml"),
            "x".repeat(MAX_FILE_BYTES as usize + 1),
        )
        .expect("write");
        let files = load_dir(&dir);
        let names: Vec<&str> = files.iter().map(|f| f.file.as_str()).collect();
        assert_eq!(names, ["a-bad.toml", "b-good.toml", "c-huge.toml"]);
        assert!(matches!(files[0].recipe, Err(RecipeError::Syntax(_))));
        assert!(files[1].recipe.is_ok());
        assert_eq!(files[2].recipe, Err(RecipeError::TooLarge));
        assert!(load_dir(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
