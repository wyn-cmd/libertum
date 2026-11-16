//! Running recipe macros from the tool runner window (`crate::macros` reads and checks them).
//!
//! A run is a small state machine. Steps that only touch the document run straight away on the
//! app thread; a `call_tool` step hands the call to a worker and the run resumes when
//! `RunnerMsg::MacroCalled` arrives. Every edit a run makes is merged into one undo step when it
//! ends, however it ends. A run stops, rather than carry on, when the writer edits the document,
//! changes the selection it read, disconnects, or presses Stop.

use libertum_mcp::ToolOutput;

use iced::widget::{column, row, text};

use super::runner::RunnerMsg;
use super::*;
use crate::macros::{self, Step};
use crate::mcp_runner::{Link, MacroRun, Runner};
use crate::widgets::controls;

/// Keep a message from a tool or a recipe short and on one line before it is shown.
fn short(text: &str) -> String {
    macros::one_line(text, 200)
}

/// What one pass over the steps ended with.
enum Pass {
    /// Waiting for a tool; the task delivers its answer.
    Waiting(Task<Message>),
    Finished,
    Failed(String),
    /// The writer edited the document mid-run: stop, and keep their edit out of the run's
    /// undo step.
    ForeignEdit,
}

impl App {
    /// (Re)read the macros folder.
    pub(super) fn load_macros(&mut self) {
        let files = macros::macros_dir()
            .map(|dir| macros::load_dir(&dir))
            .unwrap_or_default();
        if let Some(runner) = self.runner.as_mut() {
            runner.macros = files;
        }
    }

    pub(super) fn start_macro(&mut self, index: usize) -> Task<Message> {
        let Some(runner) = self.runner.as_mut() else {
            return Task::none();
        };
        if runner.macro_run.is_some() {
            return Task::none();
        }
        let Some(Ok(recipe)) = runner.macros.get(index).map(|f| f.recipe.clone()) else {
            return Task::none();
        };
        // A recipe that calls tools must find its program connected, under the name it gave.
        if let Some(wanted) = &recipe.server {
            let connected = match &runner.link {
                Link::Connected { server, .. } => self
                    .settings
                    .mcp_servers
                    .get(*server)
                    .is_some_and(|saved| saved.name == *wanted),
                _ => false,
            };
            if !connected {
                runner.macro_status = Some(format!("{} {wanted}", s::MACROS_NEEDS_SERVER));
                return Task::none();
            }
        }
        let Some(doc) = self.doc.as_ref() else {
            runner.macro_status = Some(format!(
                "{} {}",
                s::MACROS_STOPPED,
                s::MACROS_ERR_NO_DOCUMENT
            ));
            return Task::none();
        };
        let undo_len = doc.editor().history().undo_len();
        runner.macro_serial += 1;
        runner.macro_status = None;
        runner.macro_run = Some(MacroRun {
            recipe,
            serial: runner.macro_serial,
            next: 0,
            vars: Default::default(),
            pending_save: None,
            selection: None,
            undo_before: undo_len,
            expected_undo: undo_len,
            edited: false,
        });
        self.advance_macro()
    }

    pub(super) fn macro_called(
        &mut self,
        serial: u64,
        result: Result<ToolOutput, String>,
    ) -> Task<Message> {
        let Some(run) = self
            .runner
            .as_mut()
            .and_then(|r| r.macro_run.as_mut())
            .filter(|run| run.serial == serial)
        else {
            // The run ended (stopped, disconnected) while the call was out.
            return Task::none();
        };
        let save = run.pending_save.take();
        let failure = match result {
            Err(error) => Some(short(&error)),
            Ok(output) if output.is_error => Some(short(&output.text)),
            Ok(output) if output.truncated => Some(s::MACROS_ERR_CUT.to_owned()),
            Ok(output) => {
                if let Some(name) = save {
                    run.keep(name, output.text).err().map(str::to_owned)
                } else {
                    None
                }
            }
        };
        if let Some(why) = failure {
            self.end_macro(Err(why), true);
            return Task::none();
        }
        self.advance_macro()
    }

    /// Stop the run, if there is one, keeping what it has already done as one undo step.
    pub(super) fn abort_macro(&mut self, why: &str) {
        if self
            .runner
            .as_ref()
            .is_some_and(|runner| runner.macro_run.is_some())
        {
            self.end_macro(Err(why.to_owned()), true);
        }
    }

    /// Run steps until one has to wait for a tool, or the recipe ends.
    fn advance_macro(&mut self) -> Task<Message> {
        let pass = self.macro_pass();
        let edited = self
            .runner
            .as_ref()
            .and_then(|r| r.macro_run.as_ref())
            .is_some_and(|run| run.edited);
        if edited {
            // The document changed: refresh layout, word count and the dirty mark.
            let _ = self.finish_edit(Ok(()));
        }
        match pass {
            Pass::Waiting(task) => task,
            Pass::Finished => {
                self.end_macro(Ok(()), true);
                Task::none()
            }
            Pass::Failed(why) => {
                self.end_macro(Err(why), true);
                Task::none()
            }
            Pass::ForeignEdit => {
                self.end_macro(Err(s::MACROS_ERR_DOCUMENT_CHANGED.to_owned()), false);
                Task::none()
            }
        }
    }

    fn macro_pass(&mut self) -> Pass {
        let (memory_mb, total_mb) = (
            self.settings.script_memory_mb,
            self.settings.script_total_memory_mb,
        );
        let (Some(runner), Some(doc)) = (self.runner.as_mut(), self.doc.as_mut()) else {
            return Pass::Failed(s::MACROS_ERR_NO_DOCUMENT.to_owned());
        };
        loop {
            let connection = runner.connection().cloned();
            let Some(run) = runner.macro_run.as_mut() else {
                return Pass::Finished;
            };
            let Some(step) = run.recipe.steps.get(run.next).cloned() else {
                return Pass::Finished;
            };
            if doc.editor().history().undo_len() != run.expected_undo {
                return Pass::ForeignEdit;
            }
            match step {
                Step::GetSelection { save_as } => {
                    let Some(text) = doc.editor().selected_text() else {
                        return Pass::Failed(s::MACROS_ERR_NO_SELECTION.to_owned());
                    };
                    run.vars.insert(save_as, text.clone());
                    run.selection = Some(text);
                }
                Step::CallTool {
                    tool,
                    args,
                    save_as,
                } => {
                    let Some(connection) = connection else {
                        return Pass::Failed(s::MACROS_ERR_DISCONNECTED.to_owned());
                    };
                    let arguments =
                        match macros::render_value(&serde_json::Value::Object(args), &run.vars) {
                            Ok(serde_json::Value::Object(map)) => map,
                            Ok(_) => {
                                return Pass::Failed("the arguments are not a table".to_owned());
                            }
                            Err(error) => return Pass::Failed(error),
                        };
                    run.pending_save = save_as;
                    run.next += 1;
                    let serial = run.serial;
                    return Pass::Waiting(Task::perform(
                        async move {
                            connection
                                .call(&tool, arguments, None)
                                .map_err(|e| e.to_string())
                        },
                        move |result| Message::Runner(RunnerMsg::MacroCalled(serial, result)),
                    ));
                }
                Step::Script { code, save_as } => {
                    // On the app thread, stopped by the script's own 2 second limit.
                    match crate::script::run(&code, &run.vars, memory_mb, total_mb) {
                        Ok(answer) => {
                            if let Err(why) = run.keep(save_as, answer) {
                                return Pass::Failed(why.to_owned());
                            }
                        }
                        Err(error) => return Pass::Failed(short(&error)),
                    }
                }
                Step::Insert { text } => {
                    let text = match macros::render(&text, &run.vars) {
                        Ok(text) => text,
                        Err(error) => return Pass::Failed(error),
                    };
                    // After the selection, or at the caret when nothing is selected.
                    let at = doc
                        .editor()
                        .ordered_selection()
                        .map(|(_, end)| end)
                        .unwrap_or_else(|| doc.editor().selection().head);
                    if let Err(error) = doc.edit(|ed| ed.replace_range(at, at, &text)) {
                        return Pass::Failed(error.to_string());
                    }
                    run.edited = true;
                    run.expected_undo = doc.editor().history().undo_len();
                }
                Step::ReplaceSelection { text } => {
                    let text = match macros::render(&text, &run.vars) {
                        Ok(text) => text,
                        Err(error) => return Pass::Failed(error),
                    };
                    // Only the text the recipe read may be replaced.
                    if doc.editor().selected_text() != run.selection {
                        return Pass::Failed(s::MACROS_ERR_SELECTION_CHANGED.to_owned());
                    }
                    let Some((start, end)) = doc.editor().ordered_selection() else {
                        return Pass::Failed(s::MACROS_ERR_NO_SELECTION.to_owned());
                    };
                    if let Err(error) = doc.edit(|ed| ed.replace_range(start, end, &text)) {
                        return Pass::Failed(error.to_string());
                    }
                    run.edited = true;
                    run.expected_undo = doc.editor().history().undo_len();
                    // The old selection is gone; a second replace needs a fresh read.
                    run.selection = None;
                }
            }
            run.next += 1;
        }
    }

    /// Finish the run: fold its edits into one undo step and say how it ended.
    fn end_macro(&mut self, outcome: Result<(), String>, merge: bool) {
        let Some(runner) = self.runner.as_mut() else {
            return;
        };
        let Some(run) = runner.macro_run.take() else {
            return;
        };
        if merge
            && run.edited
            && let Some(doc) = self.doc.as_mut()
        {
            let made = doc
                .editor()
                .history()
                .undo_len()
                .saturating_sub(run.undo_before);
            if made >= 2 {
                let _ = doc.edit(|ed| {
                    ed.merge_last_undo_steps(made);
                    Ok(Default::default())
                });
            }
        }
        let name = short(&run.recipe.name);
        runner.macro_status = Some(match outcome {
            Ok(()) => format!("{} {name}", s::MACROS_DONE),
            Err(why) => format!("{} {name}: {why}", s::MACROS_STOPPED),
        });
    }

    pub(super) fn macros_section<'a>(
        &'a self,
        runner: &'a Runner,
        t: Tokens,
        plain: &dyn Fn(String, Option<RunnerMsg>) -> Element<'a, Message>,
        heading: &dyn Fn(&'static str) -> Element<'a, Message>,
    ) -> Element<'a, Message> {
        let mut out = column![heading(s::MACROS_TITLE)].spacing(6);
        out = out.push(controls::caption(t, s::MACROS_HINT));
        let folder = macros::macros_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        out = out.push(
            text(folder)
                .size(11)
                .font(iced::Font::MONOSPACE)
                .color(t.text),
        );
        if runner.macros.is_empty() {
            out = out.push(controls::caption(t, s::MACROS_NONE));
        }
        let running = runner.macro_run.is_some();
        for (index, file) in runner.macros.iter().enumerate() {
            match &file.recipe {
                Ok(recipe) => {
                    let label = if running
                        && runner
                            .macro_run
                            .as_ref()
                            .is_some_and(|run| run.recipe.name == recipe.name)
                    {
                        format!("{} ({})", short(&recipe.name), s::MACROS_RUNNING)
                    } else {
                        short(&recipe.name)
                    };
                    let mut line = row![text(label).size(13).color(t.text)].spacing(8);
                    line = line.push(iced::widget::space::horizontal());
                    line = line.push(plain(
                        s::MACROS_RUN.to_owned(),
                        (!running).then_some(RunnerMsg::RunMacro(index)),
                    ));
                    out = out.push(line);
                    if !recipe.description.is_empty() {
                        out = out.push(controls::caption(t, short(&recipe.description)));
                    }
                }
                Err(error) => {
                    out = out.push(controls::caption(
                        t,
                        format!(
                            "{} {}: {}",
                            s::MACROS_BAD_FILE,
                            short(&file.file),
                            short(&error.to_string())
                        ),
                    ));
                }
            }
        }
        let mut buttons = row![plain(
            s::MACROS_RELOAD.to_owned(),
            Some(RunnerMsg::ReloadMacros)
        )]
        .spacing(6);
        if running {
            buttons = buttons.push(plain(s::MACROS_STOP.to_owned(), Some(RunnerMsg::StopMacro)));
        }
        out = out.push(buttons);
        if let Some(status) = &runner.macro_status {
            out = out.push(text(status.clone()).size(12).color(t.text));
        }
        out.into()
    }
}
