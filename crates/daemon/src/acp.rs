//! The only place that knows about ACP. Each session gets one task and one
//! agent process, so a crash takes down a single session. This module turns
//! ACP messages into tectr events and tectr commands into ACP messages.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::schema::{MaybeUndefined, ProtocolVersion};
use agent_client_protocol::v1::ConnectionTo;
use agent_client_protocol::v1::schema::{
    AvailableCommandInput, CancelNotification, ContentBlock, EnvVariable, ImageContent,
    InitializeRequest, InitializeResponse, McpServer, McpServerHttp, McpServerStdio, Meta,
    NewSessionRequest, PermissionOption, PermissionOptionKind, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    ResumeSessionRequest, SelectedPermissionOutcome, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelect,
    SessionConfigSelectOption, SessionConfigSelectOptions, SessionId, SessionModeState,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest,
    StopReason, TextContent, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus,
    ToolCallUpdateFields, ToolKind,
};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, Client};
use anyhow::{Result, anyhow, bail};
use protocol as tp;
use tokio::sync::mpsc;

use crate::agents::AgentLauncher;
use crate::core::{Core, Prompt, SessionCommand, SessionSettings};
use crate::mcp::{self, AgentMcp, Isolation};

/// An agent mid-turn can be slow to answer, but nothing else reaches it until it does, and
/// that includes cancelling. So one that takes this long is treated as having failed.
const SETTINGS_TIMEOUT: Duration = Duration::from_secs(15);

pub struct AgentSpawn {
    pub session_id: tp::SessionId,
    /// Tells this process apart from a later one for the same session.
    pub run: String,
    pub cwd: PathBuf,
    pub launcher: AgentLauncher,
    pub settings: SessionSettings,
    /// The agent's own session ID, if this session has run before.
    pub resume: Option<String>,
    /// The MCP servers it gets, and the only ones.
    pub mcp: Vec<AgentMcp>,
}

/// Starts the agent with its own MCP servers switched off.
async fn launch(core: &Core, launcher: &AgentLauncher, isolation: &Isolation) -> Result<AcpAgent> {
    let (program, mut args) = core.resolve_launcher(launcher).await?;
    args.extend(isolation.args.iter().cloned());
    let config = AcpAgentConfig::new(program)
        .args(args)
        .envs(isolation.env.iter().cloned());
    Ok(AcpAgent::new(config))
}

/// The servers as ACP passes them. An agent that can't use HTTP servers goes without them,
/// and the session says so rather than failing.
fn acp_servers(
    core: &Core,
    id: &tp::SessionId,
    servers: &[AgentMcp],
    init: &InitializeResponse,
) -> Vec<McpServer> {
    let http = init.agent_capabilities.mcp_capabilities.http;
    let mut left_out = vec![];
    let mut passed = vec![];
    for server in servers {
        match server {
            AgentMcp::Stdio {
                name,
                command,
                args,
                env,
            } => passed.push(McpServer::Stdio(
                McpServerStdio::new(name.clone(), command.clone())
                    .args(args.clone())
                    .env(
                        env.iter()
                            .map(|(key, value)| EnvVariable::new(key.clone(), value.clone()))
                            .collect(),
                    ),
            )),
            AgentMcp::Http { name, url } if http => {
                passed.push(McpServer::Http(McpServerHttp::new(
                    name.clone(),
                    url.clone(),
                )));
            }
            AgentMcp::Http { name, .. } => left_out.push(name.clone()),
        }
    }
    if !left_out.is_empty() {
        core.agent_event(
            id,
            tp::SessionEventKind::Error {
                message: format!(
                    "This agent can't use HTTP MCP servers, so it doesn't get {}",
                    left_out.join(", ")
                ),
            },
        );
    }
    passed
}

pub async fn run(
    core: Arc<Core>,
    spawn: AgentSpawn,
    commands: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let session_id = spawn.session_id.clone();
    let run = spawn.run.clone();
    let result = drive(&core, spawn, commands).await;
    core.agent_exited(&session_id, &run, result);
}

async fn drive(
    core: &Arc<Core>,
    spawn: AgentSpawn,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
) -> Result<()> {
    let keep: Vec<&str> = spawn.mcp.iter().map(AgentMcp::name).collect();
    let isolation = mcp::isolation(&spawn.launcher, &spawn.cwd, &keep, core.config_home());
    let agent = launch(core, &spawn.launcher, &isolation).await?;

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
            let servers = acp_servers(core, &id, &spawn.mcp, &init);
            let meta = isolation.meta.clone();
            let mut agent_session = open_session(core, &spawn, &connection, &init, servers, meta)
                .await
                .map_err(|error| acp_error(&format!("{error:#}")))?;

            core.agent_ready(&id, ready_event(&init, &agent_session));

            loop {
                tokio::select! {
                    command = commands.recv() => match command {
                        Some(SessionCommand::Prompt(prompt)) => {
                            if !prompt.images.is_empty()
                                && !init.agent_capabilities.prompt_capabilities.image
                            {
                                core.turn_ended(
                                    &id,
                                    Err("This agent doesn't take images. Send it again without them".to_owned()),
                                );
                                continue;
                            }
                            let core = core.clone();
                            let id = id.clone();
                            connection
                                .send_request(PromptRequest::new(
                                    agent_session.id.clone(),
                                    prompt_blocks(prompt),
                                ))
                                .on_receiving_result(async move |result| {
                                    core.turn_ended(
                                        &id,
                                        result
                                            .map(|response| stop_reason_from_acp(response.stop_reason))
                                            .map_err(|error| error.to_string()),
                                    );
                                    // The turn may have switched branches. Git is asked
                                    // elsewhere, because this runs in the dispatch loop.
                                    tokio::spawn(core.refresh_branches(Some(id)));
                                    Ok(())
                                })?;
                        }
                        Some(SessionCommand::Cancel) => {
                            connection.send_notification(CancelNotification::new(
                                agent_session.id.clone(),
                            ))?;
                        }
                        Some(SessionCommand::Configure { settings, reply }) => {
                            let applied = tokio::time::timeout(
                                SETTINGS_TIMEOUT,
                                agent_session.apply(&connection, &spawn.launcher, &settings),
                            )
                            .await;
                            let (result, unsure) = match applied {
                                Ok(Ok(())) => (Ok(()), false),
                                Ok(Err(error)) => (Err(format!("{error:#}")), agent_session.diverged),
                                Err(_) => (Err("the agent didn't answer in time".to_owned()), true),
                            };
                            // If only part of it took, the agent could be running with
                            // permissions nobody chose. Stopping it is safe, because the next
                            // prompt starts it again with the session's settings.
                            let stop = result.as_ref().err().filter(|_| unsure).map(|reason| {
                                acp_error(&format!(
                                    "the agent was stopped because changing its settings failed partway: {reason}"
                                ))
                            });
                            let _ = reply.send(result);
                            if let Some(error) = stop {
                                return Err(error);
                            }
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

/// The entry agents use for "whatever you'd pick". tectr always asks for something
/// specific, so that what's shown is what runs.
const DEFAULT_ENTRY: &str = "default";

struct OpenedSession {
    id: SessionId,
    /// What the agent reported when it opened. Agents don't all use both of these.
    reported_modes: Option<SessionModeState>,
    options: Vec<SessionConfigOption>,
    /// The mode IDs the agent offers.
    modes: Vec<String>,
    /// As of the last thing the agent said or was told. Neither report above follows a mode
    /// set through the other, so they can't be asked.
    current_mode: Option<String>,
    /// What was last applied in full, which is nothing until the session has opened.
    settings: Option<SessionSettings>,
    /// The last call to `apply` changed something in the agent before it failed.
    diverged: bool,
}

impl OpenedSession {
    /// Moves the agent to these settings, sending only what's needed. The model goes first,
    /// because changing it can change the others: Claude moves from auto to acceptEdits when
    /// switched to Haiku 4.5. A mode the agent can't honor is a failure, because running with
    /// different permissions than the user chose would be worse than not running.
    async fn apply(
        &mut self,
        connection: &ConnectionTo<Agent>,
        launcher: &AgentLauncher,
        wanted: &SessionSettings,
    ) -> Result<()> {
        self.diverged = false;
        let previous = self.settings.clone();
        let model_changed = previous.as_ref().is_none_or(|p| p.model != wanted.model);
        if model_changed && let Some(model) = &wanted.model {
            let option = select_in(&self.options, &SessionConfigOptionCategory::Model)
                .map_or_else(|| "model".to_owned(), |(option, _)| option.id.to_string());
            let changed = set_option(connection, &self.id, option, model.clone())
                .await
                .map_err(|error| anyhow!("failed to set the model to {model}: {error}"))?;
            self.diverged = true;
            self.note_options(changed);
        }

        let mode = launcher
            .mode_id(wanted.mode)
            .ok_or_else(|| anyhow!("this agent has no permission mode matching that choice"))?;
        self.modes = mode_ids(self.reported_modes.as_ref(), &self.options);
        if self.modes.is_empty() {
            bail!("this agent doesn't report permission modes, so {mode} can't be applied");
        }
        if !self.modes.iter().any(|offered| offered == mode) {
            bail!("this agent doesn't offer the permission mode {mode}");
        }
        if self.current_mode.as_deref() != Some(mode) {
            let failed = |error| anyhow!("failed to set the permission mode to {mode}: {error}");
            let mode_option = select_in(&self.options, &SessionConfigOptionCategory::Mode)
                .map(|(option, _)| option.id.to_string());
            match (&self.reported_modes, mode_option) {
                (None, Some(option)) => {
                    let changed = set_option(connection, &self.id, option, mode.to_owned())
                        .await
                        .map_err(failed)?;
                    self.note_options(changed);
                }
                _ => {
                    connection
                        .send_request(SetSessionModeRequest::new(self.id.clone(), mode))
                        .block_task()
                        .await
                        .map_err(failed)?;
                }
            }
            self.diverged = true;
            self.current_mode = Some(mode.to_owned());
        }

        let effort_changed =
            model_changed || previous.as_ref().is_none_or(|p| p.effort != wanted.effort);
        if effort_changed && let Some(effort) = &wanted.effort {
            let Some((option, _)) =
                select_in(&self.options, &SessionConfigOptionCategory::ThoughtLevel)
            else {
                bail!("this agent has no effort setting for that model");
            };
            let option = option.id.to_string();
            let changed = set_option(connection, &self.id, option, effort.clone())
                .await
                .map_err(|error| anyhow!("failed to set the effort to {effort}: {error}"))?;
            self.note_options(changed);
        }

        self.settings = Some(wanted.clone());
        Ok(())
    }

    /// Takes in what the agent says its options are after a change, which can include a mode
    /// it moved to by itself.
    fn note_options(&mut self, changed: Vec<SessionConfigOption>) {
        if changed.is_empty() {
            return;
        }
        self.options = changed;
        if let Some((_, select)) = select_in(&self.options, &SessionConfigOptionCategory::Mode) {
            self.current_mode = Some(select.current_value.to_string());
        }
    }
}

/// An agent's select option in a category, such as its models.
fn select_in<'a>(
    options: &'a [SessionConfigOption],
    category: &SessionConfigOptionCategory,
) -> Option<(&'a SessionConfigOption, &'a SessionConfigSelect)> {
    options
        .iter()
        .filter(|option| option.category.as_ref() == Some(category))
        .find_map(|option| match &option.kind {
            SessionConfigKind::Select(select) => Some((option, select)),
            _ => None,
        })
}

fn select_values(select: &SessionConfigSelect) -> Vec<&SessionConfigSelectOption> {
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
        SessionConfigSelectOptions::Grouped(groups) => {
            groups.iter().flat_map(|group| &group.options).collect()
        }
        _ => vec![],
    }
}

/// The agent's mode IDs. Some agents only report them as a config option.
fn mode_ids(modes: Option<&SessionModeState>, options: &[SessionConfigOption]) -> Vec<String> {
    match modes {
        Some(modes) => modes
            .available_modes
            .iter()
            .map(|mode| mode.id.to_string())
            .collect(),
        None => select_in(options, &SessionConfigOptionCategory::Mode)
            .map(|(_, select)| select_values(select))
            .unwrap_or_default()
            .iter()
            .map(|option| option.value.to_string())
            .collect(),
    }
}

async fn set_option(
    connection: &ConnectionTo<Agent>,
    session: &SessionId,
    option: String,
    value: String,
) -> Result<Vec<SessionConfigOption>, agent_client_protocol::Error> {
    let response = connection
        .send_request(SetSessionConfigOptionRequest::new(
            session.clone(),
            option,
            SessionConfigOptionValue::value_id(value),
        ))
        .block_task()
        .await?;
    Ok(response.config_options)
}

/// Creates or resumes the agent's session, then applies this session's settings.
async fn open_session(
    core: &Core,
    spawn: &AgentSpawn,
    connection: &ConnectionTo<Agent>,
    init: &InitializeResponse,
    servers: Vec<McpServer>,
    meta: Option<Meta>,
) -> Result<OpenedSession> {
    let (id, modes, options): (SessionId, _, _) = match &spawn.resume {
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
                .send_request(
                    ResumeSessionRequest::new(agent_id.clone(), spawn.cwd.clone())
                        .mcp_servers(servers)
                        .meta(meta),
                )
                .block_task()
                .await
                .map_err(|error| anyhow!("failed to resume the agent's session: {error}"))?;
            (
                agent_id.clone().into(),
                response.modes,
                response.config_options.unwrap_or_default(),
            )
        }
        None => {
            let response = connection
                .send_request(
                    NewSessionRequest::new(spawn.cwd.clone())
                        .mcp_servers(servers)
                        .meta(meta),
                )
                .block_task()
                .await
                .map_err(|error| anyhow!("failed to start the agent's session: {error}"))?;
            core.agent_session_created(&spawn.session_id, &response.session_id.to_string());
            (
                response.session_id,
                response.modes,
                response.config_options.unwrap_or_default(),
            )
        }
    };

    // The config option is the newer of the two.
    let current_mode = select_in(&options, &SessionConfigOptionCategory::Mode)
        .map(|(_, select)| select.current_value.to_string())
        .or_else(|| modes.as_ref().map(|m| m.current_mode_id.to_string()));
    let mut session = OpenedSession {
        id,
        reported_modes: modes,
        options,
        modes: vec![],
        current_mode,
        settings: None,
        diverged: false,
    };
    session
        .apply(connection, &spawn.launcher, &spawn.settings)
        .await?;
    Ok(session)
}

/// The text as typed, then each image after the name the text calls it by, so the agent can
/// tell which one `[Image #2]` means.
fn prompt_blocks(prompt: Prompt) -> Vec<ContentBlock> {
    let mut blocks = vec![ContentBlock::Text(TextContent::new(prompt.text))];
    for (image, data) in prompt.images {
        blocks.push(ContentBlock::Text(TextContent::new(image.token())));
        blocks.push(ContentBlock::Image(ImageContent::new(data, image.mime)));
    }
    blocks
}

/// Starts an agent just long enough to ask what it offers. It sends no prompt,
/// and the session it opens is never used.
pub async fn probe(
    core: &Core,
    name: &str,
    launcher: &AgentLauncher,
    cwd: PathBuf,
) -> Result<tp::Provider> {
    // Its own servers would only slow it down, and it uses none.
    let isolation = mcp::isolation(launcher, &cwd, &[], core.config_home());
    let agent = launch(core, launcher, &isolation).await?;
    Client
        .v1()
        .connect_with(agent, async |connection: ConnectionTo<Agent>| {
            let init = connection
                .send_request(InitializeRequest::new(ProtocolVersion::V1))
                .block_task()
                .await?;
            let session = connection
                .send_request(NewSessionRequest::new(cwd.clone()).meta(isolation.meta.clone()))
                .block_task()
                .await?;
            let options = session.config_options.unwrap_or_default();

            let available = mode_ids(session.modes.as_ref(), &options);
            let modes: Vec<_> = [
                tp::PermissionMode::Manual,
                tp::PermissionMode::Auto,
                tp::PermissionMode::Yolo,
            ]
            .into_iter()
            .filter(|mode| {
                launcher
                    .mode_id(*mode)
                    .is_some_and(|id| available.iter().any(|mode| mode == id))
            })
            .collect();
            if modes.is_empty() {
                return Err(acp_error(
                    "this agent offers no permission mode tectr can use",
                ));
            }

            // Effort levels depend on the model, so each one has to be selected
            // to find out. A model that can't be selected isn't worth offering.
            let model_select = select_in(&options, &SessionConfigOptionCategory::Model);
            let mut models: Vec<tp::ProviderModel> = vec![];
            let mut recommended_name = None;
            if let Some((option, select)) = model_select {
                for model in select_values(select) {
                    let id = model.value.to_string();
                    // Claude's "Default (recommended)" entry names the real model.
                    if id == DEFAULT_ENTRY {
                        recommended_name.clone_from(&model.description);
                        continue;
                    }
                    let changed = set_option(
                        &connection,
                        &session.session_id,
                        option.id.to_string(),
                        id.clone(),
                    )
                    .await;
                    match changed {
                        Ok(changed) => models.push(tp::ProviderModel {
                            id,
                            name: model.name.clone(),
                            efforts: efforts_in(&changed),
                        }),
                        Err(error) => tracing::warn!(model = %id, "skipping a model: {error}"),
                    }
                }
            }

            let recommended_model = recommended_name
                .and_then(|name| models.iter().find(|model| model.name == name))
                .map(|model| model.id.clone());
            let starts_on = model_select
                .map(|(_, select)| select.current_value.to_string())
                .filter(|id| models.iter().any(|model| &model.id == id));
            let default_model = recommended_model
                .clone()
                .or(starts_on)
                .or_else(|| models.first().map(|model| model.id.clone()));
            let default_effort = select_in(&options, &SessionConfigOptionCategory::ThoughtLevel)
                .map(|(_, select)| select.current_value.to_string())
                .filter(|effort| {
                    models
                        .iter()
                        .find(|model| Some(&model.id) == default_model.as_ref())
                        .is_some_and(|model| model.efforts.iter().any(|e| &e.id == effort))
                });

            Ok(tp::Provider {
                agent: name.to_owned(),
                name: launcher.display_name(name),
                version: init.agent_info.as_ref().map(|info| info.version.clone()),
                modes,
                models,
                default_model,
                default_effort,
                recommended_model,
            })
        })
        .await
        .map_err(|error| anyhow!("{error}"))
}

fn efforts_in(options: &[SessionConfigOption]) -> Vec<tp::ProviderEffort> {
    select_in(options, &SessionConfigOptionCategory::ThoughtLevel)
        .map(|(_, select)| select_values(select))
        .unwrap_or_default()
        .iter()
        .filter(|option| option.value.to_string() != DEFAULT_ENTRY)
        .map(|option| tp::ProviderEffort {
            id: option.value.to_string(),
            name: option.name.clone(),
        })
        .collect()
}

fn ready_event(init: &InitializeResponse, session: &OpenedSession) -> tp::SessionEventKind {
    let info = init.agent_info.as_ref();
    tp::SessionEventKind::AgentReady {
        name: info.map_or_else(|| "unknown".to_owned(), |info| info.name.clone()),
        version: info.map(|info| info.version.clone()),
        modes: session.modes.clone(),
        current_mode: session.current_mode.clone(),
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
        SessionUpdate::AvailableCommandsUpdate(update) => core.commands_changed(
            id,
            update
                .available_commands
                .into_iter()
                .map(|command| tp::AgentCommand {
                    hint: match command.input {
                        Some(AvailableCommandInput::Unstructured(input)) => Some(input.hint),
                        _ => None,
                    },
                    name: command.name,
                    description: command.description,
                })
                .collect(),
        ),
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
