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
//!    number, then [`ServerMessage::SessionReplayed`], then live events.
//! 5. The client sends [`ClientMessage::AttachTerminal`] for each terminal it shows. What
//!    the terminal prints and what's typed into it travel as binary [`frame`]s.
//!
//! Every enum that can grow has an `Unknown` variant, so an older client can
//! skip a message or value it doesn't understand.
//!
//! The websocket runs over loopback, or over a QUIC stream from iroh for a client on
//! another machine. That one speaks [`REMOTE_ALPN`], and a client that isn't paired yet
//! speaks [`PAIRING_ALPN`] once to become paired.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod frame;
pub mod liveness;

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

    /// What agents call the CLI. Dev has its own name, so a release `tectr` earlier on an
    /// agent's PATH can't send it to the wrong daemon.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Release => "tectr",
            Self::Dev => "tectr-dev",
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

/// Over iroh, a paired client speaks this protocol on one bidirectional stream.
pub const REMOTE_ALPN: &[u8] = b"tectr/1";
/// Over iroh, a client that isn't paired yet speaks this to become paired: it sends one
/// [`PairRequest`] and finishes, and the daemon answers with [`PairResponse`] lines, a
/// [`PairResponse::Confirm`] and then the outcome, or only a refusal.
pub const PAIRING_ALPN: &[u8] = b"tectr-pair/2";
/// The QUIC close code of a daemon that refuses a remote client because its machine is only
/// briefly awake, as a sleeping Mac is every so often. It would be gone again in seconds, so
/// showing it as online would only flicker.
pub const CLOSE_ASLEEP: u32 = 2;
const REMOTE_SCHEME: &str = "iroh://";

/// The address of a daemon on another machine, reached over iroh by its public key.
pub fn remote_daemon_url(daemon_key: &str) -> String {
    format!("{REMOTE_SCHEME}{daemon_key}")
}

/// The daemon's public key, if this is the address of a daemon reached over iroh.
pub fn remote_daemon_key(url: &str) -> Option<&str> {
    url.strip_prefix(REMOTE_SCHEME)
        .filter(|key| !key.is_empty())
}

/// What a daemon shows to pair a new client: its public key, which is also where to
/// reach it, and a secret that only works for a few minutes. The secret only lets a client
/// ask: someone at the daemon still types the digits that client shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCode {
    pub daemon_key: String,
    pub secret: String,
}

impl PairingCode {
    const PREFIX: &str = "tectr-";

    /// Accepts the code with whatever space copying it added around it.
    pub fn parse(text: &str) -> Option<Self> {
        let rest = text.trim().strip_prefix(Self::PREFIX)?;
        let (daemon_key, secret) = rest.split_once('-')?;
        let valid =
            |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric());
        (valid(daemon_key) && valid(secret)).then(|| Self {
            daemon_key: daemon_key.to_owned(),
            secret: secret.to_owned(),
        })
    }
}

impl std::fmt::Display for PairingCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}-{}", Self::PREFIX, self.daemon_key, self.secret)
    }
}

/// The one message a client that isn't paired yet sends, over [`PAIRING_ALPN`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub secret: String,
    /// What the daemon lists this client as, such as its machine's name.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum PairResponse {
    /// The secret was right, and the client waits for someone at the daemon to type
    /// `digits`, which it shows. They're picked for this client, so seeing the code isn't
    /// enough to get in.
    Confirm {
        digits: String,
        machine: Machine,
    },
    /// The client's key is on the allowed list now, so it can connect over [`REMOTE_ALPN`].
    Paired {
        machine: Machine,
    },
    Refused {
        reason: String,
    },
    #[serde(other)]
    Unknown,
}

/// A client on another machine that may connect to this daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedClient {
    /// Its public key, which is what it proves when it connects.
    pub key: String,
    pub name: String,
    pub paired_at_ms: i64,
}

/// A client that brought the secret of an open code and waits for its digits to be typed.
/// The digits stay out of the state, so only the client's own screen shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingRequest {
    pub key: String,
    pub name: String,
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
    /// Unique within a machine, like [`SessionId`].
    TerminalId,
    /// Unique within a machine, like [`SessionId`].
    DiffId,
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
    /// Start showing a terminal at this size, which it takes on. A [`frame::Kind::Reset`]
    /// with everything it shows comes first, then what it prints. A terminal that isn't
    /// running starts its shell, where it last was.
    AttachTerminal {
        terminal_id: TerminalId,
        cols: u16,
        rows: u16,
        /// The colors this client shows it in, which programs that ask for them get told.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        colors: Option<TerminalColors>,
    },
    DetachTerminal {
        terminal_id: TerminalId,
    },
    /// The client showing a terminal is now this size. The last one to say wins, and every
    /// client showing it gets a [`frame::Kind::Reset`] at that size.
    ResizeTerminal {
        terminal_id: TerminalId,
        cols: u16,
        rows: u16,
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
    /// Every stored event a [`ClientMessage::Subscribe`] asked for has been sent, so what
    /// follows is live. A client can show a session's history in one piece once this comes,
    /// rather than guess from how events arrive.
    SessionReplayed {
        session_id: SessionId,
        /// The last event sent, or the `after_seq` asked for when there was nothing to send.
        /// A client that has fewer events is hearing about an earlier subscription.
        seq: u64,
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
    /// Keep the machine from idle sleep for this many seconds from now, whatever its agents
    /// do, so it stays reachable. It's a duration and not a time, so the client's clock
    /// doesn't matter. `None` stops early. At most [`MAX_KEEP_AWAKE_SECS`].
    KeepAwakeFor {
        seconds: Option<u64>,
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
    /// Start a shell in the topic's folder, in a new tab.
    OpenTerminal {
        topic_id: TopicId,
        cols: u16,
        rows: u16,
    },
    /// Stop its shell and remove its tab, with what it showed.
    CloseTerminal {
        terminal_id: TerminalId,
    },
    /// Open a tab showing what changed in the topic's folder.
    OpenDiff {
        topic_id: TopicId,
        scope: DiffScope,
    },
    CloseDiff {
        diff_id: DiffId,
    },
    /// Change what a diff tab compares, for every client showing it.
    SetDiffScope {
        diff_id: DiffId,
        scope: DiffScope,
    },
    /// Stage or unstage whole files in a diff's folder. Unstaging a renamed file needs both
    /// its paths.
    StageFiles {
        diff_id: DiffId,
        paths: Vec<String>,
        stage: bool,
    },
    /// Stage or unstage one hunk of a modified file, as [`Command::ReadDiff`] sent it from
    /// the side it's on: not staged to stage it, staged to unstage it. Fails if the file no
    /// longer has it, so an edit in between can't get a different hunk staged.
    StageHunk {
        diff_id: DiffId,
        path: String,
        hunk: Hunk,
        stage: bool,
    },
    /// Throw away what isn't staged in these files. Untracked files are deleted.
    DiscardFiles {
        diff_id: DiffId,
        paths: Vec<String>,
    },
    /// Throw away one hunk that isn't staged, checked like [`Command::StageHunk`].
    DiscardHunk {
        diff_id: DiffId,
        path: String,
        hunk: Hunk,
    },
    /// Commit what's staged in a diff's folder. Amending with an empty message keeps the last
    /// commit's.
    Commit {
        diff_id: DiffId,
        message: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        amend: bool,
    },
    /// Answered with [`CommandOutput::DiffContent`], what the tab shows now. Its revision is
    /// the tab's [`Diff::revision`] by the time the answer arrives.
    ReadDiff {
        diff_id: DiffId,
    },
    /// What a terminal shows as plain text, its last `lines` lines with wrapped lines joined.
    /// Without `lines`, as many as it has rows. A terminal that isn't running starts its shell.
    ReadTerminal {
        terminal_id: TerminalId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
    },
    /// Type into a terminal, for clients that don't show it. A terminal that isn't running
    /// starts its shell first.
    WriteTerminal {
        terminal_id: TerminalId,
        text: String,
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
    /// Ask the added Claude and Codex agents for their connectors, and keep the list of
    /// connector servers in step: new ones are added switched off, gone ones are removed. An
    /// agent that can't be asked keeps the connectors it had.
    RefreshMcpConnectors,
    /// List the conversations Claude Code and Codex keep on this device, newest first, for
    /// agents added here. One already open in a tab is left out.
    ListConversations,
    /// Open a conversation from [`Command::ListConversations`] as a new tab in a topic, with
    /// its history. The tab starts stopped, and its first prompt resumes the agent's own
    /// session, so the agent remembers everything. If the topic's folder isn't where the
    /// conversation was, the agent is told it moved.
    ImportConversation {
        topic_id: TopicId,
        agent: String,
        id: String,
    },
    /// Open a new tab in the same topic that carries on from one of a tab's answers, with the
    /// history up to there and the agent's memory of it, answered with
    /// [`CommandOutput::Session`]. It uses the agent's own way of copying a session, so only
    /// providers with `can_fork` can. The source can be working on a later answer meanwhile.
    ForkSession {
        session_id: SessionId,
        /// The `seq` of the answer's [`SessionEventKind::TurnEnded`]. Without it, the latest
        /// finished answer.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_seq: Option<u64>,
    },
    /// Show a code that lets a client from another machine ask to pair, answered with
    /// [`CommandOutput::Pairing`]. Only a client on the daemon's own machine can ask, so
    /// letting a new machine in needs someone at this one. A new code replaces the last,
    /// which keeps working for a short grace, like every code past its `expires_at_ms`.
    /// Clients already waiting keep waiting.
    StartPairing,
    /// Pair the waiting client that shows `digits`, answered with [`CommandOutput::Paired`].
    /// The others are refused and the code stops working. Only from the daemon's own
    /// machine, like [`Command::StartPairing`].
    ConfirmPairing {
        digits: String,
    },
    /// Stop a code from [`Command::StartPairing`] from working, and refuse every client
    /// waiting on one.
    CancelPairing,
    /// Take a client off the allowed list. If it's connected, it's disconnected.
    RemovePairedClient {
        key: String,
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
    Terminal {
        topic_id: TopicId,
        terminal: Terminal,
    },
    Diff {
        topic_id: TopicId,
        diff: Diff,
    },
    DiffContent {
        content: DiffContent,
    },
    TerminalText {
        text: String,
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
    McpConnectors {
        /// The names of the connectors added and removed.
        added: Vec<String>,
        removed: Vec<String>,
        /// Why an agent's connectors couldn't be listed.
        problems: Vec<String>,
    },
    /// Newest first.
    Conversations {
        conversations: Vec<Conversation>,
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
    Pairing {
        /// The [`PairingCode`] as text.
        code: String,
        expires_at_ms: i64,
    },
    Paired {
        client: PairedClient,
    },
    Done,
    #[serde(other)]
    Unknown,
}

/// A conversation an agent keeps itself, outside tectr, which can be imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    /// The agent that carries it on, such as `claude` or `codex`.
    pub agent: String,
    /// The agent's own ID for it.
    pub id: String,
    pub title: String,
    /// The folder it was in.
    pub cwd: String,
    /// Milliseconds since the Unix epoch.
    pub updated_at_ms: i64,
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

/// The longest [`Command::KeepAwakeFor`] takes. A week covers any trip, and anything longer
/// is more likely a typo than a plan.
pub const MAX_KEEP_AWAKE_SECS: u64 = 7 * 24 * 60 * 60;

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
    /// Until when the machine stays awake because someone asked with
    /// [`Command::KeepAwakeFor`], in milliseconds since the Unix epoch on the daemon's clock.
    /// The daemon clears it once it has passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awake_until_ms: Option<i64>,
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
    /// The clients on other machines that may connect, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paired_clients: Vec<PairedClient>,
    /// Clients waiting for their digits to be typed, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairing_requests: Vec<PairingRequest>,
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
            StateChange::AwakeUntilChanged { until_ms } => {
                self.awake_until_ms = *until_ms;
            }
            StateChange::TerminalAdded { topic_id, terminal } => {
                if let Some(topic) = self.topic_mut(topic_id) {
                    upsert(&mut topic.terminals, terminal.clone(), |t| &t.id);
                }
            }
            StateChange::TerminalChanged { terminal } => {
                if let Some(existing) = self
                    .workspaces
                    .iter_mut()
                    .flat_map(|w| &mut w.topics)
                    .flat_map(|t| &mut t.terminals)
                    .find(|t| t.id == terminal.id)
                {
                    *existing = terminal.clone();
                }
            }
            StateChange::TerminalRemoved { terminal_id } => {
                for topic in self.workspaces.iter_mut().flat_map(|w| &mut w.topics) {
                    topic.terminals.retain(|t| &t.id != terminal_id);
                }
            }
            StateChange::DiffAdded { topic_id, diff } => {
                if let Some(topic) = self.topic_mut(topic_id) {
                    upsert(&mut topic.diffs, diff.clone(), |d| &d.id);
                }
            }
            StateChange::DiffChanged { diff } => {
                if let Some(existing) = self.diff_mut(&diff.id) {
                    *existing = diff.clone();
                }
            }
            StateChange::DiffRemoved { diff_id } => {
                for topic in self.workspaces.iter_mut().flat_map(|w| &mut w.topics) {
                    topic.diffs.retain(|d| &d.id != diff_id);
                }
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
            StateChange::ClientPaired { client } => {
                upsert(&mut self.paired_clients, client.clone(), |c| &c.key);
            }
            StateChange::PairedClientRemoved { key } => {
                self.paired_clients.retain(|c| &c.key != key);
            }
            StateChange::PairingRequestsChanged { requests } => {
                self.pairing_requests = requests.clone();
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

    pub fn terminal(&self, id: &TerminalId) -> Option<&Terminal> {
        self.topics()
            .flat_map(|t| &t.terminals)
            .find(|t| &t.id == id)
    }

    /// A diff tab and the topic it's in.
    pub fn diff(&self, id: &DiffId) -> Option<(&Topic, &Diff)> {
        self.topics()
            .find_map(|t| t.diffs.iter().find(|d| &d.id == id).map(|d| (t, d)))
    }

    pub fn diff_mut(&mut self, id: &DiffId) -> Option<&mut Diff> {
        self.workspaces
            .iter_mut()
            .flat_map(|w| &mut w.topics)
            .flat_map(|t| &mut t.diffs)
            .find(|d| &d.id == id)
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
    /// Its shells. Tabs go in the order they were opened, agents, terminals and diffs together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<Terminal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<Diff>,
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
    /// Milliseconds since the Unix epoch, which puts its tab in order.
    #[serde(default)]
    pub opened_at_ms: i64,
}

/// A shell in its topic's folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Terminal {
    pub id: TerminalId,
    /// What the program in it last called it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub status: TerminalStatus,
    /// Milliseconds since the Unix epoch, which puts its tab in order.
    pub opened_at_ms: i64,
}

/// A tab showing what changed in its topic's folder. It follows the topic when it moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub id: DiffId,
    pub scope: DiffScope,
    /// Milliseconds since the Unix epoch, which puts its tab in order.
    pub opened_at_ms: i64,
    /// Changes whenever what it shows does. Read that with [`Command::ReadDiff`]. Zero until
    /// the daemon has looked since it started.
    #[serde(default)]
    pub revision: u64,
}

/// What a diff compares.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffScope {
    /// The folder against its last commit, with what's staged apart from what isn't.
    Uncommitted,
    /// Everything since the branch left its base, committed or not.
    Branch,
    #[serde(other)]
    Unknown,
}

/// What a diff tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffContent {
    pub revision: u64,
    /// What the branch scope compares with, like `main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// The last commit's message, which amending starts from. Only in the uncommitted scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_message: Option<String>,
    /// In the uncommitted scope, the staged files come before the rest. A file can be in both.
    pub files: Vec<FileDiff>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// Where it is now, or was for a deleted file.
    pub path: String,
    /// Where a renamed or copied file came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// In the uncommitted scope, this is what's staged rather than what isn't.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub staged: bool,
    pub added: u32,
    pub removed: u32,
    pub body: FileBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    /// New and not yet known to git.
    Untracked,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FileBody {
    /// No hunks means only its mode changed, or it's empty.
    Text {
        hunks: Vec<Hunk>,
    },
    Binary,
    /// Too big to send. Its counts are still right, except for an untracked file, which has
    /// none.
    TooLarge,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    /// What git says the hunk is in, such as the function around it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    /// As in a unified diff: each starts with ` `, `+` or `-`. One starting with `\` says the
    /// line before it ends its file without a newline.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Running,
    /// No shell since the daemon started. Showing it starts one where the last one was,
    /// below what that one left.
    Stopped,
    /// Its shell failed. Typing starts another. One that exits cleanly closes its tab.
    Exited,
    #[serde(other)]
    Unknown,
}

/// A client's colors, as `#rrggbb`. Programs ask a terminal for these to tell dark from light.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalColors {
    pub foreground: String,
    pub background: String,
    pub cursor: String,
    /// The 16 ANSI colors, black to bright white.
    pub palette: Vec<String>,
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
/// so these are the only ones it gets, along with the connectors turned on here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub id: McpServerId,
    /// What agents call it. Letters, digits, `-` and `_`, and unique among the servers a topic
    /// can get. A connector has its agent's name for it instead, like `claude.ai Slack`.
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
    /// A claude.ai connector, which only Claude agents get. Anthropic hosts the login, and
    /// Claude Code connects to it by itself. Always device-wide, and added by
    /// [`Command::RefreshMcpConnectors`] rather than by hand.
    ClaudeConnector {
        /// The service's own address, which is how Claude Code tells connectors apart.
        url: String,
    },
    /// A ChatGPT app, which only Codex agents get. OpenAI hosts the login. Always device-wide,
    /// and added by [`Command::RefreshMcpConnectors`] rather than by hand.
    CodexApp { id: String },
    #[serde(other)]
    Unknown,
}

impl McpTransport {
    pub fn is_connector(&self) -> bool {
        matches!(self, Self::ClaudeConnector { .. } | Self::CodexApp { .. })
    }
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
    /// Whether the agent can copy one of its sessions, which [`Command::ForkSession`] needs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_fork: bool,
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
    AwakeUntilChanged {
        until_ms: Option<i64>,
    },
    TerminalAdded {
        topic_id: TopicId,
        terminal: Terminal,
    },
    TerminalChanged {
        terminal: Terminal,
    },
    /// Its tab was closed, or its shell exited cleanly.
    TerminalRemoved {
        terminal_id: TerminalId,
    },
    DiffAdded {
        topic_id: TopicId,
        diff: Diff,
    },
    /// Its scope changed, or what it shows did.
    DiffChanged {
        diff: Diff,
    },
    DiffRemoved {
        diff_id: DiffId,
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
    ClientPaired {
        client: PairedClient,
    },
    /// The daemon has also closed its connections.
    PairedClientRemoved {
        key: String,
    },
    /// Replaces the clients waiting for their digits to be typed.
    PairingRequestsChanged {
        requests: Vec<PairingRequest>,
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
        /// The agent's ID for the last thing it said this turn, which is where a branch from
        /// this answer starts. Missing when the agent doesn't report one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_message: Option<String>,
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
    #[test]
    fn a_pairing_code_survives_being_copied() {
        let code = PairingCode {
            daemon_key: "abc123".into(),
            secret: "f00d".into(),
        };
        let text = code.to_string();
        assert_eq!(text, "tectr-abc123-f00d");
        assert_eq!(PairingCode::parse(&format!("  {text}\n")), Some(code));
        for broken in [
            "",
            "tectr-",
            "tectr-abc",
            "tectr-abc-",
            "abc-f00d",
            "tectr-a b-c",
            "tectr-a-b-c",
        ] {
            assert_eq!(PairingCode::parse(broken), None, "{broken:?}");
        }
    }

    #[test]
    fn remote_urls_carry_the_daemon_key() {
        let url = remote_daemon_url("abc");
        assert_eq!(remote_daemon_key(&url), Some("abc"));
        assert_eq!(remote_daemon_key("iroh://"), None);
        assert_eq!(remote_daemon_key("ws://127.0.0.1:7769"), None);
    }

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
            terminals: vec![],
            diffs: vec![],
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
            awake_until_ms: None,
            mcp_servers: vec![
                server("on", None, true),
                server("off", None, false),
                server("mine", Some("t"), true),
                server("mine-off", Some("t"), false),
                server("theirs", Some("other"), true),
            ],
            paired_clients: vec![],
            pairing_requests: vec![],
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
            awake_until_ms: None,
            mcp_servers: vec![],
            paired_clients: vec![],
            pairing_requests: vec![],
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
            opened_at_ms: 0,
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
                    terminals: vec![],
                    diffs: vec![],
                    mcp: BTreeMap::new(),
                }],
            }],
            approvals: vec![],
            providers: vec![],
            drafts: BTreeMap::new(),
            draft_images: BTreeMap::new(),
            commands: BTreeMap::new(),
            settings: DeviceSettings::default(),
            awake_until_ms: None,
            mcp_servers: vec![],
            paired_clients: vec![],
            pairing_requests: vec![],
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
