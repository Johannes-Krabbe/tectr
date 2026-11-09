//! The tectr wire protocol: JSON messages exchanged over a websocket between
//! a daemon and a client engine.
//!
//! This crate holds types only. It has no sockets and no ACP types, so an ACP
//! upgrade never forces a client update.
//!
//! A connection goes through these steps:
//!
//! 1. The client sends [`ClientMessage::Hello`] with the versions it supports.
//! 2. The daemon replies with [`ServerMessage::Welcome`], or with
//!    [`ServerMessage::VersionRejected`] and closes the connection.
//! 3. The daemon sends [`ServerMessage::Snapshot`], then a
//!    [`ServerMessage::StateChanged`] for every later change.
//! 4. The client sends [`ClientMessage::Subscribe`] for each session it wants
//!    to follow, and receives every [`SessionEvent`] after the given sequence
//!    number, followed by live events.
//!
//! Every enum that can grow has an `Unknown` variant, so an older client can
//! skip a message or value it doesn't understand.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The newest protocol version this build speaks. It's separate from crate
/// versions because components ship independently.
pub const PROTOCOL_VERSION: u32 = 1;
/// The oldest protocol version this build still speaks.
pub const MIN_PROTOCOL_VERSION: u32 = 1;
/// The port a daemon listens on unless configured otherwise.
pub const DEFAULT_PORT: u16 = 7767;

/// Picks the highest version both sides support, if there is one.
pub fn negotiate_version(peer_min: u32, peer_max: u32) -> Option<u32> {
    let version = peer_max.min(PROTOCOL_VERSION);
    (version >= peer_min.max(MIN_PROTOCOL_VERSION)).then_some(version)
}

macro_rules! id_type {
    ($($(#[$meta:meta])* $name:ident),+ $(,)?) => {$(
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    )+};
}

id_type!(
    /// Identifies a daemon's machine. It's generated once and never changes.
    MachineId,
    WorkspaceId,
    TopicId,
    WorktreeId,
    /// Unique within a machine. Use [`SessionRef`] when merging many daemons.
    SessionId,
    ApprovalId,
);

/// Identifies a session across all daemons.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionRef {
    pub machine_id: MachineId,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Must be the first message on a connection.
    Hello {
        min_version: u32,
        max_version: u32,
        client: String,
    },
    /// Answered by exactly one [`ServerMessage::Response`] with the same `id`.
    /// The response comes after every state change and followed session event
    /// the command caused, so a client's state is current once it arrives.
    Request {
        id: u64,
        command: Command,
    },
    /// Start or restart following a session from just after `after_seq`.
    /// Use 0 for the whole history.
    Subscribe {
        session_id: SessionId,
        after_seq: u64,
    },
    Unsubscribe {
        session_id: SessionId,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        version: u32,
        machine: Machine,
        daemon_version: String,
    },
    /// No version in common. The daemon closes the connection after this.
    VersionRejected {
        min_version: u32,
        max_version: u32,
    },
    /// Replaces everything the client knows about this daemon's state.
    Snapshot {
        state: DaemonState,
    },
    StateChanged {
        change: StateChange,
    },
    SessionEvent {
        event: SessionEvent,
    },
    Response {
        id: u64,
        result: Result<CommandOutput, CommandError>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Add an existing git repository on the daemon's machine. The path is absolute or
    /// starts with `~`, which means the home folder on that machine.
    AddWorkspace {
        path: String,
    },
    /// List the folders directly inside a folder on the daemon's machine, for completing
    /// a path as it's typed. The path works as in [`Command::AddWorkspace`]. A folder that
    /// doesn't exist has no folders in it, because that's normal halfway through typing.
    ListFolders {
        path: String,
    },
    /// Create a topic on a fresh branch and worktree, or in the repo itself.
    CreateTopic {
        workspace_id: WorkspaceId,
        /// `None` numbers it within its workspace, like "Topic #3", until its first tab has a
        /// title, which it then goes by.
        name: Option<String>,
        /// Work in the repo's own folder on its current branch, with no new worktree.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        at_root: bool,
    },
    /// Give a topic the name a person chose. From then on it stops following its first
    /// tab's title.
    RenameTopic {
        topic_id: TopicId,
        name: String,
    },
    /// Stop the topic's agents and remove it. Its worktree goes too, unless it has uncommitted
    /// changes or is the repo's own folder, and its branch goes only if it's fully merged. Fails
    /// with [`ErrorCode::Busy`] while an agent is starting, working or waiting, unless `force`
    /// says the person has confirmed. `force` never discards anything in git.
    DeleteTopic {
        topic_id: TopicId,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Start an agent in one of the topic's worktrees. It returns as soon as
    /// the session exists. Follow its events to see it become ready.
    StartSession {
        topic_id: TopicId,
        worktree_id: WorktreeId,
        agent: String,
        mode: PermissionMode,
        /// The agent's own model name. `None` keeps the agent's default.
        model: Option<String>,
        /// The agent's own effort level. `None` keeps the agent's default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    /// Fails with [`ErrorCode::Busy`] while a turn is running. If the agent
    /// process has stopped, the daemon restarts and resumes it first.
    SendPrompt {
        session_id: SessionId,
        text: String,
    },
    /// Interrupt the current turn.
    CancelTurn {
        session_id: SessionId,
    },
    /// Answer a pending approval with one of its options. Only the first
    /// answer counts. Later ones fail with [`ErrorCode::NotFound`].
    AnswerApproval {
        approval_id: ApprovalId,
        option_id: String,
    },
    /// Check which agents this device could run. Nothing is started.
    DetectProviders,
    /// Start the agent once and remember what it offers on this device. Adding
    /// one again refreshes what it offers.
    AddProvider {
        agent: String,
    },
    RemoveProvider {
        agent: String,
    },
    /// Choose what new sessions with this provider start with. The effort must be
    /// one the model offers.
    SetProviderDefaults {
        agent: String,
        model: Option<String>,
        effort: Option<String>,
    },
    /// Change what a session runs with. A running agent changes straight away, even during
    /// a turn, and the session only reports the change once the agent has made it. A stopped
    /// agent gets these when it next starts. They're sent together because the efforts on
    /// offer depend on the model. `None` leaves the agent's own value alone.
    SetSessionSettings {
        session_id: SessionId,
        mode: PermissionMode,
        model: Option<String>,
        effort: Option<String>,
    },
    /// Keep what's typed for a session but not sent yet.
    SetDraft {
        session_id: SessionId,
        text: String,
    },
    /// Stop the agent and remove its tab. Fails with [`ErrorCode::Busy`] while the agent
    /// is starting, working or waiting, unless `force` says the person has confirmed.
    CloseSession {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "output", rename_all = "snake_case")]
pub enum CommandOutput {
    Workspace {
        workspace: Workspace,
    },
    Topic {
        workspace_id: WorkspaceId,
        topic: Topic,
    },
    Session {
        topic_id: TopicId,
        session: Session,
    },
    ProviderCandidates {
        candidates: Vec<ProviderCandidate>,
    },
    Provider {
        provider: Provider,
    },
    Folders {
        folders: Vec<Folder>,
    },
    Done,
    #[serde(other)]
    Unknown,
}

/// A folder on a daemon's machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    /// Absolute, so it can be compared with a workspace's path.
    pub path: String,
    /// False can also mean the daemon didn't look, because looking would have asked
    /// the person for permission.
    pub is_repo: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    NotFound,
    InvalidInput,
    Busy,
    Internal,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    pub id: MachineId,
    pub name: String,
}

/// Everything a daemon knows that clients show, so that every client sees the same. What
/// each client is looking at is its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonState {
    pub machine: Machine,
    pub workspaces: Vec<Workspace>,
    /// Permission requests waiting for an answer, across all sessions.
    pub approvals: Vec<Approval>,
    /// The agents added to this device, part of its settings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<Provider>,
    /// What's typed for each session but not sent yet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drafts: BTreeMap<SessionId, String>,
}

impl DaemonState {
    /// Applies a live change. Changes are idempotent, so applying one twice
    /// has no further effect.
    pub fn apply(&mut self, change: &StateChange) {
        match change {
            StateChange::WorkspaceAdded { workspace } => {
                upsert(&mut self.workspaces, workspace.clone(), |w| &w.id);
            }
            StateChange::TopicAdded {
                workspace_id,
                topic,
            } => {
                if let Some(workspace) = self.workspaces.iter_mut().find(|w| &w.id == workspace_id)
                {
                    upsert(&mut workspace.topics, topic.clone(), |t| &t.id);
                }
            }
            StateChange::TopicRenamed { topic_id, name } => {
                if let Some(topic) = self.topic_mut(topic_id) {
                    topic.name = name.clone();
                }
            }
            StateChange::TopicRemoved { topic_id } => {
                for workspace in &mut self.workspaces {
                    workspace.topics.retain(|t| &t.id != topic_id);
                }
            }
            StateChange::SessionAdded { topic_id, session } => {
                if let Some(topic) = self.topic_mut(topic_id) {
                    upsert(&mut topic.sessions, session.clone(), |s| &s.id);
                }
            }
            StateChange::SessionChanged { session } => {
                if let Some(existing) = self.session_mut(&session.id) {
                    *existing = session.clone();
                }
            }
            StateChange::ApprovalAdded { approval } => {
                upsert(&mut self.approvals, approval.clone(), |a| &a.id);
            }
            StateChange::ApprovalResolved { approval_id } => {
                self.approvals.retain(|a| &a.id != approval_id);
            }
            StateChange::ProviderChanged { provider } => {
                upsert(&mut self.providers, provider.clone(), |p| &p.agent);
            }
            StateChange::ProviderRemoved { agent } => {
                self.providers.retain(|p| &p.agent != agent);
            }
            StateChange::SessionRemoved { session_id } => {
                for topic in self.workspaces.iter_mut().flat_map(|w| &mut w.topics) {
                    topic.sessions.retain(|s| &s.id != session_id);
                }
            }
            StateChange::DraftChanged { session_id, text } => {
                if text.is_empty() {
                    self.drafts.remove(session_id);
                } else {
                    self.drafts.insert(session_id.clone(), text.clone());
                }
            }
            StateChange::Unknown => {}
        }
    }

    pub fn workspace(&self, id: &WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| &w.id == id)
    }

    pub fn topics(&self) -> impl Iterator<Item = &Topic> {
        self.workspaces.iter().flat_map(|w| &w.topics)
    }

    pub fn topic(&self, id: &TopicId) -> Option<&Topic> {
        self.topics().find(|t| &t.id == id)
    }

    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.topics().flat_map(|t| &t.sessions)
    }

    pub fn session(&self, id: &SessionId) -> Option<&Session> {
        self.sessions().find(|s| &s.id == id)
    }

    fn topic_mut(&mut self, id: &TopicId) -> Option<&mut Topic> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| &mut w.topics)
            .find(|t| &t.id == id)
    }

    pub fn session_mut(&mut self, id: &SessionId) -> Option<&mut Session> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| &mut w.topics)
            .flat_map(|t| &mut t.sessions)
            .find(|s| &s.id == id)
    }
}

fn upsert<T, K: PartialEq>(items: &mut Vec<T>, item: T, key: impl Fn(&T) -> &K) {
    match items
        .iter_mut()
        .find(|existing| key(existing) == key(&item))
    {
        Some(existing) => *existing = item,
        None => items.push(item),
    }
}

/// A repository on one device. The same repo on two devices is two workspaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub path: String,
    pub topics: Vec<Topic>,
}

/// A feature or task inside a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    pub id: TopicId,
    pub name: String,
    pub worktrees: Vec<Worktree>,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub path: String,
    pub branch: String,
}

/// One agent run. It points at a worktree without owning it, so nothing here
/// decides yet whether agents may share a worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub worktree_id: WorktreeId,
    pub agent: String,
    pub mode: PermissionMode,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub status: SessionStatus,
    pub title: Option<String>,
}

/// An agent this device knows how to run. It may not be installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCandidate {
    pub agent: String,
    pub name: String,
    /// Why it can't be added on this device. `None` means it can.
    pub missing: Option<String>,
}

/// An agent added to this device, with what it reported when last started.
/// Models and effort levels come from the agent, so new ones need no update here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    pub agent: String,
    pub name: String,
    pub version: Option<String>,
    /// The permission modes this agent can honor.
    pub modes: Vec<PermissionMode>,
    pub models: Vec<ProviderModel>,
    /// What new sessions start with. It's chosen in the device's settings, and
    /// starts out as what the agent recommends or would use itself.
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
    /// The model the agent itself names as recommended, if it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_model: Option<String>,
}

impl Provider {
    pub fn model(&self, id: &str) -> Option<&ProviderModel> {
        self.models.iter().find(|model| model.id == id)
    }

    /// Takes over the choices from before this provider was asked again, as far as
    /// the agent still offers them.
    pub fn keep_defaults(&mut self, previous: &Provider) {
        if let Some(model) = &previous.default_model
            && self.model(model).is_some()
        {
            self.default_model = Some(model.clone());
        }
        let offered = |provider: &Provider, effort: &str| {
            provider
                .default_model
                .as_deref()
                .and_then(|model| provider.model(model))
                .is_some_and(|model| model.efforts.iter().any(|e| e.id == effort))
        };
        if let Some(effort) = &previous.default_effort
            && offered(self, effort)
        {
            self.default_effort = Some(effort.clone());
        }
        if let Some(effort) = self.default_effort.clone()
            && !offered(self, &effort)
        {
            self.default_effort = None;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
    /// Effort levels depend on the model, and some models have none.
    pub efforts: Vec<ProviderEffort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderEffort {
    pub id: String,
    pub name: String,
}

/// How much the agent may do without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Always ask before making changes.
    Manual,
    /// The agent decides what needs asking. This is tectr's default.
    Auto,
    /// Never ask.
    Yolo,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// The agent process is starting.
    Starting,
    /// The agent is up and has had no prompt yet.
    Ready,
    Working,
    WaitingForYou,
    /// The last turn finished.
    Done,
    /// The agent process or last turn failed. Sending a prompt tries again.
    Failed,
    /// The agent process isn't running. Sending a prompt resumes it.
    Stopped,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum StateChange {
    WorkspaceAdded {
        workspace: Workspace,
    },
    TopicAdded {
        workspace_id: WorkspaceId,
        topic: Topic,
    },
    TopicRenamed {
        topic_id: TopicId,
        name: String,
    },
    /// It was deleted, along with its tabs. Any open approvals and drafts of its sessions are
    /// reported as resolved and emptied before this.
    TopicRemoved {
        topic_id: TopicId,
    },
    SessionAdded {
        topic_id: TopicId,
        session: Session,
    },
    SessionChanged {
        session: Session,
    },
    ApprovalAdded {
        approval: Approval,
    },
    ApprovalResolved {
        approval_id: ApprovalId,
    },
    ProviderChanged {
        provider: Provider,
    },
    ProviderRemoved {
        agent: String,
    },
    /// Its tab was closed and its agent stopped.
    SessionRemoved {
        session_id: SessionId,
    },
    /// Empty means there's no draft any more.
    DraftChanged {
        session_id: SessionId,
        text: String,
    },
    #[serde(other)]
    Unknown,
}

/// A permission request waiting for an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub id: ApprovalId,
    pub session_id: SessionId,
    pub tool_call: ToolCallPatch,
    pub options: Vec<PermissionOption>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    #[serde(other)]
    Unknown,
}

/// One entry in a session's history. `seq` starts at 1 and has no gaps, so a
/// client can reconnect and ask for everything after the last one it saw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    pub session: SessionRef,
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub at_ms: i64,
    pub kind: SessionEventKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum SessionEventKind {
    StatusChanged {
        status: SessionStatus,
    },
    /// What the running agent reports about itself. A client can adapt its UI
    /// to this, because agents don't all offer the same things.
    AgentReady {
        name: String,
        version: Option<String>,
        modes: Vec<String>,
        current_mode: Option<String>,
    },
    UserPrompt {
        text: String,
    },
    AgentMessage {
        text: String,
    },
    AgentThought {
        text: String,
    },
    ToolCall {
        id: String,
        tool: ToolCallPatch,
    },
    ToolCallUpdate {
        id: String,
        tool: ToolCallPatch,
    },
    PermissionRequested {
        approval: Approval,
    },
    PermissionResolved {
        approval_id: ApprovalId,
        outcome: PermissionOutcome,
    },
    TurnEnded {
        stop_reason: StopReason,
    },
    Error {
        message: String,
    },
    #[serde(other)]
    Unknown,
}

/// The fields of a tool call that an event sets. A missing field keeps its
/// previous value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A category such as `edit` or `execute`. It's a string so that a new
    /// category doesn't need a protocol change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<ToolCallContent>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolCallContent {
    Text {
        text: String,
    },
    Diff {
        path: String,
        old_text: Option<String>,
        new_text: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionOutcome {
    Selected {
        option_id: String,
    },
    /// The turn was cancelled, or the agent went away, before anyone answered.
    Cancelled,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_picks_highest_common_version() {
        assert_eq!(
            negotiate_version(MIN_PROTOCOL_VERSION, PROTOCOL_VERSION),
            Some(PROTOCOL_VERSION)
        );
        assert_eq!(
            negotiate_version(0, PROTOCOL_VERSION + 5),
            Some(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn negotiation_fails_without_a_common_version() {
        assert_eq!(
            negotiate_version(PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 5),
            None
        );
        assert_eq!(negotiate_version(0, MIN_PROTOCOL_VERSION - 1), None);
    }

    #[test]
    fn older_clients_skip_what_they_dont_know() {
        let message: ServerMessage =
            serde_json::from_str(r#"{"type":"from_the_future","x":1}"#).unwrap();
        assert_eq!(message, ServerMessage::Unknown);

        let kind: SessionEventKind =
            serde_json::from_str(r#"{"event":"from_the_future","x":1}"#).unwrap();
        assert_eq!(kind, SessionEventKind::Unknown);

        let status: SessionStatus = serde_json::from_str(r#""from_the_future""#).unwrap();
        assert_eq!(status, SessionStatus::Unknown);
    }

    #[test]
    fn applying_a_change_twice_has_no_further_effect() {
        let mut state = DaemonState {
            machine: Machine {
                id: "m".into(),
                name: "m".into(),
            },
            workspaces: vec![],
            approvals: vec![],
            providers: vec![],
            drafts: BTreeMap::new(),
        };
        let change = StateChange::WorkspaceAdded {
            workspace: Workspace {
                id: "w".into(),
                name: "w".into(),
                path: "/w".into(),
                topics: vec![],
            },
        };
        state.apply(&change);
        state.apply(&change);
        assert_eq!(state.workspaces.len(), 1);
    }
}
