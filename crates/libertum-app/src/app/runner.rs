//! The MCP tool runner window (developer mode): messages, handlers and view. The pure parts are
//! in `crate::mcp_runner`. Reachable only while MCP is switched on in Settings, and closed (which
//! stops any program it started) when that goes off.

use std::sync::Arc;

use libertum_mcp::{ClientError, Connection, ToolInfo, ToolOutput};

use iced::widget::{button, column, container, mouse_area, row, space, stack, text};

use super::*;
use crate::mcp_runner::{
    self as run, ArgumentError, CallResult, CommandLineError, FieldKind, Link, Runner,
};
use crate::widgets::controls;

#[derive(Debug, Clone)]
pub enum RunnerMsg {
    Open,
    Close,
    Name(String),
    Command(String),
    Token(String),
    Add,
    Remove(usize),
    Select(usize),
    Connect(usize),
    /// Approve the selected program's command line as it stands, then connect.
    ApproveAndConnect(usize),
    /// The program started (or did not). The number says which connection attempt this is for.
    Connected(u64, Result<Arc<Connection>, String>),
    Disconnect,
    Tools(u64, Result<Vec<ToolInfo>, String>),
    PickTool(usize),
    Field(usize, String),
    Tick(usize, bool),
    Run,
    Done(u64, Result<ToolOutput, String>),
    Insert,
    Copy,
    ReloadMacros,
    RunMacro(usize),
    StopMacro,
    /// A tool answered a macro step. The number says which run it was for.
    MacroCalled(u64, Result<ToolOutput, String>),
}

fn command_error(error: CommandLineError) -> &'static str {
    match error {
        CommandLineError::Empty => s::RUNNER_ERR_COMMAND_EMPTY,
        CommandLineError::UnclosedQuote => s::RUNNER_ERR_COMMAND_QUOTE,
        CommandLineError::ControlCharacter => s::RUNNER_ERR_COMMAND_CONTROL,
    }
}

fn argument_error(error: &ArgumentError) -> String {
    let (label, name) = match error {
        ArgumentError::Missing(name) => (s::RUNNER_ERR_MISSING, name),
        ArgumentError::NotANumber(name) => (s::RUNNER_ERR_NUMBER, name),
        ArgumentError::NotAnInteger(name) => (s::RUNNER_ERR_INTEGER, name),
        ArgumentError::NotJson(name) => (s::RUNNER_ERR_JSON, name),
    };
    format!("{label} {name}")
}

/// A server's own words can be long or hold odd characters; keep what the window shows short.
fn shown(text: &str) -> String {
    crate::macros::one_line(text, 300)
}

impl App {
    pub(super) fn update_runner(&mut self, message: RunnerMsg) -> Task<Message> {
        // The window exists only while MCP is on; a late message after it went off is dropped.
        if !self.settings.mcp_active() {
            self.close_runner();
            return Task::none();
        }
        if matches!(message, RunnerMsg::Open) {
            if self.runner.is_none() {
                self.runner = Some(Runner::default());
                self.load_macros();
            }
            return Task::none();
        }
        let Some(runner) = self.runner.as_mut() else {
            return Task::none();
        };
        match message {
            RunnerMsg::Open => {}
            RunnerMsg::Close => self.close_runner(),
            RunnerMsg::Name(text) => runner.name_draft = text,
            RunnerMsg::Command(text) => runner.command_draft = text,
            RunnerMsg::Token(text) => runner.token_draft = text,
            RunnerMsg::Add => {
                if let Err(error) = run::parse_command_line(&runner.command_draft) {
                    runner.notice = Some(command_error(error).to_owned());
                    return Task::none();
                }
                let full = self.settings.mcp_servers.len() >= crate::settings::MAX_MCP_SERVERS;
                match self
                    .settings
                    .add_mcp_server(&runner.name_draft, &runner.command_draft)
                {
                    Some(index) => {
                        runner.name_draft.clear();
                        runner.command_draft.clear();
                        runner.selected = Some(index);
                        runner.notice = None;
                        if !runner.token_draft.trim().is_empty()
                            && !self
                                .settings
                                .set_mcp_server_token(index, &runner.token_draft)
                        {
                            runner.notice = Some(s::RUNNER_ERR_TOKEN.to_owned());
                        }
                        runner.token_draft.clear();
                        self.save_settings();
                    }
                    None => {
                        runner.notice = Some(
                            if full {
                                s::RUNNER_ERR_FULL
                            } else {
                                s::RUNNER_ERR_ADD
                            }
                            .to_owned(),
                        );
                    }
                }
            }
            RunnerMsg::Remove(index) => {
                // Indices shift, so end any connection rather than track which one moved.
                self.disconnect_runner();
                if let Some(runner) = self.runner.as_mut() {
                    runner.selected = None;
                    runner.notice = None;
                }
                self.settings.remove_mcp_server(index);
                self.save_settings();
            }
            RunnerMsg::Select(index) => {
                runner.selected = Some(index);
                runner.notice = None;
            }
            RunnerMsg::ApproveAndConnect(index) => {
                self.settings.approve_mcp_server(index);
                self.save_settings();
                return self.connect_runner(index);
            }
            RunnerMsg::Connect(index) => return self.connect_runner(index),
            RunnerMsg::Connected(generation, result) => {
                if generation != runner.generation {
                    // The window moved on (disconnected, or another program chosen).
                    if let Ok(connection) = result {
                        run::stop_in_background(connection);
                    }
                    return Task::none();
                }
                let Link::Connecting(server) = runner.link else {
                    return Task::none();
                };
                match result {
                    Ok(connection) => {
                        runner.link = Link::Connected {
                            server,
                            announced: shown(connection.server_name()),
                            connection: Arc::clone(&connection),
                        };
                        runner.notice = None;
                        return Task::perform(
                            async move { connection.tools().map_err(|e| e.to_string()) },
                            move |result| Message::Runner(RunnerMsg::Tools(generation, result)),
                        );
                    }
                    Err(error) => {
                        runner.link = Link::Idle;
                        runner.notice =
                            Some(format!("{} {}", s::RUNNER_ERR_CONNECT, shown(&error)));
                    }
                }
            }
            RunnerMsg::Disconnect => self.disconnect_runner(),
            RunnerMsg::ReloadMacros => self.load_macros(),
            RunnerMsg::RunMacro(index) => return self.start_macro(index),
            RunnerMsg::StopMacro => self.abort_macro(s::MACROS_ERR_STOPPED),
            RunnerMsg::MacroCalled(serial, result) => return self.macro_called(serial, result),
            RunnerMsg::Tools(generation, result) => {
                if generation != runner.generation {
                    return Task::none();
                }
                match result {
                    Ok(tools) => {
                        runner.tools = tools;
                        runner.tool = None;
                        runner.fields.clear();
                    }
                    Err(error) => {
                        runner.notice = Some(format!("{} {}", s::RUNNER_ERR_CALL, shown(&error)));
                    }
                }
            }
            RunnerMsg::PickTool(index) => runner.pick_tool(index),
            RunnerMsg::Field(index, value) => {
                if let Some(field) = runner.fields.get_mut(index) {
                    field.value = value;
                }
            }
            RunnerMsg::Tick(index, on) => {
                if let Some(field) = runner.fields.get_mut(index) {
                    field.checked = on;
                }
            }
            RunnerMsg::Run => {
                let (Some(connection), Some(tool)) = (runner.connection(), runner.tool) else {
                    return Task::none();
                };
                if runner.running {
                    return Task::none();
                }
                let Some(name) = runner.tools.get(tool).map(|t| t.name.clone()) else {
                    return Task::none();
                };
                let arguments = match run::build_arguments(&runner.fields) {
                    Ok(arguments) => arguments,
                    Err(error) => {
                        runner.notice = Some(argument_error(&error));
                        return Task::none();
                    }
                };
                let connection = Arc::clone(connection);
                let generation = runner.generation;
                runner.running = true;
                runner.result = None;
                runner.notice = None;
                return Task::perform(
                    async move {
                        connection
                            .call(&name, arguments, None)
                            .map_err(|e: ClientError| e.to_string())
                    },
                    move |result| Message::Runner(RunnerMsg::Done(generation, result)),
                );
            }
            RunnerMsg::Done(generation, result) => {
                if generation != runner.generation {
                    return Task::none();
                }
                runner.running = false;
                match result {
                    Ok(output) => runner.result = Some(CallResult::from(output)),
                    Err(error) => {
                        runner.notice = Some(format!("{} {}", s::RUNNER_ERR_CALL, shown(&error)));
                    }
                }
            }
            RunnerMsg::Copy => {
                if let Some(result) = &runner.result {
                    return iced::clipboard::write(result.text.clone());
                }
            }
            RunnerMsg::Insert => {
                let Some(text) = runner.result.as_ref().map(|r| r.text.clone()) else {
                    return Task::none();
                };
                if text.is_empty() {
                    return Task::none();
                }
                let Some(doc) = self.doc.as_mut() else {
                    return Task::none();
                };
                // One undo step that never joins the writer's typing.
                let result = doc.edit(|ed| ed.insert_text_separately(&text));
                return self.finish_edit(result);
            }
        }
        Task::none()
    }

    fn connect_runner(&mut self, index: usize) -> Task<Message> {
        let Some(server) = self.settings.mcp_servers.get(index) else {
            return Task::none();
        };
        // The one gate that matters: a program is started only from a command line the writer
        // approved, exactly as it stands now.
        if !server.is_approved() {
            return Task::none();
        }
        let spec = match run::parse_command_line(&server.command_line) {
            Ok(spec) => spec,
            Err(error) => {
                if let Some(runner) = self.runner.as_mut() {
                    runner.notice = Some(command_error(error).to_owned());
                }
                return Task::none();
            }
        };
        let spec = match &server.token {
            Some(token) if spec.is_http() => spec.with_bearer(token.clone()),
            _ => spec,
        };
        self.disconnect_runner();
        let Some(runner) = self.runner.as_mut() else {
            return Task::none();
        };
        runner.generation += 1;
        let generation = runner.generation;
        runner.link = Link::Connecting(index);
        runner.selected = Some(index);
        runner.notice = None;
        Task::perform(
            async move {
                Connection::start(&spec)
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            },
            move |result| Message::Runner(RunnerMsg::Connected(generation, result)),
        )
    }

    /// End the connection, if any, and forget its tools. A call still in flight is ignored.
    fn disconnect_runner(&mut self) {
        self.abort_macro(s::MACROS_ERR_DISCONNECTED);
        let Some(runner) = self.runner.as_mut() else {
            return;
        };
        runner.generation += 1;
        if let Link::Connected { connection, .. } = std::mem::take(&mut runner.link) {
            run::stop_in_background(connection);
        }
        runner.clear_tools();
    }

    /// Close the window, which stops the program it started.
    pub(super) fn close_runner(&mut self) {
        self.disconnect_runner();
        self.runner = None;
    }

    pub(super) fn runner_layer<'a>(
        &'a self,
        runner: &'a Runner,
        t: Tokens,
    ) -> Element<'a, Message> {
        let plain = |label: String, message: Option<RunnerMsg>| -> Element<'_, Message> {
            let mut b = button(text(label).size(12).wrapping(Wrapping::None))
                .padding([4, 10])
                .style(move |_, status| controls::flat(t, status));
            if let Some(message) = message {
                b = b.on_press(Message::Runner(message));
            }
            b.into()
        };
        let input = |placeholder: &'static str,
                     value: &str,
                     on_input: fn(String) -> RunnerMsg|
         -> Element<'_, Message> {
            iced::widget::text_input(placeholder, value)
                .on_input(move |v| Message::Runner(on_input(v)))
                .size(13)
                .padding(6)
                .width(Length::Fill)
                .style(move |_, _| input_style(t))
                .into()
        };
        let heading = |label: &'static str| -> Element<'_, Message> {
            text(label)
                .size(13)
                .font(controls::bold())
                .color(t.text)
                .into()
        };

        // The saved programs.
        let mut servers = column![heading(s::RUNNER_SERVERS)].spacing(6);
        if self.settings.mcp_servers.is_empty() {
            servers = servers.push(controls::caption(t, s::RUNNER_NO_SERVERS));
        }
        for (index, server) in self.settings.mcp_servers.iter().enumerate() {
            let selected = runner.selected == Some(index);
            let connected =
                matches!(&runner.link, Link::Connected { server, .. } if *server == index);
            let connecting = matches!(runner.link, Link::Connecting(i) if i == index);
            let name = button(text(shown(&server.name)).size(13).wrapping(Wrapping::None))
                .padding([5, 10])
                .width(Length::Fill)
                .style(move |_, _| button::Style {
                    background: Some(if selected { t.accent } else { t.control }.into()),
                    text_color: if selected { t.on_accent } else { t.text },
                    border: border::rounded(5),
                    ..button::Style::default()
                })
                .on_press(Message::Runner(RunnerMsg::Select(index)));
            let state: Element<'_, Message> = if connecting {
                controls::caption(t, s::RUNNER_CONNECTING)
            } else if connected {
                plain(s::RUNNER_DISCONNECT.to_owned(), Some(RunnerMsg::Disconnect))
            } else if server.is_approved() {
                plain(
                    s::RUNNER_CONNECT.to_owned(),
                    Some(RunnerMsg::Connect(index)),
                )
            } else {
                space().into()
            };
            servers = servers.push(
                row![
                    name,
                    container(state).width(Length::Fixed(96.0)),
                    plain(s::RUNNER_REMOVE.to_owned(), Some(RunnerMsg::Remove(index)))
                ]
                .spacing(6)
                .align_y(Center),
            );
        }

        // The selected program: its exact command, and the approval it needs before it runs.
        let mut body = column![
            text(s::RUNNER_TITLE)
                .size(15)
                .font(controls::bold())
                .color(t.text),
            controls::caption(t, s::RUNNER_HINT),
            servers,
        ]
        .spacing(12);
        if let Some(server) = runner
            .selected
            .and_then(|i| self.settings.mcp_servers.get(i).map(|server| (i, server)))
        {
            let (index, server) = server;
            let mut detail = column![].spacing(4);
            let is_web = crate::mcp_runner::parse_command_line(&server.command_line)
                .is_ok_and(|spec| spec.is_http());
            if !server.is_approved() {
                detail = detail.push(controls::caption(
                    t,
                    if is_web {
                        s::RUNNER_APPROVE_WEB_TITLE
                    } else {
                        s::RUNNER_APPROVE_TITLE
                    },
                ));
            }
            if is_web && server.token.is_some() {
                detail = detail.push(controls::caption(t, s::RUNNER_TOKEN_SAVED));
            }
            detail = detail.push(
                text(server.command_line.clone())
                    .size(12)
                    .font(iced::Font::MONOSPACE)
                    .color(t.text),
            );
            if !server.is_approved() {
                detail = detail
                    .push(controls::caption(
                        t,
                        if is_web {
                            s::RUNNER_APPROVE_WEB_DETAIL
                        } else {
                            s::RUNNER_APPROVE_DETAIL
                        },
                    ))
                    .push(self.action_button(
                        s::RUNNER_APPROVE_AND_CONNECT,
                        true,
                        Message::Runner(RunnerMsg::ApproveAndConnect(index)),
                    ));
            } else if let Link::Connected {
                announced,
                server: at,
                ..
            } = &runner.link
                && *at == index
                && !announced.is_empty()
            {
                detail = detail.push(controls::caption(
                    t,
                    format!("{}: {announced}", s::RUNNER_CONNECTED),
                ));
            }
            body = body.push(detail);
        }

        // Adding a program.
        body = body.push(
            column![
                input(
                    s::RUNNER_NAME_PLACEHOLDER,
                    &runner.name_draft,
                    RunnerMsg::Name
                ),
                row![
                    input(
                        s::RUNNER_COMMAND_PLACEHOLDER,
                        &runner.command_draft,
                        RunnerMsg::Command
                    ),
                    plain(s::RUNNER_ADD.to_owned(), Some(RunnerMsg::Add)),
                ]
                .spacing(6)
                .align_y(Center),
                iced::widget::text_input(s::RUNNER_TOKEN_PLACEHOLDER, &runner.token_draft)
                    .on_input(|v| Message::Runner(RunnerMsg::Token(v)))
                    .secure(true)
                    .size(13)
                    .padding(6)
                    .width(Length::Fill)
                    .style(move |_, _| input_style(t)),
            ]
            .spacing(6),
        );

        // Tools, the form for the chosen one, and its result.
        if runner.connection().is_some() {
            body = body.push(self.runner_tools(runner, t, &plain, &heading));
        }
        body = body.push(self.macros_section(runner, t, &plain, &heading));
        if let Some(notice) = &runner.notice {
            body = body.push(text(notice.clone()).size(12).color(t.text));
        }

        let height = (self.window_height - 140.0).max(240.0);
        let card = container(
            column![
                iced::widget::scrollable(body.width(Length::Fill)).height(Length::Fixed(height)),
                row![
                    space::horizontal(),
                    self.action_button(s::RUNNER_CLOSE, true, Message::Runner(RunnerMsg::Close))
                ],
            ]
            .spacing(12),
        )
        .padding(18)
        .width(Length::Fixed(560.0))
        .style(move |_| container::Style {
            background: Some(t.inspector.into()),
            border: border::rounded(12),
            shadow: iced::Shadow {
                color: t.shadow,
                offset: iced::Vector::new(0.0, 8.0),
                blur_radius: 24.0,
            },
            ..container::Style::default()
        });
        stack![
            // A click outside closes it, as for the other windows.
            mouse_area(space().width(Length::Fill).height(Length::Fill))
                .on_press(Message::Runner(RunnerMsg::Close)),
            container(card)
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill),
        ]
        .into()
    }

    fn runner_tools<'a>(
        &'a self,
        runner: &'a Runner,
        t: Tokens,
        plain: &dyn Fn(String, Option<RunnerMsg>) -> Element<'a, Message>,
        heading: &dyn Fn(&'static str) -> Element<'a, Message>,
    ) -> Element<'a, Message> {
        let mut out = column![heading(s::RUNNER_TOOLS)].spacing(6);
        if runner.tools.is_empty() {
            out = out.push(controls::caption(t, s::RUNNER_NO_TOOLS));
            return out.into();
        }
        let mut list = column![].spacing(3);
        for (index, tool) in runner.tools.iter().enumerate() {
            let selected = runner.tool == Some(index);
            list = list.push(
                button(text(shown(&tool.name)).size(12).wrapping(Wrapping::None))
                    .padding([4, 10])
                    .width(Length::Fill)
                    .style(move |_, _| button::Style {
                        background: Some(if selected { t.accent } else { t.control }.into()),
                        text_color: if selected { t.on_accent } else { t.text },
                        border: border::rounded(5),
                        ..button::Style::default()
                    })
                    .on_press(Message::Runner(RunnerMsg::PickTool(index))),
            );
        }
        out = out.push(iced::widget::scrollable(list).height(Length::Fixed(120.0)));

        let Some(tool) = runner.tool.and_then(|i| runner.tools.get(i)) else {
            return out.into();
        };
        if !tool.description.is_empty() {
            out = out.push(controls::caption(t, shown(&tool.description)));
        }
        if runner.fields.is_empty() {
            out = out.push(controls::caption(t, s::RUNNER_NO_ARGUMENTS));
        }
        for (index, field) in runner.fields.iter().enumerate() {
            let mut label = field.name.clone();
            if field.required {
                label.push_str(&format!(" ({})", s::RUNNER_REQUIRED));
            }
            if field.kind == FieldKind::Json {
                label.push_str(&format!(" ({})", s::RUNNER_JSON_HINT));
            }
            let mut block = column![controls::caption(t, label)].spacing(3);
            if !field.description.is_empty() {
                block = block.push(controls::caption(t, shown(&field.description)));
            }
            if field.kind == FieldKind::Boolean {
                block = block.push(controls::checkbox_row(
                    t,
                    "",
                    field.checked,
                    false,
                    move |on| Message::Runner(RunnerMsg::Tick(index, on)),
                ));
            } else {
                block = block.push(
                    iced::widget::text_input("", &field.value)
                        .on_input(move |v| Message::Runner(RunnerMsg::Field(index, v)))
                        .size(13)
                        .padding(6)
                        .width(Length::Fill)
                        .style(move |_, _| input_style(t)),
                );
            }
            out = out.push(block);
        }
        let run_label = if runner.running {
            s::RUNNER_RUNNING
        } else {
            s::RUNNER_RUN
        };
        out = out.push(self.action_button(run_label, true, Message::Runner(RunnerMsg::Run)));
        // A running call cannot be pressed again: the handler ignores it too.
        if let Some(result) = &runner.result {
            out = out.push(heading(s::RUNNER_RESULT));
            if result.is_error {
                out = out.push(text(s::RUNNER_RESULT_ERROR).size(12).color(t.text));
            }
            let shown_text: &str = if result.text.is_empty() {
                s::RUNNER_RESULT_EMPTY
            } else {
                &result.text
            };
            out = out.push(
                container(
                    iced::widget::scrollable(text(shown_text).size(12).color(t.text))
                        .height(Length::Fixed(160.0))
                        .width(Length::Fill),
                )
                .padding(8)
                .style(move |_| container::Style {
                    background: Some(t.control.into()),
                    border: border::rounded(6),
                    ..container::Style::default()
                }),
            );
            if result.truncated {
                out = out.push(controls::caption(t, s::RUNNER_RESULT_CUT));
            }
            let can_insert = self.doc.is_some() && !result.text.is_empty();
            out = out.push(
                row![
                    plain(
                        s::RUNNER_INSERT.to_owned(),
                        can_insert.then_some(RunnerMsg::Insert)
                    ),
                    plain(s::RUNNER_COPY.to_owned(), Some(RunnerMsg::Copy)),
                ]
                .spacing(6),
            );
        }
        out.into()
    }
}

fn input_style(t: Tokens) -> iced::widget::text_input::Style {
    iced::widget::text_input::Style {
        background: t.control.into(),
        border: iced::Border {
            color: t.control_border,
            width: 1.0,
            radius: 5.0.into(),
        },
        icon: t.text_secondary,
        placeholder: t.text_secondary,
        value: t.text,
        selection: t.accent,
    }
}
