//! A scripted ACP agent for tests. It behaves like the Claude adapter where the
//! daemon depends on it, including the same mode IDs, and the prompt says what
//! to do:
//!
//! - `echo a b c`: streams each word as a separate chunk.
//! - `burst N`: streams N chunks as fast as possible.
//! - `title a b c`: reports `a b c` as the session's title.
//! - `write`: runs a tool that creates `fake-output.txt` in the session's
//!   directory. In the `default` mode it asks for permission first.
//! - `abandon`: asks for permission from background work, then ends the turn
//!   without waiting for the answer.
//! - `hang`: does nothing until the turn is cancelled.
//! - `crash`: exits immediately with an error.
//! - `whoami`: reports the session's mode, model, directory, and whether it was
//!   resumed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AgentCapabilities, CancelNotification, ContentBlock, ContentChunk, Implementation,
    InitializeRequest, InitializeResponse, NewSessionRequest, NewSessionResponse, PermissionOption,
    PermissionOptionKind, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, ResumeSessionRequest, ResumeSessionResponse, SessionCapabilities,
    SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOption, SessionId,
    SessionInfoUpdate, SessionMode, SessionModeState, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse, StopReason,
    TextContent, ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{
    Agent, Client, Error, Stdio, on_receive_notification, on_receive_request,
};
use tokio::sync::Notify;

const MODES: [&str; 3] = ["default", "auto", "bypassPermissions"];

#[derive(Default)]
struct State {
    cwd: PathBuf,
    mode: String,
    model: Option<String>,
    effort: Option<String>,
    resumed: bool,
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
        ..State::default()
    }));
    let cancel = Arc::new(Notify::new());

    let on_new = state.clone();
    let on_resume = state.clone();
    let on_mode = state.clone();
    let on_config = state.clone();
    let on_prompt = state.clone();
    let cancel_prompt = cancel.clone();

    Agent
        .v1()
        .name("fake-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                responder.respond(
                    InitializeResponse::new(request.protocol_version)
                        .agent_capabilities(AgentCapabilities::new().session_capabilities(
                            SessionCapabilities::new().resume(SessionResumeCapabilities::new()),
                        ))
                        .agent_info(Implementation::new("fake-agent", "0.0.0")),
                )
            },
            on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _connection| {
                let mut state = on_new.lock().unwrap();
                state.cwd = request.cwd;
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
                state.resumed = true;
                responder.respond(
                    ResumeSessionResponse::new()
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
        .on_receive_notification(
            async move |_: CancelNotification, _connection| {
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
    let text: String = request
        .prompt
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect();
    let mut words = text.split_whitespace();

    match words.next().unwrap_or_default() {
        "echo" => {
            for word in words {
                say(connection, &session_id, word)?;
            }
        }
        "burst" => {
            let count: u32 = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
            for n in 0..count {
                say(connection, &session_id, &n.to_string())?;
            }
        }
        "title" => {
            let title = words.collect::<Vec<_>>().join(" ");
            connection.send_notification(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)),
            ))?;
        }
        "write" => return write_file(&state, connection, &session_id).await,
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
                    "pid={} mode={} model={} effort={} resumed={} cwd={}",
                    std::process::id(),
                    state.mode,
                    state.model.as_deref().unwrap_or("none"),
                    state.effort.as_deref().unwrap_or("none"),
                    state.resumed,
                    state.cwd.display()
                )
            };
            say(connection, &session_id, &report)?;
        }
        other => say(connection, &session_id, &format!("unknown script: {other}"))?,
    }
    Ok(StopReason::EndTurn)
}

async fn write_file(
    state: &Mutex<State>,
    connection: &ConnectionTo<Client>,
    session_id: &SessionId,
) -> Result<StopReason, Error> {
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
                say(connection, session_id, "denied")?;
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
    say(connection, session_id, "wrote the file")?;
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

fn say(connection: &ConnectionTo<Client>, session_id: &SessionId, text: &str) -> Result<(), Error> {
    connection.send_notification(SessionNotification::new(
        session_id.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}
