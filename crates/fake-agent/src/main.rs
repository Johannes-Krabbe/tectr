//! A scripted ACP agent for tests. It behaves like the Claude adapter where the
//! daemon depends on it, including the same mode IDs, and the prompt says what
//! to do:
//!
//! - `echo a b c`: streams each word as a separate chunk.
//! - `burst N`: streams N chunks as fast as possible.
//! - `title a b c`: reports `a b c` as the session's title.
//! - `commands a b:hint`: reports the slash commands `a` and `b`, where `b` takes input
//!   described by `hint`. With nothing after it, reports that there are none.
//! - `write`: runs a tool that creates `fake-output.txt` in the session's
//!   directory. In the `default` mode it asks for permission first.
//! - `abandon`: asks for permission from background work, then ends the turn
//!   without waiting for the answer.
//! - `hang`: does nothing until the turn is cancelled.
//! - `crash`: exits immediately with an error.
//! - `whoami`: reports the session's mode, model, directory, whether it was
//!   resumed and which session, and what it was branched from.
//! - `subagent`: says `main`, then has a subagent say something, marked the way
//!   Claude's adapter marks it.
//! - `mcp`: reports the MCP servers the session was opened with, and its `_meta`.
//! - `context`: reports `$TECTR_SESSION`, `$TECTR_TOPIC`, the first folder on its PATH and
//!   what the session's system prompt added, each on a line of its own.
//! - `mcp-call NAME`: sends an MCP initialize request to the HTTP server NAME and
//!   reports the status and body it got back.
//! - `images`: reports each image in the prompt as the text before it, its type and its data.
//! - `files`: reports each file sent with the prompt as its URI and its text.
//! - `ask`: asks a form question like Claude's AskUserQuestion, then reports the answer.
//! - `login`: asks to have a page visited, then reports the answer.
//! - `plan`: reports a todo list, then the same list with its first entry done.
//! - `plan-doc`: reports an approved plan in Markdown.
//! - `exit-plan`: moves itself from plan mode to `auto`, as Claude does once a plan is approved.
//! - `usage`: reports how full its context is.
//! - `notice`: reports a warning notice.
//! - `compact`: compacts its context, with a summary in two pieces. `compact slow` takes half
//!   a second, and a message steered in meanwhile aborts it, as it does Claude's.
//! - `suggest a b c`: suggests `a b c` as the next prompt.
//! - `team`: starts a subagent, which is given a task, says something, uses a tool and finishes.
//! - `task`: starts a background command, as Claude Code reports one, and ends the turn while
//!   it runs. `task-done` ends it.
//! - `later`: ends the turn, then works on a turn of its own, as Claude Code does after a
//!   background task finishes.
//! - `wait`: waits until a message is steered into the turn, then repeats it and ends.
//!
//! Everything it says in a turn is one message, whose ID is `msg:` and the prompt with
//! dashes for spaces. A
//! branch is made from a session at the message `_meta.jetbrains.air.fork.messageId`
//! names, as Claude's adapter does, unless started with `--no-fork`. Branching from the
//! answer to `unforkable` fails.
//!
//! It takes images in prompts unless started with `--no-images`, and messages into a running
//! turn unless started with `--no-steer`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate,
    CancelNotification, CompactionStatus, CompactionSummaryChunk, CompactionUpdate, ContentBlock,
    ContentChunk, CurrentModeUpdate, EmbeddedResourceResource, ForkSessionRequest,
    ForkSessionResponse, IdleStateUpdate, Implementation, InitializeRequest, InitializeResponse,
    McpCapabilities, McpServer, NewSessionRequest, NewSessionResponse, Notice, NoticeSeverity,
    PermissionOption, PermissionOptionKind, Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus,
    PlanUpdate, PlanUpdateContent, PromptCapabilities, PromptRequest, PromptResponse,
    RequestPermissionOutcome, RequestPermissionRequest, ResumeSessionRequest,
    ResumeSessionResponse, RunningStateUpdate, SessionCapabilities, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionForkCapabilities, SessionId,
    SessionInfoUpdate, SessionMessage, SessionMode, SessionModeState, SessionNotification,
    SessionResumeCapabilities, SessionSystemPromptCapabilities, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StateUpdate, StopReason, SubagentUpdate, SystemPrompt, TextContent,
    ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
    UnstructuredCommandInput, UsageUpdate,
};
use agent_client_protocol::{
    Agent, Client, Error, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, Stdio,
    on_receive_notification, on_receive_request,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

const MODES: [&str; 5] = [
    "default",
    "acceptEdits",
    "auto",
    "bypassPermissions",
    "plan",
];

/// Claude Code's own messages, which Claude's adapter passes on when asked.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcNotification)]
#[notification(method = "_claude/sdkMessage")]
#[serde(rename_all = "camelCase")]
struct ClaudeMessage {
    session_id: String,
    message: Value,
}

/// A message for the turn that's running, as Claude's adapter takes it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "_session/steering", response = SteerResponse)]
#[serde(rename_all = "camelCase")]
struct SteerRequest {
    session_id: String,
    prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcResponse)]
struct SteerResponse {
    outcome: String,
}

/// Agents ask for forms with this. It's sent as it would come, to test the daemon's reading
/// of it rather than ACP's types.
#[derive(Debug, Clone, Serialize, Deserialize, JsonRpcRequest)]
#[request(method = "elicitation/create", response = Value)]
#[serde(transparent)]
struct Elicitation(Value);

#[derive(Default)]
struct State {
    cwd: PathBuf,
    mode: String,
    model: Option<String>,
    effort: Option<String>,
    resumed: bool,
    /// The agent's own session it was asked to resume.
    resumed_id: Option<String>,
    /// The session and message it was branched from, as `session@message`.
    forked: Option<String>,
    mcp: Vec<McpServer>,
    meta: Option<serde_json::Map<String, serde_json::Value>>,
    /// What the client added to the system prompt.
    system_prompt: Option<String>,
    ultracode: String,
    /// Where a running `wait` turn hears what's steered into it.
    steer: Option<mpsc::UnboundedSender<String>>,
    /// Whether the client said it can show subagents and forms.
    subagents: bool,
}

fn prompt_text(prompt: Option<SystemPrompt>) -> Option<String> {
    let blocks = prompt?.content.into_iter().filter_map(|block| match block {
        ContentBlock::Text(text) => Some(text.text),
        _ => None,
    });
    Some(blocks.collect::<Vec<_>>().join("\n\n"))
}

/// Like Claude's list: a default entry naming the real model, then the models.
fn model_values() -> Vec<SessionConfigSelectOption> {
    let mut values = vec![
        SessionConfigSelectOption::new("default", "Default (recommended)").description("SMART"),
    ];
    values.extend(select_values(&["cheap", "smart"]));
    values
}

fn select_values(values: &[&str]) -> Vec<SessionConfigSelectOption> {
    values
        .iter()
        .map(|value| SessionConfigSelectOption::new(value.to_string(), value.to_uppercase()))
        .collect()
}

/// Reported the way real agents do. As with Claude's Haiku 4.5, the cheap model has
/// no effort levels, and selecting it changes the mode.
fn config_options(state: &State) -> Vec<SessionConfigOption> {
    let model = state.model.as_deref().unwrap_or("smart");
    let mut options = vec![
        SessionConfigOption::select("mode", "Mode", state.mode.clone(), select_values(&MODES))
            .category(SessionConfigOptionCategory::Mode),
        SessionConfigOption::select("model", "Model", model.to_owned(), model_values())
            .category(SessionConfigOptionCategory::Model),
    ];
    options.push(
        SessionConfigOption::select(
            "ultracode",
            "Ultracode",
            state.ultracode.clone(),
            select_values(&["on", "off"]),
        )
        .description("Plan and verify bigger changes"),
    );
    if model != "cheap" {
        options.push(
            SessionConfigOption::select(
                "effort",
                "Effort",
                state.effort.clone().unwrap_or_else(|| "low".to_owned()),
                select_values(&["default", "low", "high"]),
            )
            .category(SessionConfigOptionCategory::ThoughtLevel),
        );
    }
    options
}

fn mode_state(current: &str) -> SessionModeState {
    SessionModeState::new(
        current.to_owned(),
        MODES
            .iter()
            .map(|id| SessionMode::new(id.to_string(), id.to_string()))
            .collect::<Vec<_>>(),
    )
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let state = Arc::new(Mutex::new(State {
        mode: "auto".to_owned(),
        ultracode: "off".to_owned(),
        ..State::default()
    }));
    let cancel = Arc::new(Notify::new());
    let images = !std::env::args().any(|arg| arg == "--no-images");
    let fork = !std::env::args().any(|arg| arg == "--no-fork");
    let steering = !std::env::args().any(|arg| arg == "--no-steer");

    let on_new = state.clone();
    let on_resume = state.clone();
    let on_fork = state.clone();
    let on_mode = state.clone();
    let on_config = state.clone();
    let on_prompt = state.clone();
    let on_steer = state.clone();
    let on_init = state.clone();
    let cancel_prompt = cancel.clone();

    Agent
        .v1()
        .name("fake-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                on_init.lock().unwrap().subagents = request.client_capabilities.subagents.is_some()
                    && request.client_capabilities.elicitation.is_some();
                let mut meta = serde_json::Map::new();
                meta.insert("steering".to_owned(), json!({ "supported": steering }));
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .meta(meta)
                        .agent_capabilities(
                            AgentCapabilities::new()
                                .session_capabilities(
                                    SessionCapabilities::new()
                                        .resume(SessionResumeCapabilities::new())
                                        .system_prompt(SessionSystemPromptCapabilities::new())
                                        .fork(fork.then(SessionForkCapabilities::new)),
                                )
                                .mcp_capabilities(McpCapabilities::new().http(true))
                                .prompt_capabilities(
                                    PromptCapabilities::new()
                                        .image(images)
                                        .embedded_context(true),
                                ),
                        )
                        .agent_info(Implementation::new("fake-agent", "0.0.0")),
                )
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _connection| {
                let mut state = on_new.lock().unwrap();
                state.cwd = request.cwd;
                state.mcp = request.mcp_servers;
                state.meta = request.meta;
                state.system_prompt = prompt_text(request.system_prompt);
                responder.respond(
                    NewSessionResponse::new("fake-session")
                        .modes(mode_state(&state.mode))
                        .config_options(config_options(&state)),
                )
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ResumeSessionRequest, responder, _connection| {
                let mut state = on_resume.lock().unwrap();
                state.cwd = request.cwd;
                state.mcp = request.mcp_servers;
                state.meta = request.meta;
                state.system_prompt = prompt_text(request.system_prompt);
                state.resumed = true;
                state.resumed_id = Some(request.session_id.to_string());
                responder.respond(
                    ResumeSessionResponse::new()
                        .modes(mode_state(&state.mode))
                        .config_options(config_options(&state)),
                )
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: ForkSessionRequest, responder, _connection| {
                let mut state = on_fork.lock().unwrap();
                let at = request
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("jetbrains")?.pointer("/air/fork/messageId"))
                    .and_then(|id| id.as_str())
                    .unwrap_or("latest");
                if at == "msg:unforkable" {
                    return responder.respond_with_error(Error::invalid_params().data(at));
                }
                state.forked = Some(format!("{}@{at}", request.session_id));
                state.cwd = request.cwd;
                state.mcp = request.mcp_servers;
                state.meta = request.meta;
                state.system_prompt = prompt_text(request.system_prompt);
                responder.respond(
                    ForkSessionResponse::new("fake-fork")
                        .modes(mode_state(&state.mode))
                        .config_options(config_options(&state)),
                )
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionModeRequest, responder, _connection| {
                on_mode.lock().unwrap().mode = request.mode_id.to_string();
                responder.respond(SetSessionModeResponse::new())
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                let value = serde_json::to_value(&request.value).unwrap_or_default();
                let value = value
                    .get("value")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned);
                let mut state = on_config.lock().unwrap();
                match request.config_id.to_string().as_str() {
                    "effort" => state.effort = value,
                    "ultracode" => state.ultracode = value.unwrap_or_default(),
                    "mode" => state.mode = value.unwrap_or_default(),
                    _ => {
                        if value.as_deref() == Some("cheap") {
                            state.mode = "default".to_owned();
                        }
                        state.model = value;
                    }
                }
                responder.respond(SetSessionConfigOptionResponse::new(config_options(&state)))
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: PromptRequest, responder, connection: ConnectionTo<Client>| {
                // Run the turn outside the dispatch loop, because it has to
                // keep receiving messages, such as a permission answer.
                let state = on_prompt.clone();
                let cancel = cancel_prompt.clone();
                let turn_connection = connection.clone();
                connection.spawn(async move {
                    let stop_reason = run_turn(state, cancel, &turn_connection, request).await?;
                    responder.respond(PromptResponse::new(stop_reason))
                })
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SteerRequest, responder, _connection| {
                let text = match request.prompt.first() {
                    Some(ContentBlock::Text(text)) => text.text.clone(),
                    _ => String::new(),
                };
                let steer = on_steer.lock().unwrap().steer.clone();
                let outcome = match steer.map(|steer| steer.send(text)) {
                    Some(Ok(())) => "injected",
                    _ => "promptRequired",
                };
                responder.respond(SteerResponse {
                    outcome: outcome.to_owned(),
                })
            },
            on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, connection: ConnectionTo<Client>| {
                // Stopping a subagent leaves the agent's own turn running.
                if notification.session_id.to_string() == CHILD {
                    connection.send_notification(SessionNotification::new(
                        SessionId::new("fake-session"),
                        SessionUpdate::SubagentUpdate(
                            SubagentUpdate::new(CHILD)
                                .state(StateUpdate::Idle(IdleStateUpdate::new())),
                        ),
                    ))?;
                    return Ok(());
                }
                cancel.notify_waiters();
                Ok(())
            },
            on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}

async fn run_turn(
    state: Arc<Mutex<State>>,
    cancel: Arc<Notify>,
    connection: &ConnectionTo<Client>,
    request: PromptRequest,
) -> Result<StopReason, Error> {
    let session_id = request.session_id;
    // The script is the first block. Any after it are images and the names they go by.
    let text = match request.prompt.first() {
        Some(ContentBlock::Text(text)) => text.text.clone(),
        _ => String::new(),
    };
    // A note from the daemon comes first. Repeating it lets tests see it arrived. Scripts
    // never have blank lines in them, so everything before the last one is the note.
    let (note, text) = match text.rsplit_once("\n\n") {
        Some((note, script)) if note.starts_with("[tectr]") => {
            (Some(note.to_owned()), script.to_owned())
        }
        _ => (None, text),
    };
    let speaker = Speaker {
        connection,
        session_id: &session_id,
        message_id: format!("msg:{}", text.replace(' ', "-")),
    };
    if let Some(note) = note {
        speaker.say(&note)?;
    }
    let mut words = text.split_whitespace();

    match words.next().unwrap_or_default() {
        "images" => {
            let mut label = String::new();
            for block in request.prompt.iter().skip(1) {
                match block {
                    ContentBlock::Text(text) => label.clone_from(&text.text),
                    ContentBlock::Image(image) => {
                        speaker.say(&format!("{label} {} {}", image.mime_type, image.data))?
                    }
                    _ => {}
                }
            }
        }
        "files" => {
            for block in request.prompt.iter().skip(1) {
                if let ContentBlock::Resource(resource) = block
                    && let EmbeddedResourceResource::TextResourceContents(contents) =
                        &resource.resource
                {
                    speaker.say(&format!("{} {}", contents.uri, contents.text))?;
                }
            }
        }
        "ask" | "login" => {
            let request = if text.starts_with("ask") {
                json!({
                    "mode": "form",
                    "sessionId": session_id.to_string(),
                    "message": "Please answer the following questions.",
                    "requestedSchema": {
                        "type": "object",
                        "properties": {
                            "question_0": {
                                "type": "string", "title": "Cache", "description": "Which cache?",
                                "oneOf": [
                                    { "const": "Redis", "title": "Redis", "description": "In memory" },
                                    { "const": "None", "title": "None" }
                                ]
                            },
                            "question_0_custom": { "type": "string", "title": "Other" },
                            "question_1": {
                                "type": "array", "title": "Parts",
                                "items": { "anyOf": [
                                    { "const": "api", "title": "API" },
                                    { "const": "ui", "title": "UI" }
                                ] }
                            }
                        }
                    }
                })
            } else {
                json!({
                    "mode": "url",
                    "sessionId": session_id.to_string(),
                    "message": "Log in to the service",
                    "elicitationId": "login-1",
                    "url": "https://example.com/login"
                })
            };
            let response = connection
                .send_request(Elicitation(request))
                .block_task()
                .await?;
            speaker.say(&format!("answer={response}"))?;
        }
        "plan" => {
            let entry =
                |content: &str, status| PlanEntry::new(content, PlanEntryPriority::Medium, status);
            for first in [PlanEntryStatus::InProgress, PlanEntryStatus::Completed] {
                connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::Plan(Plan::new(vec![
                        entry("Read the code", first),
                        entry("Write the fix", PlanEntryStatus::Pending),
                    ])),
                ))?;
            }
        }
        "plan-doc" => connection.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::PlanUpdate(PlanUpdate::new(PlanUpdateContent::markdown(
                "plan-mode",
                "# Plan\n\n1. Fix it",
            ))),
        ))?,
        "exit-plan" => {
            state.lock().unwrap().mode = "auto".to_owned();
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new("auto")),
            ))?;
        }
        "usage" => connection.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::UsageUpdate(UsageUpdate::new(1200, 200_000)),
        ))?,
        "notice" => connection.send_notification(SessionNotification::new(
            session_id.clone(),
            SessionUpdate::Notice(
                Notice::new(NoticeSeverity::Warning, "Retrying").description("The API is busy"),
            ),
        ))?,
        "compact" => {
            let update = |update| SessionNotification::new(session_id.clone(), update);
            connection.send_notification(update(SessionUpdate::CompactionUpdate(
                CompactionUpdate::new("c1", CompactionStatus::InProgress),
            )))?;
            if words.next() == Some("slow") {
                let (sender, mut steered) = mpsc::unbounded_channel();
                state.lock().unwrap().steer = Some(sender);
                let aborted =
                    tokio::time::timeout(std::time::Duration::from_millis(500), steered.recv())
                        .await
                        .is_ok();
                state.lock().unwrap().steer = None;
                if aborted {
                    connection.send_notification(update(SessionUpdate::CompactionUpdate(
                        CompactionUpdate::new("c1", CompactionStatus::Failed)
                            .error("Request was aborted"),
                    )))?;
                    return Ok(StopReason::EndTurn);
                }
            }
            for piece in ["We fixed ", "the bug."] {
                connection.send_notification(update(SessionUpdate::CompactionSummaryChunk(
                    CompactionSummaryChunk::new("c1", ContentBlock::Text(TextContent::new(piece))),
                )))?;
            }
            connection.send_notification(update(SessionUpdate::CompactionUpdate(
                CompactionUpdate::new("c1", CompactionStatus::Completed),
            )))?;
        }
        "suggest" => {
            let text = words.collect::<Vec<_>>().join(" ");
            let mut meta = serde_json::Map::new();
            meta.insert(
                "promptSuggestions".to_owned(),
                json!({ "suggestions": [{ "text": text }] }),
            );
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().meta(meta)),
            ))?;
        }
        "team" => {
            if !state.lock().unwrap().subagents {
                speaker.say("no subagents")?;
                return Ok(StopReason::EndTurn);
            }
            let child = SessionId::new(CHILD);
            let on = |session: &SessionId, update| {
                connection.send_notification(SessionNotification::new(session.clone(), update))
            };
            on(
                &session_id,
                SessionUpdate::SubagentUpdate(
                    SubagentUpdate::new(CHILD)
                        .title("Explore")
                        .description("Find the bug")
                        .state(StateUpdate::Running(RunningStateUpdate::new())),
                ),
            )?;
            on(
                &child,
                SessionUpdate::SessionMessage(
                    SessionMessage::new("toolu_task")
                        .sender_session_id(session_id.clone())
                        .recipient_session_id(child.clone())
                        .content(vec![ContentBlock::Text(TextContent::new(
                            "Look for the bug",
                        ))]),
                ),
            )?;
            on(
                &child,
                SessionUpdate::AgentMessageChunk(
                    ContentChunk::new(ContentBlock::Text(TextContent::new("found it")))
                        .message_id("msg:child"),
                ),
            )?;
            on(
                &child,
                SessionUpdate::ToolCall(
                    ToolCall::new("child-tool", "Read main.rs")
                        .kind(ToolKind::Read)
                        .status(ToolCallStatus::Completed),
                ),
            )?;
            on(
                &session_id,
                SessionUpdate::SubagentUpdate(
                    SubagentUpdate::new(CHILD).state(StateUpdate::Idle(IdleStateUpdate::new())),
                ),
            )?;
            speaker.say("main")?;
        }
        "task" | "task-done" => {
            let raw = |message: Value| {
                connection.send_notification(ClaudeMessage {
                    session_id: session_id.to_string(),
                    message,
                })
            };
            let output = state.lock().unwrap().cwd.join("tasks").join("b1.output");
            if text == "task" {
                std::fs::create_dir_all(output.parent().unwrap())
                    .map_err(Error::into_internal_error)?;
                std::fs::write(&output, "server started\n").map_err(Error::into_internal_error)?;
                connection.send_notification(SessionNotification::new(
                    session_id.clone(),
                    SessionUpdate::ToolCall(
                        ToolCall::new("toolu_bg", "npm run dev")
                            .kind(ToolKind::Execute)
                            .status(ToolCallStatus::Completed)
                            .raw_input(json!({ "command": "npm run dev", "run_in_background": true }))
                            .content(vec![ToolCallContent::from(format!(
                                "Command running in background with ID: b1. Output is being written to: {}",
                                output.display()
                            ))]),
                    ),
                ))?;
                raw(json!({
                    "type": "system", "subtype": "task_started", "task_id": "b1",
                    "tool_use_id": "toolu_bg", "description": "Start the dev server",
                    "task_type": "local_bash"
                }))?;
                raw(json!({
                    "type": "system", "subtype": "background_tasks_changed",
                    "tasks": [{ "task_id": "b1", "task_type": "local_bash", "description": "Start the dev server" }]
                }))?;
            } else {
                raw(json!({
                    "type": "system", "subtype": "task_notification", "task_id": "b1",
                    "status": "completed", "summary": "Exited", "output_file": output.display().to_string()
                }))?;
            }
        }
        // Like Claude Code when a background subagent reports back as it answers: it goes on
        // running past the end of the turn it was asked for, writes more, and only then says
        // it's idle.
        "carry-on" => {
            let state = |state: &str| ClaudeMessage {
                session_id: session_id.to_string(),
                message: json!({ "type": "system", "subtype": "session_state_changed", "state": state }),
            };
            connection.send_notification(state("running"))?;
            speaker.say("asked")?;
            let background = connection.clone();
            let session = session_id.clone();
            connection.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                background.send_notification(SessionNotification::new(
                    session.clone(),
                    SessionUpdate::AgentMessageChunk(
                        ContentChunk::new(ContentBlock::Text(TextContent::new("carried on")))
                            .message_id("msg:carried"),
                    ),
                ))?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                background.send_notification(ClaudeMessage {
                    session_id: session.to_string(),
                    message: json!({ "type": "system", "subtype": "session_state_changed", "state": "idle" }),
                })?;
                Ok(())
            })?;
        }
        "later" => {
            let background = connection.clone();
            let session = session_id.clone();
            connection.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                let state = |state: &str| ClaudeMessage {
                    session_id: session.to_string(),
                    message: json!({ "type": "system", "subtype": "session_state_changed", "state": state }),
                };
                background.send_notification(state("running"))?;
                background.send_notification(SessionNotification::new(
                    session.clone(),
                    SessionUpdate::AgentMessageChunk(
                        ContentChunk::new(ContentBlock::Text(TextContent::new("on my own")))
                            .message_id("msg:later"),
                    ),
                ))?;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                background.send_notification(state("idle"))?;
                Ok(())
            })?;
        }
        "wait" => {
            let (sender, mut steered) = mpsc::unbounded_channel();
            state.lock().unwrap().steer = Some(sender);
            let heard = steered.recv().await;
            state.lock().unwrap().steer = None;
            speaker.say(&format!("steered: {}", heard.unwrap_or_default()))?;
        }
        "echo" => {
            for word in words {
                speaker.say(word)?;
            }
        }
        "burst" => {
            let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            for n in 0..count {
                speaker.say(&n.to_string())?;
            }
        }
        "commands" => {
            let commands = words
                .map(|word| {
                    let (name, hint) = match word.split_once(':') {
                        Some((name, hint)) => (name, Some(hint)),
                        None => (word, None),
                    };
                    let command = AvailableCommand::new(name, format!("Runs {name}"));
                    match hint {
                        Some(hint) => command.input(AvailableCommandInput::Unstructured(
                            UnstructuredCommandInput::new(hint),
                        )),
                        None => command,
                    }
                })
                .collect();
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(commands)),
            ))?;
        }
        "title" => {
            let title = words.collect::<Vec<_>>().join(" ");
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)),
            ))?;
        }
        "write" => return write_file(&state, &speaker).await,
        "subagent" => {
            speaker.say("main")?;
            let mut chunk =
                ContentChunk::new(ContentBlock::Text(TextContent::new("from a subagent")))
                    .message_id("msg:sub");
            chunk.meta = serde_json::json!({ "claudeCode": { "parentToolUseId": "tool-1" } })
                .as_object()
                .cloned();
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::AgentMessageChunk(chunk),
            ))?;
        }
        "abandon" => {
            let background = connection.clone();
            let request = RequestPermissionRequest::new(
                session_id.clone(),
                ToolCallUpdate::new(
                    "fake-tool-2",
                    ToolCallUpdateFields::new().title("Background work"),
                ),
                vec![PermissionOption::new(
                    "allow",
                    "Yes",
                    PermissionOptionKind::AllowOnce,
                )],
            );
            connection.spawn(async move {
                let _ = background.send_request(request).block_task().await;
                Ok(())
            })?;
            // Give the request a moment to go out ahead of the turn's end.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        "hang" => {
            cancel.notified().await;
            return Ok(StopReason::Cancelled);
        }
        "crash" => std::process::exit(3),
        "whoami" => {
            let report = {
                let state = state.lock().unwrap();
                format!(
                    "pid={} mode={} model={} effort={} ultracode={} resumed={} session={} forked={} cwd={}",
                    std::process::id(),
                    state.mode,
                    state.model.as_deref().unwrap_or("none"),
                    state.effort.as_deref().unwrap_or("none"),
                    state.ultracode,
                    state.resumed,
                    state.resumed_id.as_deref().unwrap_or("none"),
                    state.forked.as_deref().unwrap_or("none"),
                    state.cwd.display()
                )
            };
            speaker.say(&report)?;
        }
        "context" => {
            let var = |name| std::env::var(name).unwrap_or_else(|_| "none".to_owned());
            let first_folder = std::env::var_os("PATH")
                .and_then(|path| std::env::split_paths(&path).next())
                .map_or_else(|| "none".to_owned(), |folder| folder.display().to_string());
            let prompt = state.lock().unwrap().system_prompt.clone();
            speaker.say(&format!(
                "session={}\ntopic={}\npath={first_folder}\nprompt={}",
                var("TECTR_SESSION"),
                var("TECTR_TOPIC"),
                prompt.as_deref().unwrap_or("none"),
            ))?;
        }
        "mcp" => {
            let report = {
                let state = state.lock().unwrap();
                let servers: Vec<String> = state
                    .mcp
                    .iter()
                    .map(|server| match server {
                        McpServer::Stdio(stdio) => {
                            let env: Vec<String> = stdio
                                .env
                                .iter()
                                .map(|var| format!("{}={}", var.name, var.value))
                                .collect();
                            format!(
                                "{}:stdio:{}:{}:{}",
                                stdio.name,
                                stdio.command.display(),
                                stdio.args.join(","),
                                env.join(",")
                            )
                        }
                        McpServer::Http(http) => {
                            let headers: Vec<String> = http
                                .headers
                                .iter()
                                .map(|header| format!("{}={}", header.name, header.value))
                                .collect();
                            format!("{}:http:{}:{}", http.name, http.url, headers.join(","))
                        }
                        _ => "other".to_owned(),
                    })
                    .collect();
                let meta = state
                    .meta
                    .clone()
                    .map_or_else(String::new, |m| serde_json::Value::Object(m).to_string());
                format!("mcp={} meta={meta}", servers.join(" "))
            };
            speaker.say(&report)?;
        }
        "mcp-call" => {
            let name = words.next().unwrap_or_default();
            let url = {
                let state = state.lock().unwrap();
                state.mcp.iter().find_map(|server| match server {
                    McpServer::Http(http) if http.name == name => Some(http.url.clone()),
                    _ => None,
                })
            };
            let report = match url {
                Some(url) => call_mcp(&url).await,
                None => format!("no HTTP server called {name}"),
            };
            speaker.say(&report)?;
        }
        // All of it, so tests can see what the daemon said.
        _ => speaker.say(&format!("unknown script: {text}"))?,
    }
    Ok(StopReason::EndTurn)
}

async fn write_file(state: &Mutex<State>, speaker: &Speaker<'_>) -> Result<StopReason, Error> {
    let connection = speaker.connection;
    let session_id = speaker.session_id;
    let (cwd, mode) = {
        let state = state.lock().unwrap();
        (state.cwd.clone(), state.mode.clone())
    };
    let path = cwd.join("fake-output.txt");
    let tool_id = "fake-tool-1";

    connection.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::ToolCall(
            ToolCall::new(tool_id, "Write fake-output.txt")
                .kind(ToolKind::Edit)
                .status(ToolCallStatus::Pending),
        ),
    ))?;

    if mode == "default" {
        let response = connection
            .send_request(RequestPermissionRequest::new(
                session_id.clone(),
                ToolCallUpdate::new(
                    tool_id,
                    ToolCallUpdateFields::new().title("Write fake-output.txt"),
                ),
                vec![
                    PermissionOption::new("allow", "Yes", PermissionOptionKind::AllowOnce),
                    PermissionOption::new("deny", "No", PermissionOptionKind::RejectOnce),
                ],
            ))
            .block_task()
            .await?;
        match response.outcome {
            RequestPermissionOutcome::Selected(selected)
                if selected.option_id.to_string() == "allow" => {}
            RequestPermissionOutcome::Selected(_) => {
                finish_tool(connection, session_id, tool_id, ToolCallStatus::Failed)?;
                speaker.say("denied")?;
                return Ok(StopReason::EndTurn);
            }
            _ => {
                finish_tool(connection, session_id, tool_id, ToolCallStatus::Failed)?;
                return Ok(StopReason::Cancelled);
            }
        }
    }

    std::fs::write(&path, "written by the fake agent\n").map_err(Error::into_internal_error)?;
    finish_tool(connection, session_id, tool_id, ToolCallStatus::Completed)?;
    speaker.say("wrote the file")?;
    Ok(StopReason::EndTurn)
}

fn finish_tool(
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
    tool_id: &'static str,
    status: ToolCallStatus,
) -> Result<(), Error> {
    connection.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            tool_id,
            ToolCallUpdateFields::new().status(status),
        )),
    ))
}

async fn call_mcp(url: &str) -> String {
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}});
    let sent = reqwest::Client::new()
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await;
    match sent {
        Ok(response) => {
            let status = response.status().as_u16();
            let session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("none")
                .to_owned();
            let text = response.text().await.unwrap_or_default();
            format!("status={status} session={session} body={text}")
        }
        Err(error) => format!("failed: {error}"),
    }
}

/// The subagent `team` starts.
const CHILD: &str = "fake-child";

/// Says things as one message of a turn.
struct Speaker<'a> {
    connection: &'a ConnectionTo<Client>,
    session_id: &'a SessionId,
    message_id: String,
}

impl Speaker<'_> {
    fn say(&self, text: &str) -> Result<(), Error> {
        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(text)))
            .message_id(self.message_id.as_str());
        self.connection.send_notification(SessionNotification::new(
            self.session_id.clone(),
            SessionUpdate::AgentMessageChunk(chunk),
        ))
    }
}
