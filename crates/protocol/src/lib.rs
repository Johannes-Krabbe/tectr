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
    /// Add an existing git repository on the daemon's machine.
    AddWorkspace { path: String },
    /// Create a topic with a fresh branch and worktree.
    CreateTopic {
        workspace_id: WorkspaceId,
        name: String,
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
    },
    /// Fails with [`ErrorCode::Busy`] while a turn is running. If the agent
    /// process has stopped, the daemon restarts and resumes it first.
    SendPrompt { session_id: SessionId, text: String },
    /// Interrupt the current turn.
    CancelTurn { session_id: SessionId },
    /// Answer a pending approval with one of its options. Only the first
    /// answer counts. Later ones fail with [`ErrorCode::NotFound`].
    AnswerApproval {
        approval_id: ApprovalId,
        option_id: String,
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
    Done,
    #[serde(other)]
    Unknown,
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

/// Everything a client needs to draw a daemon's part of the tree and inbox.
///
/// Tabs aren't here. They're a client's view of a session, terminal, or diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonState {
    pub machine: Machine,
    pub workspaces: Vec<Workspace>,
    /// Permission requests waiting for an answer, across all sessions.
    pub approvals: Vec<Approval>,
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
    pub status: SessionStatus,
    pub title: Option<String>,
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
