//! The only place that knows about ACP. Each session gets one task and one
//! agent process, so a crash takes down a single session. This module turns
//! ACP messages into tectr events and tectr commands into ACP messages.

use std::path::PathBuf;
use std::sync::Arc;

use agent_client_protocol::schema::{MaybeUndefined, ProtocolVersion};
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    CancelNotification, ContentBlock, InitializeRequest, InitializeResponse, NewSessionRequest,
    PermissionOption, PermissionOptionKind, PromptRequest, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, ResumeSessionRequest,
    SelectedPermissionOutcome, SessionConfigOptionValue, SessionModeState, SessionNotification,
    SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest, StopReason, TextContent,
    ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use anyhow::{Result, anyhow, bail};
use protocol as tp;
use tokio::sync::mpsc;

use crate::agents::{AgentLauncher, acp_mode_id};
use crate::core::{Core, SessionCommand};

pub struct AgentSpawn {
    pub session_id: tp::SessionId,
    pub cwd: PathBuf,
    pub launcher: AgentLauncher,
    pub mode: tp::PermissionMode,
    pub model: Option<String>,
    /// The agent's own session ID, if this session has run before.
    pub resume: Option<String>,
}

pub async fn run(
    core: Arc<Core>,
    spawn: AgentSpawn,
    commands: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let session_id = spawn.session_id.clone();
    let result = drive(&core, spawn, commands).await;
    core.agent_exited(&session_id, result);
}

async fn drive(
    core: &Arc<Core>,
    spawn: AgentSpawn,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
) -> Result<()> {
    let (program, args) = core.resolve_launcher(&spawn.launcher).await?;
    let agent = AcpAgent::new(AcpAgentConfig::new(program).args(args));

    let id = spawn.session_id.clone();
    let on_update_core = core.clone();
    let on_update_id = id.clone();
    let on_permission_core = core.clone();
    let on_permission_id = id.clone();

    // These callbacks run in the connection's dispatch loop, which keeps events
    // in order. They must not wait for anything, so a permission request stores
    // its responder and returns straight away.
    Client
        .v1()
        .on_receive_notification(
            async move |notification: SessionNotification, _connection| {
                handle_update(&on_update_core, &on_update_id, notification.update);
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            async move |request: RequestPermissionRequest, responder, _connection| {
                on_permission_core.permission_requested(
                    &on_permission_id,
                    patch_from_update(request.tool_call.fields),
                    request.options.iter().map(option_from_acp).collect(),
                    Box::new(move |answer| {
                        let outcome = match answer {
                            Some(option_id) => RequestPermissionOutcome::Selected(
                                SelectedPermissionOutcome::new(option_id),
                            ),
                            None => RequestPermissionOutcome::Cancelled,
                        };
                        // This only fails if the agent has already gone away.
                        let _ = responder.respond(RequestPermissionResponse::new(outcome));
                    }),
                );
                Ok(())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, async |connection: ConnectionTo<Agent>| {
            let init = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let agent_session = open_session(core, &spawn, &connection, &init)
                .await
                .map_err(|error| acp_error(&format!("{error:#}")))?;

            core.agent_ready(&id, ready_event(&init, &agent_session.modes));

            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Some(SessionCommand::Prompt(text)) => {
                            let core = core.clone();
                            let id = id.clone();
                            connection
                                .send_request(PromptRequest::new(
                                    agent_session.id.clone(),
                                    vec![ContentBlock::Text(TextContent::new(text))],
                                ))
                                .on_receiving_result(async move |result| {
                                    core.turn_ended(
                                        &id,
                                        result
                                            .map(|response| stop_reason_from_acp(response.stop_reason))
                                            .map_err(|error| error.to_string()),
                                    );
                                    Ok(())
                                })?;
                        }
                        Some(SessionCommand::Cancel) => {
                            connection.send_notification(CancelNotification::new(
                                agent_session.id.clone(),
                            ))?;
                        }
                        Some(SessionCommand::Shutdown) | None => return Ok(()),
                    },
                    () = connection.incoming_closed() => {
                        return Err(acp_error("the agent process exited"));
                    }
                }
            }
        })
        .await
        .map_err(|error| anyhow!("{error}"))
}

fn acp_error(message: &str) -> agent_client_protocol::Error {
    agent_client_protocol::Error::internal_error().data(message)
}

struct OpenedSession {
    id: agent_client_protocol::v1::schema::SessionId,
    modes: Option<SessionModeState>,
}

/// Creates or resumes the agent's session, then applies this session's mode and
/// model. A mode the agent can't honor is a failure: running with different
/// permissions than the user chose would be worse than not running at all.
async fn open_session(
    core: &Core,
    spawn: &AgentSpawn,
    connection: &ConnectionTo<Agent>,
    init: &InitializeResponse,
) -> Result<OpenedSession> {
    let (id, modes) = match &spawn.resume {
        Some(agent_id) => {
            if init
                .agent_capabilities
                .session_capabilities
                .resume
                .is_none()
            {
                bail!("this agent can't resume a session after its process has stopped");
            }
            let response = connection
                .send_request(ResumeSessionRequest::new(
                    agent_id.clone(),
                    spawn.cwd.clone(),
                ))
                .block_task()
                .await
                .map_err(|error| anyhow!("failed to resume the agent's session: {error}"))?;
            (agent_id.clone().into(), response.modes)
        }
        None => {
            let response = connection
                .send_request(NewSessionRequest::new(spawn.cwd.clone()))
                .block_task()
                .await
                .map_err(|error| anyhow!("failed to start the agent's session: {error}"))?;
            core.agent_session_created(&spawn.session_id, &response.session_id.to_string());
            (response.session_id, response.modes)
        }
    };

    let wanted = acp_mode_id(spawn.mode).ok_or_else(|| anyhow!("unknown permission mode"))?;
    let Some(mut modes) = modes else {
        bail!("this agent doesn't report permission modes, so {wanted} can't be applied");
    };
    if !modes
        .available_modes
        .iter()
        .any(|mode| mode.id.to_string() == wanted)
    {
        bail!("this agent doesn't offer the permission mode {wanted}");
    }
    if modes.current_mode_id.to_string() != wanted {
        connection
            .send_request(SetSessionModeRequest::new(id.clone(), wanted))
            .block_task()
            .await
            .map_err(|error| anyhow!("failed to set the permission mode to {wanted}: {error}"))?;
        modes.current_mode_id = wanted.into();
    }

    if let Some(model) = &spawn.model {
        connection
            .send_request(SetSessionConfigOptionRequest::new(
                id.clone(),
                "model",
                SessionConfigOptionValue::value_id(model.clone()),
            ))
            .block_task()
            .await
            .map_err(|error| anyhow!("failed to set the model to {model}: {error}"))?;
    }
    Ok(OpenedSession {
        id,
        modes: Some(modes),
    })
}

fn ready_event(
    init: &InitializeResponse,
    modes: &Option<SessionModeState>,
) -> tp::SessionEventKind {
    let info = init.agent_info.as_ref();
    tp::SessionEventKind::AgentReady {
        name: info.map_or_else(|| "unknown".to_owned(), |info| info.name.clone()),
        version: info.map(|info| info.version.clone()),
        modes: modes
            .iter()
            .flat_map(|modes| &modes.available_modes)
            .map(|mode| mode.id.to_string())
            .collect(),
        current_mode: modes
            .as_ref()
            .map(|modes| modes.current_mode_id.to_string()),
    }
}

fn handle_update(core: &Core, id: &tp::SessionId, update: SessionUpdate) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content {
                core.agent_event(id, tp::SessionEventKind::AgentMessage { text: text.text });
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content {
                core.agent_event(id, tp::SessionEventKind::AgentThought { text: text.text });
            }
        }
        SessionUpdate::ToolCall(call) => core.agent_event(
            id,
            tp::SessionEventKind::ToolCall {
                id: call.tool_call_id.to_string(),
                tool: patch_from_call(call),
            },
        ),
        SessionUpdate::ToolCallUpdate(update) => core.agent_event(
            id,
            tp::SessionEventKind::ToolCallUpdate {
                id: update.tool_call_id.to_string(),
                tool: patch_from_update(update.fields),
            },
        ),
        // A missing title means "unchanged", and only an explicit null clears it.
        SessionUpdate::SessionInfoUpdate(info) => match info.title {
            MaybeUndefined::Value(title) => core.title_changed(id, Some(title)),
            MaybeUndefined::Null => core.title_changed(id, None),
            MaybeUndefined::Undefined => {}
        },
        // Plans, usage, slash commands, and config changes aren't part of the
        // protocol yet. Storing them would add noise that no client can show.
        _ => {}
    }
}

fn patch_from_call(call: ToolCall) -> tp::ToolCallPatch {
    tp::ToolCallPatch {
        title: Some(call.title),
        kind: Some(kind_name(call.kind)),
        status: Some(status_from_acp(call.status)),
        input: call.raw_input,
        locations: Some(call.locations.iter().map(location_path).collect()),
        content: Some(call.content.into_iter().map(content_from_acp).collect()),
    }
}

fn patch_from_update(fields: ToolCallUpdateFields) -> tp::ToolCallPatch {
    tp::ToolCallPatch {
        title: fields.title,
        kind: fields.kind.map(kind_name),
        status: fields.status.map(status_from_acp),
        input: fields.raw_input,
        locations: fields
            .locations
            .map(|locations| locations.iter().map(location_path).collect()),
        content: fields
            .content
            .map(|content| content.into_iter().map(content_from_acp).collect()),
    }
}

fn kind_name(kind: ToolKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_owned())
}

fn location_path(location: &ToolCallLocation) -> String {
    location.path.display().to_string()
}

fn status_from_acp(status: ToolCallStatus) -> tp::ToolCallStatus {
    match status {
        ToolCallStatus::Pending => tp::ToolCallStatus::Pending,
        ToolCallStatus::InProgress => tp::ToolCallStatus::InProgress,
        ToolCallStatus::Completed => tp::ToolCallStatus::Completed,
        ToolCallStatus::Failed => tp::ToolCallStatus::Failed,
        _ => tp::ToolCallStatus::Unknown,
    }
}

fn content_from_acp(content: ToolCallContent) -> tp::ToolCallContent {
    match content {
        ToolCallContent::Content(content) => match content.content {
            ContentBlock::Text(text) => tp::ToolCallContent::Text { text: text.text },
            _ => tp::ToolCallContent::Unknown,
        },
        ToolCallContent::Diff(diff) => tp::ToolCallContent::Diff {
            path: diff.path.display().to_string(),
            old_text: diff.old_text,
            new_text: diff.new_text,
        },
        _ => tp::ToolCallContent::Unknown,
    }
}

fn option_from_acp(option: &PermissionOption) -> tp::PermissionOption {
    tp::PermissionOption {
        id: option.option_id.to_string(),
        name: option.name.clone(),
        kind: match option.kind {
            PermissionOptionKind::AllowOnce => tp::PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways => tp::PermissionOptionKind::AllowAlways,
            PermissionOptionKind::RejectOnce => tp::PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways => tp::PermissionOptionKind::RejectAlways,
            _ => tp::PermissionOptionKind::Unknown,
        },
    }
}

fn stop_reason_from_acp(reason: StopReason) -> tp::StopReason {
    match reason {
        StopReason::EndTurn => tp::StopReason::EndTurn,
        StopReason::MaxTokens => tp::StopReason::MaxTokens,
        StopReason::MaxTurnRequests => tp::StopReason::MaxTurnRequests,
        StopReason::Refusal => tp::StopReason::Refusal,
        StopReason::Cancelled => tp::StopReason::Cancelled,
        _ => tp::StopReason::Unknown,
    }
}
