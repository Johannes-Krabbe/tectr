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
/// Which install a build belongs to. Dev and release each get their own daemon port, data
/// and settings, so working on tectr never touches the installed app's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Dev,
    Release,
}

impl Channel {
    /// Only the release script builds with `TECTR_CHANNEL=release`. Everything else,
    /// including `cargo run`, tests and `cargo build --release`, is dev, so a stray build
    /// can't take over the installed daemon's port or data.
    pub fn current() -> Self {
        match option_env!("TECTR_CHANNEL") {
            Some("release") => Self::Release,
            _ => Self::Dev,
        }
    }

    /// The port this channel's daemon listens on unless configured otherwise.
    pub fn default_port(self) -> u16 {
        match self {
            Self::Release => 7767,
            Self::Dev => 7769,
        }
    }

    /// The folder name for this channel's data and settings, such as
    /// `~/Library/Application Support/<dir_name>`.
    pub fn dir_name(self) -> &'static str {
        match self {
            Self::Release => "tectr",
            Self::Dev => "tectr-dev",
        }
    }
}

/// The daemon on this machine for this build's channel.
pub fn local_daemon_url() -> String {
    format!("ws://127.0.0.1:{}", Channel::current().default_port())
}

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
    McpServerId,
    /// Names an image by what's in it, so the same image always has the same ID, on any
    /// device. See [`Command::AddImage`].
    ImageId,
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
    /// Stop the topic's agents, take it out of the app and remove its worktrees and the branches
    /// tectr made for them, uncommitted work and unpushed commits included. The repo's own folder
    /// is never touched. Without `force`, nothing happens if that would lose something, and the
    /// answer is an [`CommandOutput::ArchiveRisk`] saying what, so a person can confirm it.
    ArchiveTopic {
        topic_id: TopicId,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Bring an archived topic back with its tabs. Its folder is made again at the same path,
    /// on its branch if that still exists here or on a remote, and otherwise on a new one.
    RestoreTopic {
        topic_id: TopicId,
    },
    ListArchivedTopics,
    /// Move every tab of a topic to another folder. Agents stop and pick up in the new folder
    /// when next prompted, told that they moved. Fails with [`ErrorCode::Busy`] while an agent
    /// is starting, working or waiting, unless `force` says the person has confirmed.
    MoveTopic {
        topic_id: TopicId,
        to: MoveTarget,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Start an agent in the topic's folder. It returns as soon as the session exists. Follow
    /// its events to see it become ready.
    StartSession {
        topic_id: TopicId,
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
        /// Each one must have been given to this daemon with [`Command::AddImage`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<PromptImage>,
    },
    /// Keep an image for a draft or a prompt, and answer with its ID as
    /// [`CommandOutput::ImageAdded`]. Adding the same image again is harmless.
    AddImage {
        /// One of [`IMAGE_TYPES`].
        mime: String,
        /// Base64, at most [`MAX_IMAGE_BYTES`] once decoded.
        data: String,
    },
    /// Answered with [`CommandOutput::Image`].
    ReadImage {
        id: ImageId,
        /// A copy at most [`THUMBNAIL_EDGE`] on its long side, for thumbnails and previews.
        /// An image that's already that small comes as it is.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        thumbnail: bool,
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
    /// Give this device the name clients show. Empty goes back to its hostname.
    RenameMachine {
        name: String,
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
    /// Replace this device's settings.
    SetDeviceSettings {
        settings: DeviceSettings,
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
    /// Mark what a session last finished as seen or not. Only a finished session
    /// can be unread.
    SetUnread {
        session_id: SessionId,
        unread: bool,
    },
    /// Keep what's typed for a session but not sent yet.
    SetDraft {
        session_id: SessionId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<PromptImage>,
    },
    /// Stop the agent and remove its tab. Fails with [`ErrorCode::Busy`] while the agent
    /// is starting, working or waiting, unless `force` says the person has confirmed.
    CloseSession {
        session_id: SessionId,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        force: bool,
    },
    /// Add an MCP server, or change one when `id` names it. Agents that would get a changed
    /// server and aren't working are stopped, so their next prompt starts them with it.
    SaveMcpServer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<McpServerId>,
        name: String,
        /// `None` gives it to every topic on the device. A server can't move between the two.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        topic_id: Option<TopicId>,
        transport: McpTransport,
        enabled: bool,
    },
    /// Remove an MCP server and forget its login.
    RemoveMcpServer {
        id: McpServerId,
    },
    /// Turn a device's MCP server on or off for one topic. `None` follows the server's own
    /// `enabled` again.
    SetTopicMcp {
        topic_id: TopicId,
        server_id: McpServerId,
        on: Option<bool>,
    },
    /// Start logging in to an HTTP MCP server. Open the returned address in a browser. The
    /// login finishes when the browser comes back to the daemon, or with
    /// [`Command::CompleteMcpLogin`] from a client that caught the redirect itself.
    LoginMcpServer {
        id: McpServerId,
    },
    /// Finish a login with what the server sent back to its redirect address.
    CompleteMcpLogin {
        state: String,
        code: String,
    },
    /// Forget the tokens of an HTTP MCP server.
    LogoutMcpServer {
        id: McpServerId,
    },
    /// Copy the MCP servers configured for Claude Code, Codex and OpenCode on this device into
    /// its own servers. A name that's already taken is skipped.
    ImportMcpServers,
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
    /// Archiving would lose this, so it waits for `force`.
    ArchiveRisk {
        risk: ArchiveRisk,
    },
    /// Newest first.
    ArchivedTopics {
        topics: Vec<ArchivedTopic>,
    },
    McpServer {
        server: McpServer,
    },
    /// Where the browser has to go to log in.
    McpLogin {
        url: String,
    },
    McpImport {
        /// The names of the servers added.
        added: Vec<String>,
        /// Why each server that wasn't added was left out.
        skipped: Vec<String>,
    },
    ImageAdded {
        id: ImageId,
    },
    Image {
        /// Base64.
        data: String,
        /// A thumbnail can be a different type from its image.
        mime: String,
    },
    Done,
    #[serde(other)]
    Unknown,
}

/// Where [`Command::MoveTopic`] takes a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MoveTarget {
    /// A fresh worktree on a new branch, starting from the commit the topic is on now.
    NewWorktree,
    /// The repo's own folder, on whatever it has checked out.
    Root,
    /// A worktree the topic was in before.
    Worktree { worktree_id: WorktreeId },
    #[serde(other)]
    Unknown,
}

/// What archiving a topic would lose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRisk {
    /// Agents starting, working or waiting, which archiving stops.
    pub running: u32,
    /// Only worktrees with something to lose.
    pub worktrees: Vec<WorktreeRisk>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRisk {
    pub path: String,
    /// The branch tectr made for this worktree, which archiving deletes.
    pub branch: String,
    /// Changed and untracked files, ignored ones aside.
    pub uncommitted: u32,
    /// Commits on the branch that no other branch or remote has.
    pub unpushed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedTopic {
    pub id: TopicId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub archived_at_ms: i64,
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

/// What a device does, kept by its daemon so that every client sees the same. A field a
/// message leaves out keeps its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSettings {
    /// Stop the machine sleeping while it's idle and an agent is starting or working, so
    /// a locked screen doesn't pause a turn. Closing a laptop's lid still sleeps.
    pub keep_awake: bool,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self { keep_awake: true }
    }
}

impl DeviceSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
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
    #[serde(default, skip_serializing_if = "DeviceSettings::is_default")]
    pub settings: DeviceSettings,
    /// What's typed for each session but not sent yet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drafts: BTreeMap<SessionId, String>,
    /// The images in each session's draft, kept apart from `drafts` so its format stays
    /// what older clients read.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub draft_images: BTreeMap<SessionId, Vec<PromptImage>>,
    /// The slash commands each session's agent last said it has. Kept apart from `Session`,
    /// because a long list would otherwise travel with every status change.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<SessionId, Vec<AgentCommand>>,
    /// The MCP servers agents on this device get, device-wide and per topic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServer>,
}

impl DaemonState {
    /// Applies a live change. Changes are idempotent, so applying one twice
    /// has no further effect.
    pub fn apply(&mut self, change: &StateChange) {
        match change {
            StateChange::MachineRenamed { name } => {
                self.machine.name = name.clone();
            }
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
                if let Some(topic) = self.topic(topic_id) {
                    let gone: Vec<SessionId> =
                        topic.sessions.iter().map(|s| s.id.clone()).collect();
                    for session_id in gone {
                        self.commands.remove(&session_id);
                    }
                }
                for workspace in &mut self.workspaces {
                    workspace.topics.retain(|t| &t.id != topic_id);
                }
            }
            StateChange::WorktreesChanged {
                topic_id,
                worktree_id,
                worktrees,
            } => {
                if let Some(topic) = self.topic_mut(topic_id) {
                    topic.worktree_id = worktree_id.clone();
                    topic.worktrees = worktrees.clone();
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
            StateChange::DeviceSettingsChanged { settings } => {
                self.settings = settings.clone();
            }
            StateChange::SessionRemoved { session_id } => {
                self.commands.remove(session_id);
                for topic in self.workspaces.iter_mut().flat_map(|w| &mut w.topics) {
                    topic.sessions.retain(|s| &s.id != session_id);
                }
            }
            StateChange::DraftChanged {
                session_id,
                text,
                images,
            } => {
                if text.is_empty() {
                    self.drafts.remove(session_id);
                } else {
                    self.drafts.insert(session_id.clone(), text.clone());
                }
                if images.is_empty() {
                    self.draft_images.remove(session_id);
                } else {
                    self.draft_images.insert(session_id.clone(), images.clone());
                }
            }
            StateChange::CommandsChanged {
                session_id,
                commands,
            } => {
                if commands.is_empty() {
                    self.commands.remove(session_id);
                } else {
                    self.commands.insert(session_id.clone(), commands.clone());
                }
            }
            StateChange::McpServerChanged { server } => {
                upsert(&mut self.mcp_servers, server.clone(), |s| &s.id);
            }
            StateChange::McpServerRemoved { id } => {
                self.mcp_servers.retain(|s| &s.id != id);
                for topic in self.workspaces.iter_mut().flat_map(|w| &mut w.topics) {
                    topic.mcp.remove(id);
                }
            }
            StateChange::TopicMcpChanged { topic_id, mcp } => {
                if let Some(topic) = self.topic_mut(topic_id) {
                    topic.mcp = mcp.clone();
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

    pub fn mcp_server(&self, id: &McpServerId) -> Option<&McpServer> {
        self.mcp_servers.iter().find(|s| &s.id == id)
    }

    /// The MCP servers a topic's agents get: the device's that are on for it, then its own
    /// that are on.
    pub fn topic_mcp_servers<'a>(
        &'a self,
        topic: &'a Topic,
    ) -> impl Iterator<Item = &'a McpServer> {
        self.mcp_servers
            .iter()
            .filter(move |server| match &server.topic_id {
                None => topic.mcp.get(&server.id).copied().unwrap_or(server.enabled),
                Some(owner) => owner == &topic.id && server.enabled,
            })
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

/// A feature or task inside a workspace. All its tabs work in one folder at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    pub id: TopicId,
    pub name: String,
    /// The folder its tabs work in, one of `worktrees`.
    pub worktree_id: WorktreeId,
    /// Every folder it has worked in and still has. One at the repo's own path is the repo.
    pub worktrees: Vec<Worktree>,
    pub sessions: Vec<Session>,
    /// The device's MCP servers this topic turns on or off, against their own `enabled`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp: BTreeMap<McpServerId, bool>,
}

impl Topic {
    /// The folder its tabs work in.
    pub fn worktree(&self) -> Option<&Worktree> {
        self.worktrees.iter().find(|w| w.id == self.worktree_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub path: String,
    /// What it has checked out now, which an agent can change. `HEAD` when that's no branch.
    pub branch: String,
}

/// One agent run, in its topic's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub agent: String,
    pub mode: PermissionMode,
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub status: SessionStatus,
    pub title: Option<String>,
    /// A turn finished and nobody has looked at it yet. It's about the person, not a
    /// client, so reading it anywhere reads it everywhere. Starting work clears it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unread: bool,
}

/// A slash command an agent offers. Sending `/name`, and anything after it, as a prompt runs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCommand {
    pub name: String,
    pub description: String,
    /// What to type after it, for a command that takes input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// Agents' slash commands that change what tectr sets itself, with what that is. Running one
/// would leave tectr showing settings the agent no longer has, so the daemon leaves them out of
/// the commands it lists and refuses a prompt that runs one. Names, not agents, because the same
/// name means the same thing in each agent that has it. `apps/dsk/src/slashCommands.ts` lists
/// the same by hand, which a test checks.
pub const TECTR_OWNED_COMMANDS: &[(&str, &str)] = &[
    ("model", "the model"),
    ("effort", "the effort"),
    ("permissions", "the permission mode"),
    ("approvals", "the permission mode"),
    ("config", "the model and permission mode"),
    ("mcp", "the MCP servers"),
];

/// What tectr sets that the command named `name` would change, if it's one of those.
pub fn tectr_owned_command(name: &str) -> Option<&'static str> {
    TECTR_OWNED_COMMANDS
        .iter()
        .find(|(owned, _)| *owned == name)
        .map(|(_, what)| *what)
}

/// Why a prompt can't be sent, when it runs a command that changes what tectr sets.
pub fn refused_prompt(prompt: &str) -> Option<String> {
    let name = prompt
        .trim_start()
        .strip_prefix('/')?
        .split_whitespace()
        .next()?;
    let what = tectr_owned_command(name)?;
    Some(format!(
        "tectr sets {what}, so /{name} can't run here. Change that in tectr instead."
    ))
}

/// An MCP server that tectr gives agents. Each agent's own MCP configuration is switched off,
/// so these are the only ones it gets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub id: McpServerId,
    /// What agents call it. Letters, digits, `-` and `_`, and unique among the servers a topic
    /// can get.
    pub name: String,
    /// The topic that has this server to itself. `None` is a device-wide server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<TopicId>,
    pub transport: McpTransport,
    /// For a device-wide server, whether topics get it unless they turn it off. A topic's own
    /// server is on while this is.
    pub enabled: bool,
    /// Whether the daemon holds a login for it. Only HTTP servers log in, and the tokens never
    /// leave the daemon.
    #[serde(default, skip_serializing_if = "McpAuth::is_none")]
    pub auth: McpAuth,
}

/// How agents reach an MCP server. Values may name the daemon's environment as `${NAME}`,
/// which keeps secrets out of tectr's state: every client of this daemon sees these values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// A program the agent starts.
    Stdio {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
    },
    /// A streamable HTTP server. Agents reach it through the daemon, which adds the login.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        /// For a server that doesn't register clients by itself, the client ID registered
        /// with it for tectr.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oauth_client_id: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAuth {
    /// No login is known to be needed.
    #[default]
    None,
    /// The server turned the daemon away, so someone has to log in.
    NeedsLogin,
    LoggedIn,
    #[serde(other)]
    Unknown,
}

impl McpAuth {
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
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
    MachineRenamed {
        name: String,
    },
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
    /// It was archived, along with its tabs. Any open approvals and drafts of its sessions are
    /// reported as resolved and emptied before this. Restoring it adds it again.
    TopicRemoved {
        topic_id: TopicId,
    },
    /// It moved to another folder, gained or lost one, or a folder's branch changed.
    WorktreesChanged {
        topic_id: TopicId,
        worktree_id: WorktreeId,
        worktrees: Vec<Worktree>,
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
    DeviceSettingsChanged {
        settings: DeviceSettings,
    },
    /// Its tab was closed and its agent stopped.
    SessionRemoved {
        session_id: SessionId,
    },
    /// No text and no images means there's no draft any more.
    DraftChanged {
        session_id: SessionId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<PromptImage>,
    },
    /// The session's agent has a new list of slash commands. Empty means it has none. Closing
    /// or archiving drops a session's list without this.
    CommandsChanged {
        session_id: SessionId,
        commands: Vec<AgentCommand>,
    },
    McpServerChanged {
        server: McpServer,
    },
    /// Topics that turned it on or off forget that too.
    McpServerRemoved {
        id: McpServerId,
    },
    /// Replaces which of the device's MCP servers the topic turns on or off.
    TopicMcpChanged {
        topic_id: TopicId,
        mcp: BTreeMap<McpServerId, bool>,
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

/// The image types agents take.
pub const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];

/// The long side of the copy [`Command::ReadImage`] gives for a thumbnail. Previews show at
/// most 320 points, and screens have two pixels to a point.
pub const THUMBNAIL_EDGE: u32 = 640;

/// The most an image can be, decoded. Model APIs refuse much bigger ones, and it keeps a
/// prompt well inside a websocket frame.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// An image in a prompt or a draft. Its bytes stay with the daemon, so events and drafts stay
/// small.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptImage {
    pub id: ImageId,
    /// What the text calls it: `[Image #3]` is number 3. Numbers count up through a session,
    /// so a number means the same image for the whole conversation.
    pub number: u32,
    pub mime: String,
    pub width: u32,
    pub height: u32,
}

impl PromptImage {
    /// How the text refers to it.
    pub fn token(&self) -> String {
        format!("[Image #{}]", self.number)
    }
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
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<PromptImage>,
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
    fn source_builds_never_share_the_installed_daemon() {
        assert_eq!(Channel::current(), Channel::Dev);
        assert_ne!(Channel::Dev.default_port(), Channel::Release.default_port());
        assert_ne!(Channel::Dev.dir_name(), Channel::Release.dir_name());
        assert_eq!(local_daemon_url(), "ws://127.0.0.1:7769");
    }

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
    fn a_topic_gets_the_device_servers_it_leaves_on_and_its_own() {
        let server = |id: &str, topic_id: Option<&str>, enabled| McpServer {
            id: id.into(),
            name: id.to_owned(),
            topic_id: topic_id.map(TopicId::from),
            transport: McpTransport::Stdio {
                command: "x".into(),
                args: vec![],
                env: BTreeMap::new(),
            },
            enabled,
            auth: McpAuth::None,
        };
        let topic = |id: &str, mcp: &[(&str, bool)]| Topic {
            id: id.into(),
            name: id.to_owned(),
            worktree_id: "wt".into(),
            worktrees: vec![],
            sessions: vec![],
            mcp: mcp
                .iter()
                .map(|(id, on)| (McpServerId::from(*id), *on))
                .collect(),
        };
        let state = DaemonState {
            machine: Machine {
                id: "m".into(),
                name: "m".into(),
            },
            workspaces: vec![],
            approvals: vec![],
            providers: vec![],
            drafts: BTreeMap::new(),
            draft_images: BTreeMap::new(),
            commands: BTreeMap::new(),
            settings: DeviceSettings::default(),
            mcp_servers: vec![
                server("on", None, true),
                server("off", None, false),
                server("mine", Some("t"), true),
                server("mine-off", Some("t"), false),
                server("theirs", Some("other"), true),
            ],
        };
        let names = |topic: &Topic| -> Vec<String> {
            state
                .topic_mcp_servers(topic)
                .map(|s| s.name.clone())
                .collect()
        };
        assert_eq!(names(&topic("t", &[])), ["on", "mine"]);
        assert_eq!(
            names(&topic("t", &[("on", false), ("off", true)])),
            ["off", "mine"]
        );
        assert_eq!(names(&topic("other", &[])), ["on", "theirs"]);
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
            draft_images: BTreeMap::new(),
            commands: BTreeMap::new(),
            settings: DeviceSettings::default(),
            mcp_servers: vec![],
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

    #[test]
    fn a_prompt_running_a_command_tectr_owns_is_refused() {
        assert_eq!(
            refused_prompt("  /model opus").as_deref(),
            Some("tectr sets the model, so /model can't run here. Change that in tectr instead.")
        );
        assert!(refused_prompt("/effort").is_some());
        assert!(refused_prompt("/usage").is_none());
        assert!(refused_prompt("/models").is_none());
        assert!(refused_prompt("what /model am I on").is_none());
        assert!(refused_prompt("/").is_none());
    }

    /// The app checks before it starts an agent, so a refused first prompt doesn't leave a
    /// tab behind.
    #[test]
    fn the_app_refuses_the_same_commands() {
        let source = include_str!("../../../apps/dsk/src/slashCommands.ts");
        let block = source
            .split_once("export const TECTR_OWNED_COMMANDS")
            .and_then(|(_, rest)| rest.split_once("};"))
            .map(|(block, _)| block)
            .expect("slashCommands.ts declares TECTR_OWNED_COMMANDS");
        let listed: Vec<(String, String)> = block
            .lines()
            .filter_map(|line| {
                let (name, what) = line.trim().split_once(": \"")?;
                Some((name.to_owned(), what.strip_suffix("\",")?.to_owned()))
            })
            .collect();
        let owned: Vec<(String, String)> = TECTR_OWNED_COMMANDS
            .iter()
            .map(|(name, what)| ((*name).to_owned(), (*what).to_owned()))
            .collect();
        assert_eq!(listed, owned);
    }

    #[test]
    fn a_sessions_commands_go_with_its_tab_or_its_topic() {
        let session = |id: &str| Session {
            id: id.into(),
            agent: "fake".into(),
            mode: PermissionMode::Auto,
            model: None,
            effort: None,
            status: SessionStatus::Ready,
            title: None,
            unread: false,
        };
        let mut state = DaemonState {
            machine: Machine {
                id: "m".into(),
                name: "m".into(),
            },
            workspaces: vec![Workspace {
                id: "w".into(),
                name: "w".into(),
                path: "/w".into(),
                topics: vec![Topic {
                    id: "t".into(),
                    name: "t".into(),
                    worktree_id: "wt".into(),
                    worktrees: vec![],
                    sessions: vec![session("a"), session("b"), session("c")],
                    mcp: BTreeMap::new(),
                }],
            }],
            approvals: vec![],
            providers: vec![],
            drafts: BTreeMap::new(),
            draft_images: BTreeMap::new(),
            commands: BTreeMap::new(),
            settings: DeviceSettings::default(),
            mcp_servers: vec![],
        };
        let usage = vec![AgentCommand {
            name: "usage".into(),
            description: "Show usage".into(),
            hint: None,
        }];
        for id in ["a", "b", "c"] {
            state.apply(&StateChange::CommandsChanged {
                session_id: id.into(),
                commands: usage.clone(),
            });
        }
        state.apply(&StateChange::CommandsChanged {
            session_id: "c".into(),
            commands: vec![],
        });
        assert_eq!(state.commands.len(), 2);

        state.apply(&StateChange::SessionRemoved {
            session_id: "a".into(),
        });
        assert_eq!(
            state.commands.keys().collect::<Vec<_>>(),
            [&SessionId::from("b")]
        );

        state.apply(&StateChange::TopicRemoved {
            topic_id: "t".into(),
        });
        assert!(state.commands.is_empty());
    }
}
