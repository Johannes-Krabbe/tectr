//! What an agent says while it runs, turned into tectr events. The agent's own session and
//! each of its subagents' sessions arrive on the same connection, told apart by session ID.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v1::schema::{
    AvailableCommandInput, CompactionStatus, ContentBlock, ContentChunk, NoticeSeverity,
    PlanEntryStatus, PlanUpdateContent, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOptions, SessionUpdate, StateUpdate, ToolCall,
    ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdateFields, ToolKind,
};
use protocol as tp;
use serde_json::Value;

use crate::agents::AgentLauncher;
use crate::core::Core;
use crate::tasks::{Report, Tasks};

/// What the connection's callbacks and its command loop both need to know.
#[derive(Default)]
pub struct Shared {
    /// The agent's own ID for the session, once it has one. Until then, everything is the
    /// session's own.
    pub root: Option<String>,
    /// The agent's mode as of the last thing it said or was told.
    pub current_mode: Option<String>,
    /// The agent's options, as it last reported them.
    pub options: Vec<SessionConfigOption>,
    /// The last thing the session's own agent said this turn, which goes on the turn's end.
    pub last_message: Option<String>,
    /// Summaries of compactions so far, which arrive in pieces.
    compactions: HashMap<String, (tp::CompactionStatus, String, Option<String>)>,
    /// Messages between sessions, which can arrive in pieces.
    messages: HashMap<String, (Option<String>, Option<String>, String)>,
    tasks: Tasks,
    last_usage: Option<(u64, u64)>,
    last_plan: Option<Vec<tp::PlanEntry>>,
}

pub struct Updates {
    core: Arc<Core>,
    id: tp::SessionId,
    launcher: AgentLauncher,
    pub shared: Arc<Mutex<Shared>>,
}

impl Updates {
    pub fn new(core: Arc<Core>, id: tp::SessionId, launcher: AgentLauncher) -> Self {
        Self {
            core,
            id,
            launcher,
            shared: Arc::default(),
        }
    }

    pub fn session_id(&self) -> &tp::SessionId {
        &self.id
    }

    /// The subagent a session ID belongs to, or `None` for the session's own agent.
    pub fn subagent_of(&self, session: &str) -> Option<String> {
        let shared = self.shared.lock().unwrap();
        match &shared.root {
            Some(root) if root == session => None,
            None => None,
            Some(_) => Some(session.to_owned()),
        }
    }

    /// A `session/update` as it came, so that one this daemon doesn't know, like a newer
    /// agent's, is skipped rather than ending the connection.
    pub fn session_update(&self, session: &str, update: Value) {
        match serde_json::from_value::<SessionUpdate>(update) {
            Ok(update) => self.handle(session, update),
            Err(error) => tracing::debug!(session = %self.id, "skipping an update: {error}"),
        }
    }

    fn event(&self, subagent: Option<String>, kind: tp::SessionEventKind) {
        self.core.subagent_event(&self.id, subagent, kind);
    }

    fn handle(&self, session: &str, update: SessionUpdate) {
        let subagent = self.subagent_of(session);
        let own = subagent.is_none();
        match update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if own {
                    self.note_message(&chunk);
                }
                if let ContentBlock::Text(text) = chunk.content {
                    self.event(
                        subagent,
                        tp::SessionEventKind::AgentMessage { text: text.text },
                    );
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if own {
                    self.note_message(&chunk);
                }
                if let ContentBlock::Text(text) = chunk.content {
                    self.event(
                        subagent,
                        tp::SessionEventKind::AgentThought { text: text.text },
                    );
                }
            }
            SessionUpdate::ToolCall(call) => {
                let id = call.tool_call_id.to_string();
                let tool = patch_from_call(call);
                self.note_tool(&id, &tool);
                self.event(subagent, tp::SessionEventKind::ToolCall { id, tool });
            }
            SessionUpdate::ToolCallUpdate(update) => {
                let id = update.tool_call_id.to_string();
                let tool = patch_from_update(update.fields);
                self.note_tool(&id, &tool);
                self.event(subagent, tp::SessionEventKind::ToolCallUpdate { id, tool });
            }
            SessionUpdate::AvailableCommandsUpdate(update) if own => self.core.commands_changed(
                &self.id,
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
            SessionUpdate::SessionInfoUpdate(info) if own => {
                // A missing title means "unchanged", and only an explicit null clears it.
                match info.title {
                    MaybeUndefined::Value(title) => self.core.title_changed(&self.id, Some(title)),
                    MaybeUndefined::Null => self.core.title_changed(&self.id, None),
                    MaybeUndefined::Undefined => {}
                }
                if let Some(suggestions) = info
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("promptSuggestions"))
                {
                    let text = suggestions
                        .get("suggestions")
                        .and_then(Value::as_array)
                        .and_then(|all| {
                            all.iter()
                                .filter_map(|s| s.get("text").and_then(Value::as_str))
                                .find(|text| !text.trim().is_empty())
                        })
                        .map(str::to_owned);
                    self.event(None, tp::SessionEventKind::Suggestion { text });
                }
            }
            SessionUpdate::CurrentModeUpdate(update) if own => {
                self.mode_changed(update.current_mode_id.to_string());
            }
            SessionUpdate::ConfigOptionUpdate(update) if own => {
                let mode = update
                    .config_options
                    .iter()
                    .filter(|o| o.category == Some(SessionConfigOptionCategory::Mode))
                    .find_map(|o| match &o.kind {
                        SessionConfigKind::Select(select) => Some(select.current_value.to_string()),
                        _ => None,
                    });
                let moved = {
                    let mut shared = self.shared.lock().unwrap();
                    shared.options = update.config_options;
                    mode.filter(|mode| shared.current_mode.as_ref() != Some(mode))
                };
                if let Some(mode) = moved {
                    self.mode_changed(mode);
                }
                self.options_changed();
            }
            SessionUpdate::Plan(plan) => {
                let entries: Vec<tp::PlanEntry> = plan
                    .entries
                    .into_iter()
                    .map(|entry| tp::PlanEntry {
                        content: entry.content,
                        status: plan_status(&entry.status),
                    })
                    .collect();
                if own {
                    let mut shared = self.shared.lock().unwrap();
                    if shared.last_plan.as_ref() == Some(&entries) {
                        return;
                    }
                    shared.last_plan = Some(entries.clone());
                }
                self.event(subagent, tp::SessionEventKind::Plan { entries });
            }
            SessionUpdate::PlanUpdate(update) => match update.plan {
                PlanUpdateContent::Markdown(plan) => self.event(
                    subagent,
                    tp::SessionEventKind::PlanDocument {
                        id: plan.plan_id.to_string(),
                        text: plan.content,
                    },
                ),
                PlanUpdateContent::Items(items) => self.event(
                    subagent,
                    tp::SessionEventKind::Plan {
                        entries: items
                            .entries
                            .into_iter()
                            .map(|entry| tp::PlanEntry {
                                content: entry.content,
                                status: plan_status(&entry.status),
                            })
                            .collect(),
                    },
                ),
                _ => {}
            },
            SessionUpdate::UsageUpdate(usage) if own => {
                {
                    let mut shared = self.shared.lock().unwrap();
                    if shared.last_usage == Some((usage.used, usage.size)) {
                        return;
                    }
                    shared.last_usage = Some((usage.used, usage.size));
                }
                self.event(
                    None,
                    tp::SessionEventKind::Usage {
                        used: usage.used,
                        size: usage.size,
                        cost: usage.cost.map(|cost| tp::Cost {
                            amount: cost.amount,
                            currency: cost.currency,
                        }),
                    },
                );
            }
            SessionUpdate::Notice(notice) => self.event(
                subagent,
                tp::SessionEventKind::Notice {
                    severity: match notice.severity {
                        NoticeSeverity::Info => tp::NoticeSeverity::Info,
                        NoticeSeverity::Warning => tp::NoticeSeverity::Warning,
                        NoticeSeverity::Error => tp::NoticeSeverity::Error,
                        _ => tp::NoticeSeverity::Unknown,
                    },
                    title: notice.title,
                    description: notice.description,
                },
            ),
            SessionUpdate::CompactionUpdate(update) => {
                let id = update.compaction_id.to_string();
                let status = compaction_status(&update.status);
                let event = {
                    let mut shared = self.shared.lock().unwrap();
                    let entry = shared.compactions.entry(id.clone()).or_insert((
                        status,
                        String::new(),
                        None,
                    ));
                    entry.0 = status;
                    match update.summary {
                        MaybeUndefined::Value(blocks) => entry.1 = text_of(&blocks),
                        MaybeUndefined::Null => entry.1.clear(),
                        MaybeUndefined::Undefined => {}
                    }
                    match update.error {
                        MaybeUndefined::Value(error) => entry.2 = Some(error),
                        MaybeUndefined::Null => entry.2 = None,
                        MaybeUndefined::Undefined => {}
                    }
                    compaction_event(id, entry)
                };
                self.event(subagent, event);
            }
            SessionUpdate::CompactionSummaryChunk(chunk) => {
                let id = chunk.compaction_id.to_string();
                let event = {
                    let mut shared = self.shared.lock().unwrap();
                    let entry = shared.compactions.entry(id.clone()).or_insert((
                        tp::CompactionStatus::InProgress,
                        String::new(),
                        None,
                    ));
                    entry
                        .1
                        .push_str(&text_of(std::slice::from_ref(&chunk.content)));
                    compaction_event(id, entry)
                };
                self.event(subagent, event);
            }
            SessionUpdate::SubagentUpdate(update) => {
                let child = update.session_id.to_string();
                self.event(
                    subagent,
                    tp::SessionEventKind::Subagent {
                        id: child,
                        title: patched(update.title),
                        description: patched(update.description),
                        status: match update.state {
                            MaybeUndefined::Value(state) => Some(match state {
                                StateUpdate::Running(_) => tp::SubagentStatus::Working,
                                StateUpdate::Idle(_) => tp::SubagentStatus::Done,
                                StateUpdate::RequiresAction(_) => tp::SubagentStatus::WaitingForYou,
                                _ => tp::SubagentStatus::Unknown,
                            }),
                            _ => None,
                        },
                    },
                );
            }
            SessionUpdate::SessionMessage(message) => {
                let id = message.message_id.to_string();
                let event = {
                    let mut shared = self.shared.lock().unwrap();
                    let root = shared.root.clone();
                    let entry =
                        shared
                            .messages
                            .entry(id.clone())
                            .or_insert((None, None, String::new()));
                    let party =
                        |session: Option<String>| session.filter(|s| Some(s) != root.as_ref());
                    if let MaybeUndefined::Value(sender) = message.sender_session_id {
                        entry.0 = party(Some(sender.to_string()));
                    }
                    if let MaybeUndefined::Value(recipient) = message.recipient_session_id {
                        entry.1 = party(Some(recipient.to_string()));
                    }
                    match message.content {
                        MaybeUndefined::Value(blocks) => entry.2 = text_of(&blocks),
                        MaybeUndefined::Null => entry.2.clear(),
                        MaybeUndefined::Undefined => {}
                    }
                    session_message_event(id, entry)
                };
                self.event(subagent, event);
            }
            SessionUpdate::SessionMessageChunk(chunk) => {
                let id = chunk.message_id.to_string();
                let event = {
                    let mut shared = self.shared.lock().unwrap();
                    let entry =
                        shared
                            .messages
                            .entry(id.clone())
                            .or_insert((None, None, String::new()));
                    entry
                        .2
                        .push_str(&text_of(std::slice::from_ref(&chunk.content)));
                    session_message_event(id, entry)
                };
                self.event(subagent, event);
            }
            _ => {}
        }
    }

    /// Keeps the ID of a message the agent is saying, unless a subagent says it. Claude's
    /// adapter marks those when it isn't told about subagents, and their messages live in
    /// another transcript, so no branch can start there.
    fn note_message(&self, chunk: &ContentChunk) {
        let subagent = chunk
            .meta
            .as_ref()
            .and_then(|meta| meta.get("claudeCode")?.get("parentToolUseId"))
            .is_some();
        if let (Some(message_id), false) = (&chunk.message_id, subagent) {
            self.shared.lock().unwrap().last_message = Some(message_id.0.to_string());
        }
    }

    /// A tool's input and output can name a background task's command and file.
    fn note_tool(&self, id: &str, tool: &tp::ToolCallPatch) {
        let texts: Vec<&str> = tool
            .content
            .iter()
            .flatten()
            .filter_map(|content| match content {
                tp::ToolCallContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let reports = {
            let mut shared = self.shared.lock().unwrap();
            shared.tasks.note_tool_call(id, tool.input.as_ref(), &texts);
            shared.tasks.output_found()
        };
        self.reports(reports);
    }

    /// One of Claude Code's own messages, which its adapter passes on as asked.
    pub fn claude_message(&self, message: &Value) {
        let reports = self.shared.lock().unwrap().tasks.read(message);
        self.reports(reports);
    }

    fn reports(&self, reports: Vec<Report>) {
        for report in reports {
            match report {
                Report::Task(task) => {
                    self.core.task_changed(&self.id, task.clone());
                    self.event(None, tp::SessionEventKind::Task { task });
                }
                Report::Working(working) => {
                    let last_message = if working {
                        None
                    } else {
                        self.shared.lock().unwrap().last_message.take()
                    };
                    self.core.agent_working(&self.id, working, last_message);
                }
            }
        }
    }

    /// The agent moved to another mode by itself, as Claude does once a plan is approved.
    pub fn mode_changed(&self, mode_id: String) {
        self.shared.lock().unwrap().current_mode = Some(mode_id.clone());
        if let Some(mode) = self.launcher.mode_for_id(&mode_id) {
            self.core.mode_changed(&self.id, mode);
        }
    }

    /// Tells the session about the agent's own options, after any change to them. The mode
    /// isn't read from them here: an agent that takes its mode another way leaves the mode
    /// in its options as it was.
    pub fn options_changed(&self) {
        let extras = extra_options(&self.shared.lock().unwrap().options);
        self.core.options_changed(&self.id, extras);
    }
}

/// An option tectr doesn't set itself, which the person can.
fn extra_options(options: &[SessionConfigOption]) -> Vec<tp::AgentOption> {
    options
        .iter()
        .filter(|option| {
            !matches!(
                option.category,
                Some(
                    SessionConfigOptionCategory::Model
                        | SessionConfigOptionCategory::Mode
                        | SessionConfigOptionCategory::ThoughtLevel
                )
            )
        })
        .filter_map(|option| {
            let SessionConfigKind::Select(select) = &option.kind else {
                return None;
            };
            let choices: Vec<tp::OptionChoice> = match &select.options {
                SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
                SessionConfigSelectOptions::Grouped(groups) => {
                    groups.iter().flat_map(|group| &group.options).collect()
                }
                _ => vec![],
            }
            .into_iter()
            .map(|choice| tp::OptionChoice {
                value: choice.value.to_string(),
                name: choice.name.clone(),
            })
            .collect();
            Some(tp::AgentOption {
                id: option.id.to_string(),
                name: option.name.clone(),
                description: option.description.clone(),
                value: select.current_value.to_string(),
                choices,
            })
        })
        .collect()
}

fn patched(value: MaybeUndefined<String>) -> Option<String> {
    match value {
        MaybeUndefined::Value(value) => Some(value),
        _ => None,
    }
}

fn compaction_event(
    id: String,
    (status, summary, error): &(tp::CompactionStatus, String, Option<String>),
) -> tp::SessionEventKind {
    tp::SessionEventKind::Compaction {
        id,
        status: *status,
        summary: (!summary.is_empty()).then(|| summary.clone()),
        error: error.clone(),
    }
}

fn session_message_event(
    id: String,
    (from, to, text): &(Option<String>, Option<String>, String),
) -> tp::SessionEventKind {
    tp::SessionEventKind::SessionMessage {
        id,
        from: from.clone(),
        to: to.clone(),
        text: text.clone(),
    }
}

fn text_of(blocks: &[ContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn plan_status(status: &PlanEntryStatus) -> tp::PlanEntryStatus {
    match status {
        PlanEntryStatus::Pending => tp::PlanEntryStatus::Pending,
        PlanEntryStatus::InProgress => tp::PlanEntryStatus::InProgress,
        PlanEntryStatus::Completed => tp::PlanEntryStatus::Completed,
        _ => tp::PlanEntryStatus::Unknown,
    }
}

fn compaction_status(status: &CompactionStatus) -> tp::CompactionStatus {
    match status {
        CompactionStatus::InProgress => tp::CompactionStatus::InProgress,
        CompactionStatus::Completed => tp::CompactionStatus::Completed,
        CompactionStatus::Failed => tp::CompactionStatus::Failed,
        CompactionStatus::Cancelled => tp::CompactionStatus::Cancelled,
        _ => tp::CompactionStatus::Unknown,
    }
}

pub fn patch_from_call(call: ToolCall) -> tp::ToolCallPatch {
    tp::ToolCallPatch {
        title: Some(call.title),
        kind: Some(kind_name(call.kind)),
        status: Some(status_from_acp(call.status)),
        input: call.raw_input,
        locations: Some(call.locations.iter().map(location_path).collect()),
        content: Some(call.content.into_iter().map(content_from_acp).collect()),
    }
}

pub fn patch_from_update(fields: ToolCallUpdateFields) -> tp::ToolCallPatch {
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
