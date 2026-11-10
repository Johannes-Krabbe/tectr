//! The daemon's state and rules. Everything that changes a session goes through
//! here, so its status, stored events, and live updates can't drift apart.
//!
//! A single lock protects the state. It's held only for short, non-async work,
//! which also gives each session's events a single order. Events are stored
//! before they're broadcast, so a client that reads history can't miss one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use protocol::{
    AgentCommand, Approval, ApprovalId, ArchiveRisk, Command, CommandError, CommandOutput,
    DaemonState, DeviceSettings, ErrorCode, Machine, MachineId, MoveTarget, PermissionMode,
    PermissionOption, PermissionOutcome, PromptImage, Provider, ProviderCandidate, Session,
    SessionEvent, SessionEventKind, SessionId, SessionRef, SessionStatus, StateChange, StopReason,
    Terminal, TerminalColors, TerminalId, TerminalStatus, ToolCallPatch, Topic, TopicId, Workspace,
    WorkspaceId, Worktree, WorktreeId, WorktreeRisk,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::agents::AgentLauncher;
use crate::awake::{KeepAwake, KeepAwakeCommand};
use crate::images::{self, Images};
use crate::mcp::ConfigHome;
use crate::remote::Network;
use crate::store::Store;
use crate::terminal::{self, Shell, Subscriber};
use crate::{acp, folders, git};

mod import;
mod mcp;
mod remote;

/// How often image files nothing names are looked for, besides at startup.
const COLLECT_IMAGES_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// A live update for connected clients.
#[derive(Debug, Clone)]
pub enum Signal {
    State(Arc<StateChange>),
    Event(Arc<SessionEvent>),
}

/// What a session runs with. `None` leaves the agent's own value alone.
#[derive(Debug, Clone)]
pub struct SessionSettings {
    pub mode: PermissionMode,
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// What an agent is sent for a prompt.
pub struct Prompt {
    pub text: String,
    /// Each with its bytes in base64.
    pub images: Vec<(PromptImage, String)>,
}

/// What the rest of the daemon can ask a running agent to do.
pub enum SessionCommand {
    Prompt(Prompt),
    Cancel,
    /// Answered with why the agent couldn't change, if it couldn't.
    Configure {
        settings: SessionSettings,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

/// Delivers an answer to the agent. `None` means the request was cancelled.
pub type PermissionAnswer = Box<dyn FnOnce(Option<String>) + Send>;

/// A session whose agent process is running or starting.
struct LiveSession {
    commands: mpsc::UnboundedSender<SessionCommand>,
    /// Set from the moment a prompt is accepted, so a second prompt can't slip
    /// in while the first is still on its way to the agent.
    turn_active: bool,
    pending: HashMap<ApprovalId, PermissionAnswer>,
    task: Option<JoinHandle<()>>,
    /// Lets the agent's requests through the MCP proxy. It dies with the agent.
    proxy_secret: String,
    /// The MCP servers the agent was started with.
    mcp: Vec<protocol::McpServerId>,
    /// What its topic's servers were when it started, to tell when they've changed.
    mcp_fingerprint: String,
    /// Its servers changed while it was busy, so it stops once it's done.
    mcp_stale: bool,
}

/// A terminal whose thread is running. `run` tells its events apart from an earlier one's.
struct LiveTerminal {
    handle: terminal::Handle,
    run: String,
}

struct Inner {
    store: Store,
    state: DaemonState,
    live: HashMap<SessionId, LiveSession>,
    terminals: HashMap<TerminalId, LiveTerminal>,
    /// Terminals told to stop, which shutdown waits for so their history is saved.
    stopping_terminals: Vec<std::thread::JoinHandle<()>>,
    /// Agents told to stop because their tab closed, which shutdown still has to wait for.
    closing: Vec<JoinHandle<()>>,
    last_seq: HashMap<SessionId, u64>,
    signals: broadcast::Sender<Signal>,
    awake: KeepAwake,
}

pub struct Core {
    inner: Mutex<Inner>,
    signals: broadcast::Sender<Signal>,
    data_dir: PathBuf,
    images: Images,
    /// What the machine is called until someone names it.
    hostname: String,
    agents: BTreeMap<String, AgentLauncher>,
    /// Stops two sessions from installing the same adapter at the same time.
    adapter_install: tokio::sync::Mutex<()>,
    /// Held while worktrees are made or removed for archiving, restoring and moving, so that
    /// restoring a topic straight after archiving it can't race the removal of its folder.
    worktree_changes: Arc<tokio::sync::Mutex<()>>,
    shutting_down: AtomicBool,
    /// The port the daemon listens on, which the MCP proxy and login redirects use.
    port: u16,
    /// Where agents keep the MCP configuration that gets switched off, and their conversations.
    config_home: ConfigHome,
    /// For MCP servers and their logins.
    http: reqwest::Client,
    logins: Mutex<HashMap<String, mcp::PendingLogin>>,
    /// Refreshing one login at a time keeps two requests from using up one refresh token.
    token_refresh: tokio::sync::Mutex<()>,
    /// What starts in a new terminal.
    shell: Shell,
    /// Tells apart the clients showing a terminal.
    next_subscriber: AtomicU64,
    /// What this daemon proves to remote clients, and what they dial.
    remote_key: iroh::SecretKey,
    network: Network,
    /// Set once something needs remote clients: a pairing, or a client already paired.
    endpoint: tokio::sync::OnceCell<iroh::Endpoint>,
    pairing: Mutex<Option<remote::Pairing>>,
}

/// What `Core::open` needs besides the data directory.
pub struct CoreOptions {
    pub hostname: String,
    pub agents: BTreeMap<String, AgentLauncher>,
    pub event_buffer: usize,
    pub keep_awake: Option<KeepAwakeCommand>,
    pub port: u16,
    pub config_home: ConfigHome,
    pub shell: Shell,
    pub network: Network,
}

impl Core {
    pub fn open(data_dir: &Path, options: CoreOptions) -> Result<Arc<Self>> {
        let CoreOptions {
            hostname,
            agents,
            event_buffer,
            keep_awake,
            port,
            config_home,
            shell,
            network,
        } = options;
        let store = Store::open(&data_dir.join("tectr.db"))?;
        let remote_key = store.remote_key(|| {
            let bytes = iroh::SecretKey::generate().to_bytes();
            bytes.iter().map(|byte| format!("{byte:02x}")).collect()
        })?;
        let remote_key: iroh::SecretKey = remote_key
            .parse()
            .context("the stored remote key is damaged")?;
        let machine = Machine {
            id: store.machine_id()?,
            name: store.machine_name()?.unwrap_or_else(|| hostname.clone()),
        };
        let state = store.load_state(machine)?;
        let last_seq = store.last_seqs()?.into_iter().collect();
        let (signals, _) = broadcast::channel(event_buffer);
        let mut inner = Inner {
            store,
            state,
            live: HashMap::new(),
            terminals: HashMap::new(),
            stopping_terminals: Vec::new(),
            closing: Vec::new(),
            last_seq,
            signals: signals.clone(),
            awake: KeepAwake::new(keep_awake),
        };
        inner.recover()?;
        Ok(Arc::new(Self {
            inner: Mutex::new(inner),
            signals,
            data_dir: data_dir.to_owned(),
            images: Images::new(data_dir),
            hostname,
            agents,
            adapter_install: tokio::sync::Mutex::new(()),
            worktree_changes: Arc::new(tokio::sync::Mutex::new(())),
            shutting_down: AtomicBool::new(false),
            port,
            config_home,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("tectr/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("failed to set up HTTP for MCP servers")?,
            logins: Mutex::new(HashMap::new()),
            token_refresh: tokio::sync::Mutex::new(()),
            shell,
            next_subscriber: AtomicU64::new(1),
            remote_key,
            network,
            endpoint: tokio::sync::OnceCell::new(),
            pairing: Mutex::new(None),
        }))
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .expect("a panic poisoned the daemon state")
    }

    pub fn machine(&self) -> Machine {
        self.lock().state.machine.clone()
    }

    /// Returns the current state and a subscription that starts exactly after
    /// it, so a client neither misses nor double-applies a change.
    pub fn snapshot_and_subscribe(&self) -> (DaemonState, broadcast::Receiver<Signal>) {
        let inner = self.lock();
        (inner.state.clone(), self.signals.subscribe())
    }

    pub fn events_after(
        &self,
        session_id: &SessionId,
        after_seq: u64,
        limit: usize,
    ) -> Result<Vec<SessionEvent>> {
        let inner = self.lock();
        let machine_id = &inner.state.machine.id;
        inner
            .store
            .events_after(machine_id, session_id, after_seq, limit)
    }

    pub async fn handle(self: &Arc<Self>, command: Command) -> Result<CommandOutput, CommandError> {
        match command {
            Command::AddWorkspace { path } => self.add_workspace(&path).await,
            Command::ListFolders { path } => list_folders(&path).await,
            Command::CreateTopic {
                workspace_id,
                name,
                at_root,
            } => {
                self.create_topic(&workspace_id, name.as_deref(), at_root)
                    .await
            }
            Command::RenameTopic { topic_id, name } => self.rename_topic(&topic_id, &name),
            Command::ArchiveTopic { topic_id, force } => self.archive_topic(&topic_id, force).await,
            Command::RestoreTopic { topic_id } => self.restore_topic(&topic_id).await,
            Command::ListArchivedTopics => {
                let topics = self.lock().store.archived_topics().map_err(internal)?;
                Ok(CommandOutput::ArchivedTopics { topics })
            }
            Command::MoveTopic {
                topic_id,
                to,
                force,
            } => self.move_topic(&topic_id, to, force).await,
            Command::StartSession {
                topic_id,
                agent,
                mode,
                model,
                effort,
            } => self.start_session(topic_id, agent, mode, model, effort),
            Command::SendPrompt {
                session_id,
                text,
                images,
            } => self.send_prompt(&session_id, text, images),
            Command::AddImage { mime, data } => {
                let images = self.images.clone();
                blocking(move || images.add(&mime, &data))
                    .await?
                    .map(|id| CommandOutput::ImageAdded { id })
                    .map_err(|error| invalid(format!("{error:#}")))
            }
            Command::ReadImage { id, thumbnail } => {
                if !self.images.has(&id) {
                    return Err(not_found("image", &id));
                }
                let images = self.images.clone();
                blocking(move || images.read(&id, thumbnail))
                    .await?
                    .map(|image| CommandOutput::Image {
                        data: image.data,
                        mime: image.mime,
                    })
                    .map_err(internal)
            }
            Command::CancelTurn { session_id } => self.cancel_turn(&session_id),
            Command::AnswerApproval {
                approval_id,
                option_id,
            } => self.answer_approval(&approval_id, &option_id),
            Command::RenameMachine { name } => self.rename_machine(&name),
            Command::DetectProviders => Ok(self.detect_providers().await),
            Command::AddProvider { agent } => self.add_provider(&agent).await,
            Command::RemoveProvider { agent } => self.remove_provider(&agent),
            Command::SetProviderDefaults {
                agent,
                model,
                effort,
            } => self.set_provider_defaults(&agent, model, effort),
            Command::SetDeviceSettings { settings } => self.set_device_settings(settings),
            Command::SetSessionSettings {
                session_id,
                mode,
                model,
                effort,
            } => {
                let settings = SessionSettings {
                    mode,
                    model,
                    effort,
                };
                self.set_session_settings(&session_id, settings).await
            }
            Command::SetUnread { session_id, unread } => self.set_unread(&session_id, unread),
            Command::SetDraft {
                session_id,
                text,
                images,
            } => self.set_draft(session_id, text, images),
            Command::CloseSession { session_id, force } => self.close_session(&session_id, force),
            Command::OpenTerminal {
                topic_id,
                cols,
                rows,
            } => self.open_terminal(topic_id, cols, rows),
            Command::CloseTerminal { terminal_id } => self.close_terminal(&terminal_id),
            Command::SaveMcpServer {
                id,
                name,
                topic_id,
                transport,
                enabled,
            } => {
                self.save_mcp_server(id, &name, topic_id, transport, enabled)
                    .await
            }
            Command::RemoveMcpServer { id } => self.remove_mcp_server(&id),
            Command::SetTopicMcp {
                topic_id,
                server_id,
                on,
            } => self.set_topic_mcp(&topic_id, &server_id, on),
            Command::LoginMcpServer { id } => self.login_mcp_server(&id).await,
            Command::CompleteMcpLogin { state, code } => self
                .complete_mcp_login(&state, &code)
                .await
                .map(|_| CommandOutput::Done),
            Command::LogoutMcpServer { id } => self.logout_mcp_server(&id),
            Command::ImportMcpServers => self.import_mcp_servers().await,
            Command::ListConversations => self.list_conversations().await,
            Command::ImportConversation {
                topic_id,
                agent,
                id,
            } => self.import_conversation(topic_id, agent, id).await,
            Command::StartPairing => self.start_pairing().await,
            Command::CancelPairing => Ok(self.cancel_pairing()),
            Command::RemovePairedClient { key } => self.remove_paired_client(&key),
            Command::Unknown => Err(invalid("this daemon doesn't know that command")),
        }
    }

    async fn add_workspace(&self, path: &str) -> Result<CommandOutput, CommandError> {
        let path = resolve_path(path)?;
        let root = git::repo_root(&path).await.map_err(|error| {
            invalid(format!(
                "{} isn't a git repository: {error}",
                path.display()
            ))
        })?;
        let root_text = root.display().to_string();

        let mut inner = self.lock();
        if let Some(existing) = inner.state.workspaces.iter().find(|w| w.path == root_text) {
            return Ok(CommandOutput::Workspace {
                workspace: existing.clone(),
            });
        }
        let workspace = Workspace {
            id: WorkspaceId(crate::new_id()),
            name: root.file_name().map_or_else(
                || root_text.clone(),
                |name| name.to_string_lossy().into_owned(),
            ),
            path: root_text,
            topics: vec![],
        };
        inner.store.insert_workspace(&workspace).map_err(internal)?;
        inner.emit(StateChange::WorkspaceAdded {
            workspace: workspace.clone(),
        });
        Ok(CommandOutput::Workspace { workspace })
    }

    async fn create_topic(
        &self,
        workspace_id: &WorkspaceId,
        name: Option<&str>,
        at_root: bool,
    ) -> Result<CommandOutput, CommandError> {
        let auto_named = name.is_none();
        let (repo, name) = {
            let inner = self.lock();
            let workspace = inner
                .state
                .workspace(workspace_id)
                .ok_or_else(|| not_found("workspace", workspace_id))?;
            let name = match name.map(str::trim) {
                Some("") => return Err(invalid("a topic needs a name")),
                Some(name) => name.to_owned(),
                // Counting up from the length, because deleting can leave that number taken.
                None => (workspace.topics.len() + 1..)
                    .map(|number| format!("Topic #{number}"))
                    .find(|name| !workspace.topics.iter().any(|t| &t.name == name))
                    .expect("there are always more numbers than topics"),
            };
            (PathBuf::from(&workspace.path), name)
        };

        let topic_id = crate::new_id();
        let (path, branch) = if at_root {
            let branch = git::current_branch(&repo).await.map_err(internal)?;
            (repo, branch)
        } else {
            let unique = format!("{}-{}", git::slug(&name), &topic_id[..6]);
            let branch = format!("tectr/{unique}");
            let path = self
                .data_dir
                .join("worktrees")
                .join(&workspace_id.0)
                .join(&unique);
            git::add_worktree(&repo, &path, &branch, "HEAD")
                .await
                .map_err(internal)?;
            // macOS temp directories are symlinks. Agents should see the real path.
            let path = tokio::fs::canonicalize(&path)
                .await
                .map_err(|e| internal(e.into()))?;
            (path, branch)
        };

        let worktree = Worktree {
            id: crate::new_id().as_str().into(),
            path: path.display().to_string(),
            branch,
        };
        let topic = Topic {
            id: topic_id.as_str().into(),
            name,
            worktree_id: worktree.id.clone(),
            worktrees: vec![worktree],
            sessions: vec![],
            terminals: vec![],
            mcp: BTreeMap::new(),
        };
        let mut inner = self.lock();
        inner
            .store
            .insert_topic(workspace_id, &topic, auto_named)
            .map_err(internal)?;
        inner.emit(StateChange::TopicAdded {
            workspace_id: workspace_id.clone(),
            topic: topic.clone(),
        });
        Ok(CommandOutput::Topic {
            workspace_id: workspace_id.clone(),
            topic,
        })
    }

    fn rename_machine(&self, name: &str) -> Result<CommandOutput, CommandError> {
        let chosen = Some(name.trim()).filter(|name| !name.is_empty());
        let mut inner = self.lock();
        inner.store.set_machine_name(chosen).map_err(internal)?;
        inner.emit(StateChange::MachineRenamed {
            name: chosen.unwrap_or(&self.hostname).to_owned(),
        });
        Ok(CommandOutput::Done)
    }

    fn set_device_settings(&self, settings: DeviceSettings) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        inner
            .store
            .set_device_settings(&settings)
            .map_err(internal)?;
        inner.emit(StateChange::DeviceSettingsChanged { settings });
        Ok(CommandOutput::Done)
    }

    fn rename_topic(
        &self,
        topic_id: &protocol::TopicId,
        name: &str,
    ) -> Result<CommandOutput, CommandError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(invalid("a topic needs a name"));
        }
        let mut inner = self.lock();
        if inner.state.topic(topic_id).is_none() {
            return Err(not_found("topic", topic_id));
        }
        inner.store.rename_topic(topic_id, name).map_err(internal)?;
        inner.emit(StateChange::TopicRenamed {
            topic_id: topic_id.clone(),
            name: name.to_owned(),
        });
        Ok(CommandOutput::Done)
    }

    /// Without `force`, git is asked first what would be lost, and anything at all leaves the
    /// topic as it is. Otherwise the topic goes straight away and its worktrees behind that,
    /// because agents can take seconds to exit and have to be gone first.
    async fn archive_topic(
        &self,
        topic_id: &TopicId,
        force: bool,
    ) -> Result<CommandOutput, CommandError> {
        let (repo, owned, running) = {
            let inner = self.lock();
            let (_, repo, topic) = inner.topic_with_repo(topic_id)?;
            let made = inner.store.made_branches(topic_id).map_err(internal)?;
            let running = topic
                .sessions
                .iter()
                .filter(|s| inner.is_running(s))
                .count();
            (repo.clone(), owned_worktrees(&repo, &topic, &made), running)
        };
        if !force {
            let mut worktrees = vec![];
            for (path, branch) in &owned {
                let uncommitted = git::uncommitted(path).await.map_err(internal)?;
                let unpushed = git::unpushed(&repo, branch).await.map_err(internal)?;
                if uncommitted > 0 || unpushed > 0 {
                    worktrees.push(WorktreeRisk {
                        path: path.display().to_string(),
                        branch: branch.clone(),
                        uncommitted,
                        unpushed,
                    });
                }
            }
            if running > 0 || !worktrees.is_empty() {
                let running = running as u32;
                let risk = ArchiveRisk { running, worktrees };
                return Ok(CommandOutput::ArchiveRisk { risk });
            }
        }

        let changing = self.worktree_changes.clone().lock_owned().await;
        let mut inner = self.lock();
        let (_, _, topic) = inner.topic_with_repo(topic_id)?;
        inner
            .store
            .archive_topic(topic_id, now_ms())
            .map_err(internal)?;

        let mut agents = vec![];
        for session in &topic.sessions {
            let running = inner.is_running(session);
            agents.extend(inner.stop_agent(&session.id));
            if running {
                inner.set_status(&session.id, SessionStatus::Stopped);
            }
            // The topic is already archived in the database, so this can't turn back now.
            if let Err(error) = inner.clear_draft(&session.id) {
                tracing::error!(id = %session.id, "failed to clear a draft: {error:#}");
            }
        }
        // Their shells end with the topic, and what they showed comes back with it.
        for terminal in &topic.terminals {
            inner.stop_terminal(&terminal.id, true);
        }
        inner.emit(StateChange::TopicRemoved {
            topic_id: topic_id.clone(),
        });
        let cleanup = tokio::spawn(async move {
            discard_worktrees(repo, owned, agents).await;
            drop(changing);
        });
        inner.keep_closing(cleanup);
        Ok(CommandOutput::Done)
    }

    /// Makes the topic's folder again at the same path, so its agents' own history still
    /// matches it when they resume.
    async fn restore_topic(&self, topic_id: &TopicId) -> Result<CommandOutput, CommandError> {
        let _changing = self.worktree_changes.lock().await;
        let (workspace_id, mut topic, repo) = {
            let inner = self.lock();
            let (workspace_id, topic) = inner
                .store
                .archived_topic(topic_id)
                .map_err(internal)?
                .ok_or_else(|| not_found("archived topic", topic_id))?;
            let repo = inner
                .state
                .workspace(&workspace_id)
                .map(|workspace| PathBuf::from(&workspace.path))
                .ok_or_else(|| not_found("workspace", &workspace_id))?;
            (workspace_id, topic, repo)
        };
        if let Some(worktree) = topic.worktree() {
            let path = Path::new(&worktree.path);
            if path != repo && !path.exists() {
                git::restore_worktree(&repo, path, &worktree.branch)
                    .await
                    .map_err(internal)?;
            }
        }
        for worktree in &mut topic.worktrees {
            if let Ok(branch) = git::current_branch(Path::new(&worktree.path)).await {
                worktree.branch = branch;
            }
        }

        let mut inner = self.lock();
        inner.store.restore_topic(topic_id).map_err(internal)?;
        // Archiving stopped its agents, and one stopped on its way out may not have said so.
        for session in &mut topic.sessions {
            if matches!(
                session.status,
                SessionStatus::Starting | SessionStatus::Working | SessionStatus::WaitingForYou
            ) {
                session.status = SessionStatus::Stopped;
                inner.store.update_session(session).map_err(internal)?;
            }
        }
        let commands = inner.store.topic_commands(topic_id).map_err(internal)?;
        inner.emit(StateChange::TopicAdded {
            workspace_id: workspace_id.clone(),
            topic: topic.clone(),
        });
        for (session_id, commands) in commands {
            inner.emit(StateChange::CommandsChanged {
                session_id,
                commands,
            });
        }
        Ok(CommandOutput::Topic {
            workspace_id,
            topic,
        })
    }

    /// Agents stop here and resume in the new folder on their next prompt, which tells them.
    /// The folder left behind stays, so the topic can go back to it.
    async fn move_topic(
        &self,
        topic_id: &TopicId,
        to: MoveTarget,
        force: bool,
    ) -> Result<CommandOutput, CommandError> {
        let _changing = self.worktree_changes.lock().await;
        let (workspace_id, repo, topic) = {
            let inner = self.lock();
            let found = inner.topic_with_repo(topic_id)?;
            inner.check_idle(&found.2, force)?;
            found
        };
        let current = topic
            .worktree()
            .ok_or_else(|| internal(anyhow::anyhow!("the topic has no folder")))?
            .clone();
        let existing = match &to {
            MoveTarget::NewWorktree => None,
            MoveTarget::Root => topic
                .worktrees
                .iter()
                .find(|w| Path::new(&w.path) == repo)
                .cloned(),
            MoveTarget::Worktree { worktree_id } => Some(
                topic
                    .worktrees
                    .iter()
                    .find(|w| &w.id == worktree_id)
                    .ok_or_else(|| not_found("worktree in that topic", worktree_id))?
                    .clone(),
            ),
            MoveTarget::Unknown => return Err(invalid("this daemon doesn't know that place")),
        };
        if existing.as_ref().is_some_and(|w| w.id == current.id) {
            return Ok(CommandOutput::Done);
        }
        let target = match existing {
            Some(worktree) if Path::new(&worktree.path).exists() => worktree,
            Some(worktree) => {
                return Err(invalid(format!("{} no longer exists", worktree.path)));
            }
            None if matches!(to, MoveTarget::Root) => Worktree {
                id: crate::new_id().as_str().into(),
                path: repo.display().to_string(),
                branch: git::current_branch(&repo).await.map_err(internal)?,
            },
            None => {
                let start = git::head_commit(Path::new(&current.path))
                    .await
                    .map_err(internal)?;
                let id = crate::new_id();
                let (path, branch) = self
                    .add_topic_worktree(&repo, &workspace_id, &topic.name, &id, &start)
                    .await?;
                Worktree {
                    id: id.as_str().into(),
                    path,
                    branch,
                }
            }
        };

        let mut inner = self.lock();
        let (_, _, topic) = inner.topic_with_repo(topic_id)?;
        let mut worktrees = topic.worktrees.clone();
        if !worktrees.iter().any(|w| w.id == target.id) {
            inner
                .store
                .insert_worktree(topic_id, &target)
                .map_err(internal)?;
            worktrees.push(target.clone());
        }
        inner
            .store
            .set_topic_worktree(topic_id, &target.id)
            .map_err(internal)?;
        for session in &topic.sessions {
            let running = inner.is_running(session);
            if let Some(task) = inner.stop_agent(&session.id) {
                inner.keep_closing(task);
            }
            if running {
                inner.set_status(&session.id, SessionStatus::Stopped);
            }
        }
        inner.emit(StateChange::WorktreesChanged {
            topic_id: topic_id.clone(),
            worktree_id: target.id,
            worktrees,
        });
        Ok(CommandOutput::Done)
    }

    /// A new branch from `start` in a new folder in the data directory, named after the topic.
    async fn add_topic_worktree(
        &self,
        repo: &Path,
        workspace_id: &WorkspaceId,
        name: &str,
        id: &str,
        start: &str,
    ) -> Result<(String, String), CommandError> {
        let unique = format!("{}-{}", git::slug(name), &id[..6]);
        let branch = format!("tectr/{unique}");
        let path = self
            .data_dir
            .join("worktrees")
            .join(&workspace_id.0)
            .join(&unique);
        git::add_worktree(repo, &path, &branch, start)
            .await
            .map_err(internal)?;
        // macOS temp directories are symlinks. Agents should see the real path.
        let path = tokio::fs::canonicalize(&path)
            .await
            .map_err(|e| internal(e.into()))?;
        Ok((path.display().to_string(), branch))
    }

    /// Reads what each folder has checked out, because an agent can switch branches. Only the
    /// topic of `session` when there is one, otherwise every topic.
    pub async fn refresh_branches(self: Arc<Self>, session: Option<SessionId>) {
        let folders: Vec<(TopicId, WorktreeId, String)> = {
            let inner = self.lock();
            inner
                .state
                .topics()
                .filter(|topic| {
                    session
                        .as_ref()
                        .is_none_or(|id| topic.sessions.iter().any(|s| &s.id == id))
                })
                .flat_map(|topic| {
                    topic
                        .worktrees
                        .iter()
                        .map(|w| (topic.id.clone(), w.id.clone(), w.path.clone()))
                })
                .collect()
        };
        let mut found = vec![];
        for (topic_id, worktree_id, path) in folders {
            // A folder that's gone has nothing checked out to show.
            if let Ok(branch) = git::current_branch(Path::new(&path)).await {
                found.push((topic_id, worktree_id, branch));
            }
        }
        let mut inner = self.lock();
        let mut changed: BTreeMap<TopicId, Topic> = BTreeMap::new();
        for (topic_id, worktree_id, branch) in found {
            let Some(topic) = changed
                .get(&topic_id)
                .or_else(|| inner.state.topic(&topic_id))
                .cloned()
            else {
                continue;
            };
            let mut topic = topic;
            if let Some(worktree) = topic.worktrees.iter_mut().find(|w| w.id == worktree_id)
                && worktree.branch != branch
            {
                worktree.branch = branch;
                changed.insert(topic_id, topic);
            }
        }
        for (topic_id, topic) in changed {
            inner.emit(StateChange::WorktreesChanged {
                topic_id,
                worktree_id: topic.worktree_id,
                worktrees: topic.worktrees,
            });
        }
    }

    fn start_session(
        self: &Arc<Self>,
        topic_id: protocol::TopicId,
        agent: String,
        mode: PermissionMode,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<CommandOutput, CommandError> {
        let Some(launcher) = self.agents.get(&agent) else {
            return Err(invalid(format!("this daemon has no agent called {agent}")));
        };
        if launcher.mode_id(mode).is_none() {
            return Err(invalid(format!(
                "{agent} can't run in that permission mode"
            )));
        }
        let mut inner = self.lock();
        let cwd = inner
            .state
            .topic(&topic_id)
            .ok_or_else(|| not_found("topic", &topic_id))?
            .worktree()
            .ok_or_else(|| internal(anyhow::anyhow!("the topic has no folder")))?
            .path
            .clone();

        let session = Session {
            id: SessionId(crate::new_id()),
            agent,
            mode,
            model,
            effort,
            status: SessionStatus::Starting,
            title: None,
            unread: false,
            opened_at_ms: now_ms(),
        };
        inner
            .store
            .insert_session(&topic_id, &session)
            .map_err(internal)?;
        inner
            .store
            .set_agent_cwd(&session.id, &cwd)
            .map_err(internal)?;
        inner.emit(StateChange::SessionAdded {
            topic_id: topic_id.clone(),
            session: session.clone(),
        });
        inner.append(
            &session.id,
            SessionEventKind::StatusChanged {
                status: SessionStatus::Starting,
            },
        );
        self.spawn_agent(&mut inner, &session, false)
            .map_err(internal)?;
        Ok(CommandOutput::Session { topic_id, session })
    }

    async fn detect_providers(&self) -> CommandOutput {
        let mut candidates = vec![];
        for (agent, launcher) in &self.agents {
            candidates.push(ProviderCandidate {
                agent: agent.clone(),
                name: launcher.display_name(agent),
                missing: launcher.missing().await,
            });
        }
        CommandOutput::ProviderCandidates { candidates }
    }

    /// Starts the agent once to ask what it offers. The folder it starts in is
    /// empty, so it can't read anyone's project.
    async fn probe_provider(&self, agent: &str) -> Result<Provider, CommandError> {
        let Some(launcher) = self.agents.get(agent) else {
            return Err(invalid(format!("this daemon has no agent called {agent}")));
        };
        let cwd = self.data_dir.join("probe");
        tokio::fs::create_dir_all(&cwd)
            .await
            .map_err(|e| internal(e.into()))?;
        acp::probe(self, agent, launcher, cwd)
            .await
            .map_err(|error| invalid(format!("{agent} didn't start: {error:#}")))
    }

    async fn add_provider(&self, agent: &str) -> Result<CommandOutput, CommandError> {
        let mut provider = self.probe_provider(agent).await?;
        let mut inner = self.lock();
        if let Some(previous) = inner.state.providers.iter().find(|p| p.agent == agent) {
            provider.keep_defaults(previous);
        }
        inner.store.upsert_provider(&provider).map_err(internal)?;
        inner.emit(StateChange::ProviderChanged {
            provider: provider.clone(),
        });
        Ok(CommandOutput::Provider { provider })
    }

    fn remove_provider(&self, agent: &str) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        inner.store.remove_provider(agent).map_err(internal)?;
        inner.emit(StateChange::ProviderRemoved {
            agent: agent.to_owned(),
        });
        Ok(CommandOutput::Done)
    }

    fn set_provider_defaults(
        &self,
        agent: &str,
        model: Option<String>,
        effort: Option<String>,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let Some(mut provider) = inner
            .state
            .providers
            .iter()
            .find(|p| p.agent == agent)
            .cloned()
        else {
            return Err(not_found("provider", &agent));
        };
        let chosen = model.as_deref().and_then(|id| provider.model(id));
        if model.is_some() && chosen.is_none() {
            return Err(invalid(format!("{agent} doesn't offer that model")));
        }
        if let Some(effort) = &effort
            && !chosen.is_some_and(|model| model.efforts.iter().any(|e| &e.id == effort))
        {
            return Err(invalid(format!(
                "that model doesn't offer the effort {effort}"
            )));
        }
        provider.default_model = model;
        provider.default_effort = effort;
        inner.store.upsert_provider(&provider).map_err(internal)?;
        inner.emit(StateChange::ProviderChanged {
            provider: provider.clone(),
        });
        Ok(CommandOutput::Provider { provider })
    }

    /// Asks the added providers again, so new models appear without doing anything.
    /// One that fails keeps what it last reported.
    pub async fn refresh_providers(self: Arc<Self>) {
        let agents: Vec<String> = {
            let inner = self.lock();
            inner
                .state
                .providers
                .iter()
                .map(|p| p.agent.clone())
                .filter(|agent| {
                    self.agents
                        .get(agent)
                        .is_some_and(AgentLauncher::refreshes_on_start)
                })
                .collect()
        };
        for agent in agents {
            match self.probe_provider(&agent).await {
                Ok(mut provider) => {
                    let mut inner = self.lock();
                    // It may have been removed while the agent was starting.
                    let Some(previous) = inner.state.providers.iter().find(|p| p.agent == agent)
                    else {
                        continue;
                    };
                    provider.keep_defaults(previous);
                    if let Err(error) = inner.store.upsert_provider(&provider) {
                        tracing::warn!(agent, "failed to store a provider: {error:#}");
                        continue;
                    }
                    inner.emit(StateChange::ProviderChanged { provider });
                }
                Err(error) => {
                    tracing::warn!(agent, "failed to refresh a provider: {}", error.message)
                }
            }
        }
    }

    /// Starts the agent process for a session, resuming the agent's own session
    /// if it already has one.
    fn spawn_agent(
        self: &Arc<Self>,
        inner: &mut Inner,
        session: &Session,
        turn_active: bool,
    ) -> Result<()> {
        let launcher = self
            .agents
            .get(&session.agent)
            .with_context(|| {
                format!(
                    "this daemon no longer has an agent called {}",
                    session.agent
                )
            })?
            .clone();
        let cwd = inner
            .folder_of(&session.id)
            .context("the session's topic has no folder")?
            .path
            .clone();
        let topic = inner
            .state
            .topics()
            .find(|topic| topic.sessions.iter().any(|s| s.id == session.id))
            .context("the session's topic no longer exists")?;
        let proxy_secret = crate::new_id();
        let proxy = format!("http://127.0.0.1:{}/mcp/{proxy_secret}", self.port);
        let (mcp_ids, mcp_servers) = crate::mcp::agent_servers(&inner.state, topic, &proxy);
        let mcp_fingerprint = mcp::fingerprint(&inner.state, topic);
        let spawn = acp::AgentSpawn {
            run: proxy_secret.clone(),
            mcp: mcp_servers,
            session_id: session.id.clone(),
            cwd: cwd.into(),
            launcher,
            settings: SessionSettings {
                mode: session.mode,
                model: session.model.clone(),
                effort: session.effort.clone(),
            },
            resume: inner.store.agent_session_id(&session.id)?,
        };
        let (commands, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(acp::run(self.clone(), spawn, receiver));
        inner.live.insert(
            session.id.clone(),
            LiveSession {
                commands,
                turn_active,
                pending: HashMap::new(),
                task: Some(task),
                proxy_secret,
                mcp: mcp_ids,
                mcp_fingerprint,
                mcp_stale: false,
            },
        );
        Ok(())
    }

    fn send_prompt(
        self: &Arc<Self>,
        session_id: &SessionId,
        text: String,
        images: Vec<PromptImage>,
    ) -> Result<CommandOutput, CommandError> {
        if text.trim().is_empty() && images.is_empty() {
            return Err(invalid("the prompt is empty"));
        }
        if let Some(reason) = protocol::refused_prompt(&text) {
            return Err(invalid(reason));
        }
        // Read before taking the lock, which nothing should wait on the disk for.
        let attached = images
            .iter()
            .map(|image| {
                let read = self
                    .images
                    .read(&image.id, false)
                    .map_err(|_| not_found("image", &image.id))?;
                Ok((image.clone(), read.data))
            })
            .collect::<Result<Vec<_>, CommandError>>()?;
        let mut inner = self.lock();
        let session = inner
            .state
            .session(session_id)
            .ok_or_else(|| not_found("session", session_id))?
            .clone();

        match inner.live.get_mut(session_id) {
            Some(live) if live.turn_active => {
                return Err(CommandError {
                    code: ErrorCode::Busy,
                    message: "this session is already working on a turn".to_owned(),
                });
            }
            Some(live) => live.turn_active = true,
            None => self
                .spawn_agent(&mut inner, &session, true)
                .map_err(internal)?,
        }

        let folder = inner
            .folder_of(session_id)
            .ok_or_else(|| internal(anyhow::anyhow!("the session's topic has no folder")))?
            .clone();
        let told = inner.store.agent_cwd(session_id).map_err(internal)?;
        let sent = match &told {
            Some(old) if old != &folder.path => moved_note(old, &folder) + &text,
            _ => text.clone(),
        };

        inner.append(session_id, SessionEventKind::UserPrompt { text, images });
        inner.set_status(session_id, SessionStatus::Working);
        let live = inner.live.get(session_id).expect("made live just above");
        live.commands
            .send(SessionCommand::Prompt(Prompt {
                text: sent,
                images: attached,
            }))
            .map_err(|_| internal(anyhow::anyhow!("the agent has just exited")))?;
        if told.as_ref() != Some(&folder.path)
            && let Err(error) = inner.store.set_agent_cwd(session_id, &folder.path)
        {
            tracing::error!(%session_id, "failed to store the agent's folder: {error:#}");
        }
        // It's been sent, so it's no longer a draft. A failure above leaves it in place.
        if let Err(error) = inner.clear_draft(session_id) {
            tracing::error!(%session_id, "failed to clear a draft: {error:#}");
        }
        Ok(CommandOutput::Done)
    }

    fn cancel_turn(&self, session_id: &SessionId) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let live = inner
            .live
            .get_mut(session_id)
            .filter(|live| live.turn_active)
            .ok_or_else(|| invalid("this session isn't working on a turn"))?;
        // ACP requires answering open permission requests when cancelling.
        let pending: Vec<_> = live.pending.drain().collect();
        let _ = live.commands.send(SessionCommand::Cancel);
        for (approval_id, answer) in pending {
            answer(None);
            inner.resolve_approval(session_id, &approval_id, PermissionOutcome::Cancelled);
        }
        inner.set_status(session_id, SessionStatus::Working);
        Ok(CommandOutput::Done)
    }

    fn answer_approval(
        &self,
        approval_id: &ApprovalId,
        option_id: &str,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        // Taking the answer out under the lock means only the first of two
        // clients answering at once can succeed.
        let approval = inner
            .state
            .approvals
            .iter()
            .find(|approval| &approval.id == approval_id)
            .ok_or_else(|| not_found("pending approval", approval_id))?
            .clone();
        if !approval.options.iter().any(|option| option.id == option_id) {
            return Err(invalid(format!("this approval has no option {option_id}")));
        }
        let session_id = approval.session_id;
        let live = inner
            .live
            .get_mut(&session_id)
            .ok_or_else(|| not_found("running session for approval", approval_id))?;
        let answer = live
            .pending
            .remove(approval_id)
            .ok_or_else(|| not_found("pending approval", approval_id))?;
        let others_waiting = !live.pending.is_empty();
        let turn_active = live.turn_active;
        answer(Some(option_id.to_owned()));

        inner.resolve_approval(
            &session_id,
            approval_id,
            PermissionOutcome::Selected {
                option_id: option_id.to_owned(),
            },
        );
        if !others_waiting {
            let status = if turn_active {
                SessionStatus::Working
            } else {
                SessionStatus::Done
            };
            inner.set_status(&session_id, status);
        }
        Ok(CommandOutput::Done)
    }

    /// The session only changes once its agent has, so what's shown is what runs. An agent
    /// that isn't running has nothing to refuse, and picks these up when it next starts.
    async fn set_session_settings(
        &self,
        session_id: &SessionId,
        settings: SessionSettings,
    ) -> Result<CommandOutput, CommandError> {
        let answer = {
            let inner = self.lock();
            let session = inner
                .state
                .session(session_id)
                .ok_or_else(|| not_found("session", session_id))?;
            let launcher = self.agents.get(&session.agent).ok_or_else(|| {
                invalid(format!(
                    "this daemon no longer has an agent called {}",
                    session.agent
                ))
            })?;
            if launcher.mode_id(settings.mode).is_none() {
                return Err(invalid(format!(
                    "{} can't run in that permission mode",
                    session.agent
                )));
            }
            // A model that's no longer on offer can stay while only the mode changes.
            let same_choice = settings.model == session.model && settings.effort == session.effort;
            let provider = inner
                .state
                .providers
                .iter()
                .find(|provider| provider.agent == session.agent);
            if let (false, Some(provider), Some(model)) =
                (same_choice, provider, settings.model.as_deref())
            {
                let chosen = provider.model(model).ok_or_else(|| {
                    invalid(format!("{} doesn't offer that model", session.agent))
                })?;
                if let Some(effort) = &settings.effort
                    && !chosen.efforts.iter().any(|e| &e.id == effort)
                {
                    return Err(invalid(format!(
                        "that model doesn't offer the effort {effort}"
                    )));
                }
            }
            inner.live.get(session_id).and_then(|live| {
                let (reply, answer) = oneshot::channel();
                let settings = settings.clone();
                let command = SessionCommand::Configure { settings, reply };
                live.commands.send(command).ok().map(|()| answer)
            })
        };

        // No answer at all means the agent exited first, so nothing is running these.
        if let Some(answer) = answer
            && let Ok(result) = answer.await
        {
            result.map_err(invalid)?;
        }

        let mut inner = self.lock();
        let session = inner
            .state
            .session_mut(session_id)
            .ok_or_else(|| not_found("session", session_id))?;
        session.mode = settings.mode;
        session.model = settings.model;
        session.effort = settings.effort;
        let session = session.clone();
        inner.store.update_session(&session).map_err(internal)?;
        inner.emit(StateChange::SessionChanged { session });
        Ok(CommandOutput::Done)
    }

    fn set_unread(
        &self,
        session_id: &SessionId,
        unread: bool,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let session = inner
            .state
            .session_mut(session_id)
            .ok_or_else(|| not_found("session", session_id))?;
        let finished = matches!(session.status, SessionStatus::Done | SessionStatus::Stopped);
        if unread && !finished {
            return Err(invalid("only a finished session can be unread"));
        }
        if session.unread == unread {
            return Ok(CommandOutput::Done);
        }
        session.unread = unread;
        let session = session.clone();
        inner.store.update_session(&session).map_err(internal)?;
        inner.emit(StateChange::SessionChanged { session });
        Ok(CommandOutput::Done)
    }

    fn set_draft(
        &self,
        session_id: SessionId,
        text: String,
        images: Vec<PromptImage>,
    ) -> Result<CommandOutput, CommandError> {
        if let Some(image) = images.iter().find(|image| !self.images.has(&image.id)) {
            return Err(not_found("image", &image.id));
        }
        let mut inner = self.lock();
        if inner.state.session(&session_id).is_none() {
            return Err(not_found("session", &session_id));
        }
        inner
            .set_draft(&session_id, text, images)
            .map_err(internal)?;
        Ok(CommandOutput::Done)
    }

    /// Stops the agent and removes its tab. Its events stay in the database.
    fn close_session(
        &self,
        session_id: &SessionId,
        force: bool,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let session = inner
            .state
            .session(session_id)
            .ok_or_else(|| not_found("session", session_id))?;
        if inner.is_running(session) && !force {
            return Err(CommandError {
                code: ErrorCode::Busy,
                message: "this agent is still running, so closing it needs confirming".to_owned(),
            });
        }
        inner
            .store
            .close_session(session_id, now_ms())
            .map_err(internal)?;

        if let Some(task) = inner.stop_agent(session_id) {
            inner.keep_closing(task);
        }

        inner.emit(StateChange::SessionRemoved {
            session_id: session_id.clone(),
        });
        inner.clear_draft(session_id).map_err(internal)?;
        Ok(CommandOutput::Done)
    }

    /// Removes the image files no draft or sent prompt names, now and then every few hours.
    /// Prompts in closed and archived tabs count, because their transcripts can still be read.
    pub async fn collect_images(self: Arc<Self>) {
        let mut every = tokio::time::interval(COLLECT_IMAGES_EVERY);
        loop {
            every.tick().await;
            if self.shutting_down.load(Ordering::Relaxed) {
                return;
            }
            // Both from one moment, so an image moving from a draft to a prompt counts.
            let keep = {
                let inner = self.lock();
                inner.store.prompt_image_ids().map(|mut keep| {
                    let drafts = inner.state.draft_images.values().flatten();
                    keep.extend(drafts.map(|image| image.id.clone()));
                    keep
                })
            };
            let keep = match keep {
                Ok(keep) => keep,
                Err(error) => {
                    tracing::error!("failed to find the images in use: {error:#}");
                    continue;
                }
            };
            let images = self.images.clone();
            let before = SystemTime::now() - images::UNREFERENCED_GRACE;
            match tokio::task::spawn_blocking(move || images.collect(&keep, before)).await {
                Ok(Ok(0)) => {}
                Ok(Ok(removed)) => tracing::info!(removed, "removed images nothing names"),
                Ok(Err(error)) => tracing::error!("failed to remove unused images: {error:#}"),
                Err(error) => tracing::error!("failed to remove unused images: {error}"),
            }
        }
    }

    fn open_terminal(
        self: &Arc<Self>,
        topic_id: TopicId,
        cols: u16,
        rows: u16,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let cwd = inner
            .state
            .topic(&topic_id)
            .ok_or_else(|| not_found("topic", &topic_id))?
            .worktree()
            .ok_or_else(|| internal(anyhow::anyhow!("the topic has no folder")))?
            .path
            .clone();
        let terminal = Terminal {
            id: TerminalId(crate::new_id()),
            title: None,
            status: TerminalStatus::Running,
            opened_at_ms: now_ms(),
        };
        inner
            .store
            .insert_terminal(&topic_id, &terminal, &cwd)
            .map_err(internal)?;
        inner.emit(StateChange::TerminalAdded {
            topic_id: topic_id.clone(),
            terminal: terminal.clone(),
        });
        let folder = PathBuf::from(cwd);
        self.start_terminal(
            &mut inner,
            &terminal.id,
            folder.clone(),
            folder,
            (cols, rows),
        )
        .map_err(internal)?;
        Ok(CommandOutput::Terminal { topic_id, terminal })
    }

    fn close_terminal(&self, terminal_id: &TerminalId) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        if inner.state.terminal(terminal_id).is_none() {
            return Err(not_found("terminal", terminal_id));
        }
        inner
            .store
            .close_terminal(terminal_id, now_ms())
            .map_err(internal)?;
        if !inner.stop_terminal(terminal_id, false) {
            let _ = std::fs::remove_file(self.terminal_history(terminal_id));
        }
        inner.emit(StateChange::TerminalRemoved {
            terminal_id: terminal_id.clone(),
        });
        Ok(CommandOutput::Done)
    }

    fn terminal_history(&self, id: &TerminalId) -> PathBuf {
        self.data_dir.join("terminals").join(format!("{id}.vt"))
    }

    /// Starts a terminal's thread and shell. Its events come back through `terminal_event`.
    fn start_terminal(
        self: &Arc<Self>,
        inner: &mut Inner,
        id: &TerminalId,
        cwd: PathBuf,
        fallback: PathBuf,
        (cols, rows): (u16, u16),
    ) -> Result<()> {
        let run = crate::new_id();
        let core = Arc::downgrade(self);
        let notify: terminal::Notify = {
            let id = id.clone();
            let run = run.clone();
            Box::new(move |event| {
                if let Some(core) = core.upgrade() {
                    core.terminal_event(&id, &run, event);
                }
            })
        };
        let start = terminal::Start {
            id: id.clone(),
            shell: self.shell.clone(),
            cwd,
            fallback,
            cols: cols.max(1),
            rows: rows.max(1),
            history: self.terminal_history(id),
        };
        let handle = terminal::Handle::start(start, notify)?;
        inner
            .terminals
            .insert(id.clone(), LiveTerminal { handle, run });
        Ok(())
    }

    fn terminal_event(&self, id: &TerminalId, run: &str, event: terminal::Event) {
        let mut inner = self.lock();
        // A terminal saves where it is on its way out too, after it's no longer live.
        if let terminal::Event::Cwd(cwd) = &event {
            if let Err(error) = inner.store.set_terminal_cwd(id, cwd) {
                tracing::warn!(%id, "failed to store where a terminal is: {error:#}");
            }
            return;
        }
        if inner.terminals.get(id).is_none_or(|live| live.run != run) {
            return;
        }
        let Some(mut terminal) = inner.state.terminal(id).cloned() else {
            return;
        };
        match event {
            terminal::Event::Running => terminal.status = TerminalStatus::Running,
            terminal::Event::Title(title) => {
                if let Err(error) = inner.store.set_terminal_title(id, title.as_deref()) {
                    tracing::warn!(%id, "failed to store a terminal's title: {error:#}");
                }
                terminal.title = title;
            }
            // A shell that exits cleanly was meant to, like typing exit, so its tab goes.
            terminal::Event::Exited(Some(0)) => {
                if let Err(error) = inner.store.close_terminal(id, now_ms()) {
                    tracing::error!(%id, "failed to close a terminal: {error:#}");
                }
                if let Some(live) = inner.terminals.remove(id) {
                    // This runs on the terminal's own thread, which ends after this returns.
                    drop(live.handle.stop(false));
                }
                inner.emit(StateChange::TerminalRemoved {
                    terminal_id: id.clone(),
                });
                return;
            }
            terminal::Event::Exited(_) => terminal.status = TerminalStatus::Exited,
            terminal::Event::Cwd(_) => unreachable!("handled above"),
        }
        if inner.state.terminal(id) != Some(&terminal) {
            inner.emit(StateChange::TerminalChanged { terminal });
        }
    }

    pub fn next_subscriber(&self) -> u64 {
        self.next_subscriber.fetch_add(1, Ordering::Relaxed)
    }

    /// Starts sending a terminal to a client, starting its shell if it isn't running.
    pub fn attach_terminal(
        self: &Arc<Self>,
        id: &TerminalId,
        subscriber: Subscriber,
        (cols, rows): (u16, u16),
        colors: Option<TerminalColors>,
    ) -> Result<()> {
        let mut inner = self.lock();
        if inner.state.terminal(id).is_none() {
            anyhow::bail!("no terminal with ID {id}");
        }
        if !inner.terminals.contains_key(id) {
            let folder = inner
                .state
                .topics()
                .find(|topic| topic.terminals.iter().any(|t| &t.id == id))
                .and_then(Topic::worktree)
                .map(|worktree| PathBuf::from(&worktree.path))
                .context("the terminal's topic has no folder")?;
            let cwd = inner
                .store
                .terminal_cwd(id)?
                .map_or_else(|| folder.clone(), PathBuf::from);
            self.start_terminal(&mut inner, id, cwd, folder, (cols, rows))?;
        }
        let live = inner.terminals.get(id).expect("started just above");
        live.handle.attach(subscriber, cols, rows, colors);
        Ok(())
    }

    pub fn detach_terminal(&self, id: &TerminalId, subscriber: u64) {
        if let Some(live) = self.lock().terminals.get(id) {
            live.handle.detach(subscriber);
        }
    }

    pub fn resize_terminal(&self, id: &TerminalId, cols: u16, rows: u16) {
        if let Some(live) = self.lock().terminals.get(id) {
            live.handle.resize(cols, rows);
        }
    }

    pub fn terminal_input(&self, id: &TerminalId, data: Vec<u8>) {
        if let Some(live) = self.lock().terminals.get(id) {
            live.handle.input(data);
        }
    }

    pub fn terminal_ack(&self, id: &TerminalId, subscriber: u64, bytes: u32) {
        if let Some(live) = self.lock().terminals.get(id) {
            live.handle.ack(subscriber, bytes);
        }
    }

    pub fn config_home(&self) -> &ConfigHome {
        &self.config_home
    }

    /// Runs one adapter install at a time, then returns how to start the agent.
    pub async fn resolve_launcher(
        &self,
        launcher: &AgentLauncher,
    ) -> Result<(PathBuf, Vec<String>)> {
        let _guard = self.adapter_install.lock().await;
        launcher.resolve(&self.data_dir).await
    }

    pub fn agent_session_created(&self, id: &SessionId, agent_session_id: &str) {
        let inner = self.lock();
        if let Err(error) = inner.store.set_agent_session_id(id, agent_session_id) {
            tracing::error!(%id, "failed to store the agent's session ID: {error:#}");
        }
    }

    /// The agent has finished starting. A prompt that arrived meanwhile has
    /// already set the session to working, and that stays.
    pub fn agent_ready(&self, id: &SessionId, ready: SessionEventKind) {
        let mut inner = self.lock();
        inner.append(id, ready);
        let turn_active = inner.live.get(id).is_some_and(|live| live.turn_active);
        let starting = inner
            .state
            .session(id)
            .is_some_and(|session| session.status == SessionStatus::Starting);
        if starting && !turn_active {
            inner.set_status(id, SessionStatus::Ready);
        }
    }

    pub fn agent_event(&self, id: &SessionId, kind: SessionEventKind) {
        self.lock().append(id, kind);
    }

    pub fn title_changed(&self, id: &SessionId, title: Option<String>) {
        let mut inner = self.lock();
        let Some(session) = inner.state.session_mut(id) else {
            return;
        };
        if session.title == title {
            return;
        }
        session.title = title.clone();
        let session = session.clone();
        if let Err(error) = inner.store.update_session(&session) {
            tracing::error!(%id, "failed to store the session title: {error:#}");
        }
        inner.emit(StateChange::SessionChanged { session });
        if let Some(title) = title {
            inner.name_topic_after(id, title.trim());
        }
    }

    pub fn commands_changed(&self, id: &SessionId, mut commands: Vec<AgentCommand>) {
        commands.retain(|command| protocol::tectr_owned_command(&command.name).is_none());
        let mut inner = self.lock();
        let current = inner.state.commands.get(id).map_or(&[][..], Vec::as_slice);
        if current == commands.as_slice() || inner.state.session(id).is_none() {
            return;
        }
        if let Err(error) = inner.store.set_session_commands(id, &commands) {
            tracing::error!(%id, "failed to store the session's commands: {error:#}");
        }
        inner.emit(StateChange::CommandsChanged {
            session_id: id.clone(),
            commands,
        });
    }

    pub fn permission_requested(
        &self,
        id: &SessionId,
        tool_call: ToolCallPatch,
        options: Vec<PermissionOption>,
        answer: PermissionAnswer,
    ) {
        let mut inner = self.lock();
        let approval = Approval {
            id: ApprovalId(crate::new_id()),
            session_id: id.clone(),
            tool_call,
            options,
        };
        let Some(live) = inner.live.get_mut(id) else {
            answer(None);
            return;
        };
        live.pending.insert(approval.id.clone(), answer);
        if let Err(error) = inner.store.insert_pending_approval(&approval) {
            tracing::error!(%id, "failed to store a pending approval: {error:#}");
        }
        inner.append(
            id,
            SessionEventKind::PermissionRequested {
                approval: approval.clone(),
            },
        );
        inner.emit(StateChange::ApprovalAdded { approval });
        inner.set_status(id, SessionStatus::WaitingForYou);
    }

    pub fn turn_ended(&self, id: &SessionId, result: Result<StopReason, String>) {
        let mut inner = self.lock();
        // Background work can still be asking for something after a turn ends.
        // The session then keeps waiting until that's answered.
        let still_waiting = inner.live.get_mut(id).is_some_and(|live| {
            live.turn_active = false;
            !live.pending.is_empty()
        });
        match result {
            Ok(stop_reason) => {
                inner.append(id, SessionEventKind::TurnEnded { stop_reason });
                let status = if still_waiting {
                    SessionStatus::WaitingForYou
                } else {
                    SessionStatus::Done
                };
                inner.set_status(id, status);
            }
            Err(message) => {
                inner.append(id, SessionEventKind::Error { message });
                inner.set_status(id, SessionStatus::Failed);
            }
        }
        if !still_waiting && inner.live.get(id).is_some_and(|live| live.mcp_stale) {
            inner.stop_for_mcp(id);
        }
    }

    /// The agent process is gone, either as asked or unexpectedly. `run` tells this process
    /// apart from one that already replaced it, as after a restart for new MCP servers.
    pub fn agent_exited(&self, id: &SessionId, run: &str, result: Result<()>) {
        let mut inner = self.lock();
        if inner
            .live
            .get(id)
            .is_none_or(|live| live.proxy_secret != run)
        {
            return;
        }
        let Some(mut live) = inner.live.remove(id) else {
            return;
        };
        for (approval_id, answer) in live.pending.drain() {
            answer(None);
            inner.resolve_approval(id, &approval_id, PermissionOutcome::Cancelled);
        }
        let failure = match result {
            Err(error) => Some(format!("{error:#}")),
            Ok(()) if live.turn_active => Some("the agent exited during a turn".to_owned()),
            Ok(()) => None,
        };
        match failure {
            Some(message) if !self.shutting_down.load(Ordering::SeqCst) => {
                inner.append(id, SessionEventKind::Error { message });
                inner.set_status(id, SessionStatus::Failed);
            }
            _ => inner.set_status(id, SessionStatus::Stopped),
        }
    }

    /// Stops every agent and waits for them to exit.
    pub async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
        // Remote clients go first, so none of them sees agents stopping one by one.
        if let Some(endpoint) = self.endpoint.get() {
            endpoint.close().await;
        }
        let tasks: Vec<_> = {
            let mut inner = self.lock();
            let mut tasks: Vec<_> = inner
                .live
                .values_mut()
                .filter_map(|live| {
                    let _ = live.commands.send(SessionCommand::Shutdown);
                    live.task.take()
                })
                .collect();
            tasks.append(&mut inner.closing);
            let ids: Vec<TerminalId> = inner.terminals.keys().cloned().collect();
            for id in ids {
                inner.stop_terminal(&id, true);
            }
            tasks
        };
        // Terminals only save and ask their shells to end, so this takes moments.
        let threads = std::mem::take(&mut self.lock().stopping_terminals);
        let joined = tokio::task::spawn_blocking(move || {
            for thread in threads {
                let _ = thread.join();
            }
        });
        let _ = tokio::time::timeout(Duration::from_secs(5), joined).await;
        for task in tasks {
            // The SDK ends a stubborn agent within about three seconds.
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
        // An agent that didn't stop in time can't hold the machine awake.
        self.lock().awake.set(false);
    }
}

impl Inner {
    /// Agent processes don't survive the daemon, so after a crash nothing can
    /// still be working or waiting.
    fn recover(&mut self) -> Result<()> {
        for approval in self.store.pending_approvals()? {
            self.resolve_approval(
                &approval.session_id.clone(),
                &approval.id,
                PermissionOutcome::Cancelled,
            );
        }
        let interrupted: Vec<SessionId> = self
            .state
            .sessions()
            .filter(|session| {
                matches!(
                    session.status,
                    SessionStatus::Starting | SessionStatus::Working | SessionStatus::WaitingForYou
                )
            })
            .map(|session| session.id.clone())
            .collect();
        for id in interrupted {
            self.set_status(&id, SessionStatus::Stopped);
        }
        Ok(())
    }

    /// The topic with the path of its workspace's repo.
    fn topic_with_repo(
        &self,
        topic_id: &TopicId,
    ) -> Result<(WorkspaceId, PathBuf, Topic), CommandError> {
        self.state
            .workspaces
            .iter()
            .find_map(|workspace| {
                let topic = workspace.topics.iter().find(|t| &t.id == topic_id)?;
                Some((
                    workspace.id.clone(),
                    PathBuf::from(&workspace.path),
                    topic.clone(),
                ))
            })
            .ok_or_else(|| not_found("topic", topic_id))
    }

    /// The folder a session's tabs work in, which is its topic's.
    fn folder_of(&self, session_id: &SessionId) -> Option<&Worktree> {
        self.state
            .topics()
            .find(|topic| topic.sessions.iter().any(|s| &s.id == session_id))?
            .worktree()
    }

    fn check_idle(&self, topic: &Topic, force: bool) -> Result<(), CommandError> {
        if !force && topic.sessions.iter().any(|s| self.is_running(s)) {
            return Err(CommandError {
                code: ErrorCode::Busy,
                message: "an agent in this topic is still running, so this needs confirming"
                    .to_owned(),
            });
        }
        Ok(())
    }

    fn is_running(&self, session: &Session) -> bool {
        matches!(
            session.status,
            SessionStatus::Starting | SessionStatus::Working | SessionStatus::WaitingForYou
        ) || self.live.get(&session.id).is_some_and(|l| l.turn_active)
    }

    /// Tells a session's agent to stop, and returns the task to wait for if it was running.
    /// Taking it out of `live` first means the agent exiting isn't reported as a failure.
    fn stop_agent(&mut self, session_id: &SessionId) -> Option<JoinHandle<()>> {
        let mut live = self.live.remove(session_id)?;
        let _ = live.commands.send(SessionCommand::Shutdown);
        // ACP requires answering open permission requests.
        for (approval_id, answer) in live.pending.drain() {
            answer(None);
            self.resolve_approval(session_id, &approval_id, PermissionOutcome::Cancelled);
        }
        live.task.take()
    }

    /// Ends a terminal's shell, saving what it showed for next time or deleting it. Returns
    /// whether it was running.
    fn stop_terminal(&mut self, id: &TerminalId, keep: bool) -> bool {
        let Some(live) = self.terminals.remove(id) else {
            return false;
        };
        self.stopping_terminals
            .retain(|thread| !thread.is_finished());
        self.stopping_terminals.extend(live.handle.stop(keep));
        if keep && let Some(terminal) = self.state.terminal(id).cloned() {
            let terminal = Terminal {
                status: TerminalStatus::Stopped,
                ..terminal
            };
            self.emit(StateChange::TerminalChanged { terminal });
        }
        true
    }

    fn keep_closing(&mut self, task: JoinHandle<()>) {
        self.closing.retain(|task| !task.is_finished());
        self.closing.push(task);
    }

    /// A topic nobody named goes by the title of its first tab, whenever that changes.
    fn name_topic_after(&mut self, session_id: &SessionId, title: &str) {
        let first_tab_of =
            |topic: &&Topic| topic.sessions.first().is_some_and(|s| &s.id == session_id);
        let Some(topic) = self.state.topics().find(first_tab_of) else {
            return;
        };
        if title.is_empty() || topic.name == title {
            return;
        }
        let topic_id = topic.id.clone();
        match self.store.auto_name_topic(&topic_id, title) {
            Ok(true) => self.emit(StateChange::TopicRenamed {
                topic_id,
                name: title.to_owned(),
            }),
            Ok(false) => {}
            Err(error) => tracing::error!(%topic_id, "failed to store a topic name: {error:#}"),
        }
    }

    /// Keeps what's typed for a session, if that changes anything. Empty removes it.
    fn set_draft(
        &mut self,
        session_id: &SessionId,
        text: String,
        images: Vec<PromptImage>,
    ) -> Result<()> {
        let mut drafts = self.state.drafts.clone();
        let mut draft_images = self.state.draft_images.clone();
        if text.is_empty() {
            drafts.remove(session_id);
        } else {
            drafts.insert(session_id.clone(), text.clone());
        }
        if images.is_empty() {
            draft_images.remove(session_id);
        } else {
            draft_images.insert(session_id.clone(), images.clone());
        }
        if drafts != self.state.drafts || draft_images != self.state.draft_images {
            self.store.set_drafts(&drafts, &draft_images)?;
            self.emit(StateChange::DraftChanged {
                session_id: session_id.clone(),
                text,
                images,
            });
        }
        Ok(())
    }

    fn clear_draft(&mut self, session_id: &SessionId) -> Result<()> {
        self.set_draft(session_id, String::new(), vec![])
    }

    fn emit(&mut self, change: StateChange) {
        self.state.apply(&change);
        let _ = self.signals.send(Signal::State(Arc::new(change)));
        self.sync_awake();
    }

    /// Every status, setting, and removed tab arrives through `emit`, so checking here keeps
    /// the machine awake exactly while some agent is starting or working. Waiting for an
    /// answer doesn't count: nothing moves until someone is back anyway.
    fn sync_awake(&mut self) {
        let busy = self.state.sessions().any(|session| {
            matches!(
                session.status,
                SessionStatus::Starting | SessionStatus::Working
            )
        });
        self.awake.set(busy && self.state.settings.keep_awake);
    }

    fn append(&mut self, id: &SessionId, kind: SessionEventKind) {
        let seq = self.last_seq.get(id).copied().unwrap_or(0) + 1;
        let event = SessionEvent {
            session: SessionRef {
                machine_id: self.machine_id().clone(),
                session_id: id.clone(),
            },
            seq,
            at_ms: now_ms(),
            kind,
        };
        // Skip a failed write entirely, so sequence numbers stay gapless and
        // match what's stored.
        if let Err(error) = self.store.insert_event(&event) {
            tracing::error!(%id, "failed to store an event: {error:#}");
            return;
        }
        self.last_seq.insert(id.clone(), seq);
        let _ = self.signals.send(Signal::Event(Arc::new(event)));
    }

    fn machine_id(&self) -> &MachineId {
        &self.state.machine.id
    }

    fn set_status(&mut self, id: &SessionId, status: SessionStatus) {
        let Some(session) = self.state.session_mut(id) else {
            return;
        };
        if session.status == status {
            return;
        }
        session.status = status;
        // An agent stopping after it finished leaves what it finished unread.
        match status {
            SessionStatus::Done => session.unread = true,
            SessionStatus::Stopped => {}
            _ => session.unread = false,
        }
        let session = session.clone();
        if let Err(error) = self.store.update_session(&session) {
            tracing::error!(%id, "failed to store the session status: {error:#}");
        }
        self.emit(StateChange::SessionChanged { session });
        self.append(id, SessionEventKind::StatusChanged { status });
    }

    fn resolve_approval(
        &mut self,
        session_id: &SessionId,
        approval_id: &ApprovalId,
        outcome: PermissionOutcome,
    ) {
        if let Err(error) = self.store.delete_pending_approval(approval_id) {
            tracing::error!(%approval_id, "failed to delete a pending approval: {error:#}");
        }
        self.append(
            session_id,
            SessionEventKind::PermissionResolved {
                approval_id: approval_id.clone(),
                outcome,
            },
        );
        self.emit(StateChange::ApprovalResolved {
            approval_id: approval_id.clone(),
        });
    }
}

/// The worktrees archiving removes, with the branch tectr made for each. The repo's own folder
/// isn't one of them.
fn owned_worktrees(
    repo: &Path,
    topic: &Topic,
    made: &[(WorktreeId, String)],
) -> Vec<(PathBuf, String)> {
    topic
        .worktrees
        .iter()
        .filter(|worktree| Path::new(&worktree.path) != repo)
        .map(|worktree| {
            let branch = made
                .iter()
                .find(|(id, _)| id == &worktree.id)
                .map_or(&worktree.branch, |(_, branch)| branch);
            (PathBuf::from(&worktree.path), branch.clone())
        })
        .collect()
}

async fn discard_worktrees(
    repo: PathBuf,
    worktrees: Vec<(PathBuf, String)>,
    agents: Vec<JoinHandle<()>>,
) {
    for agent in agents {
        // The SDK ends a stubborn agent within about three seconds.
        let _ = tokio::time::timeout(Duration::from_secs(5), agent).await;
    }
    for (path, branch) in worktrees {
        if let Err(error) = git::discard_worktree(&repo, &path, &branch).await {
            tracing::warn!(path = %path.display(), "failed to remove a worktree: {error:#}");
        }
    }
}

/// Goes before the first prompt an agent gets in a new folder. Its history is full of paths in
/// the old one, which it would otherwise go on editing.
fn moved_note(old: &str, folder: &Worktree) -> String {
    format!(
        "[tectr] This conversation moved to another folder. You now work in {} on branch {}, \
         no longer in {old}. Use paths in the new folder from here on and change nothing in the \
         old one.\n\n",
        folder.path, folder.branch
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

fn resolve_path(path: &str) -> Result<PathBuf, CommandError> {
    folders::resolve(path, dirs::home_dir().as_deref())
        .ok_or_else(|| invalid("the path must start with / or ~/"))
}

async fn list_folders(path: &str) -> Result<CommandOutput, CommandError> {
    let path = resolve_path(path)?;
    // Reading a folder can wait on a slow disk, or on someone answering a macOS prompt.
    let listed = tokio::task::spawn_blocking(move || {
        folders::list(&path, dirs::home_dir().as_deref())
            .map_err(|error| invalid(format!("Can't read {}: {error}", path.display())))
    })
    .await
    .map_err(|error| internal(error.into()))??;
    Ok(CommandOutput::Folders { folders: listed })
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError {
        code: ErrorCode::InvalidInput,
        message: message.into(),
    }
}

/// Runs work that decodes or waits on the disk away from the threads handling connections.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, CommandError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| internal(anyhow::anyhow!("{error}")))
}

fn not_found(what: &str, id: &impl std::fmt::Display) -> CommandError {
    CommandError {
        code: ErrorCode::NotFound,
        message: format!("no {what} with ID {id}"),
    }
}

fn internal(error: anyhow::Error) -> CommandError {
    CommandError {
        code: ErrorCode::Internal,
        message: format!("{error:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(hostname: &str, keep_awake: Option<KeepAwakeCommand>) -> CoreOptions {
        CoreOptions {
            hostname: hostname.to_owned(),
            agents: BTreeMap::new(),
            event_buffer: 16,
            keep_awake,
            port: 0,
            shell: Shell {
                program: "/bin/sh".into(),
                args: vec![],
            },
            config_home: ConfigHome {
                home: None,
                codex_home: None,
                xdg_config: None,
                opencode_config: None,
                opencode_content: None,
            },
            network: Network::Local,
        }
    }

    #[tokio::test]
    async fn a_chosen_name_outlives_a_restart_and_empty_goes_back_to_the_hostname() {
        let dir = tempfile::tempdir().unwrap();
        let open = || Core::open(dir.path(), options("host", None)).unwrap();
        let name = |core: &Core| core.snapshot_and_subscribe().0.machine.name;

        let core = open();
        assert_eq!(name(&core), "host");
        let rename = |name: &str| Command::RenameMachine {
            name: name.to_owned(),
        };
        core.handle(rename("  Desk  ")).await.unwrap();
        assert_eq!(name(&core), "Desk");

        drop(core);
        let core = open();
        assert_eq!(name(&core), "Desk");
        core.handle(rename(" ")).await.unwrap();
        assert_eq!(name(&core), "host");
    }

    #[tokio::test]
    async fn a_pairing_code_works_once_and_wrong_secrets_use_it_up() {
        let dir = tempfile::tempdir().unwrap();
        let core = Core::open(dir.path(), options("host", None)).unwrap();
        let start = || async {
            let Ok(CommandOutput::Pairing { code, .. }) = core.handle(Command::StartPairing).await
            else {
                panic!("no code");
            };
            protocol::PairingCode::parse(&code).unwrap()
        };
        let request = |secret: &str| protocol::PairRequest {
            secret: secret.to_owned(),
            name: "  Laptop  ".to_owned(),
        };
        let paired = |response: protocol::PairResponse| {
            matches!(response, protocol::PairResponse::Paired { .. })
        };
        let laptop = iroh::SecretKey::generate().public();
        let phone = iroh::SecretKey::generate().public();

        let code = start().await;
        assert_eq!(code.daemon_key, core.remote_id().to_string());
        assert!(core.remote_addr().is_some(), "pairing starts listening");
        assert!(!core.is_paired(&laptop));
        assert!(paired(core.pair(&laptop, &request(&code.secret))));
        assert!(core.is_paired(&laptop));
        let clients = core.snapshot_and_subscribe().0.paired_clients;
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].name, "Laptop");
        assert!(
            !paired(core.pair(&phone, &request(&code.secret))),
            "a code works once"
        );

        let code = start().await;
        for _ in 0..5 {
            assert!(!paired(core.pair(&phone, &request("wrong"))));
        }
        assert!(
            !paired(core.pair(&phone, &request(&code.secret))),
            "too many wrong secrets use the code up"
        );

        let code = start().await;
        core.handle(Command::CancelPairing).await.unwrap();
        assert!(!core.pairing_open());
        assert!(!paired(core.pair(&phone, &request(&code.secret))));

        let key = laptop.to_string();
        let remove = || Command::RemovePairedClient { key: key.clone() };
        core.handle(remove()).await.unwrap();
        assert!(!core.is_paired(&laptop));
        assert!(core.handle(remove()).await.is_err());
        core.shutdown().await;

        let reopened = Core::open(dir.path(), options("host", None)).unwrap();
        assert_eq!(
            reopened.remote_id(),
            code.daemon_key.parse().unwrap(),
            "the key survives a restart"
        );
    }

    #[tokio::test]
    async fn the_machine_stays_awake_only_while_an_agent_works_and_the_setting_allows() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::open(&dir.path().join("tectr.db")).unwrap();
            store
                .insert_workspace(&Workspace {
                    id: "w".into(),
                    name: "r".into(),
                    path: "/r".into(),
                    topics: vec![],
                })
                .unwrap();
            let topic = Topic {
                id: "t".into(),
                name: "t".into(),
                worktree_id: "wt".into(),
                worktrees: vec![Worktree {
                    id: "wt".into(),
                    path: "/wt".into(),
                    branch: "b".into(),
                }],
                sessions: vec![],
                terminals: vec![],
                mcp: BTreeMap::new(),
            };
            store.insert_topic(&"w".into(), &topic, false).unwrap();
            store
                .insert_session(
                    &topic.id,
                    &Session {
                        id: "s".into(),
                        agent: "a".into(),
                        mode: PermissionMode::Auto,
                        model: None,
                        effort: None,
                        status: SessionStatus::Ready,
                        title: None,
                        unread: false,
                        opened_at_ms: 0,
                    },
                )
                .unwrap();
        }
        let stand_in = KeepAwakeCommand {
            program: "sleep".into(),
            args: vec!["60".into()],
        };
        let open = || Core::open(dir.path(), options("m", Some(stand_in.clone()))).unwrap();
        let core = open();
        let set_status = |core: &Core, status| core.lock().set_status(&"s".into(), status);
        let awake = |core: &Core| core.lock().awake.is_on();
        let keep_awake = |keep_awake| Command::SetDeviceSettings {
            settings: DeviceSettings { keep_awake },
        };

        assert!(!awake(&core), "an idle machine may sleep");
        set_status(&core, SessionStatus::Working);
        assert!(awake(&core));
        set_status(&core, SessionStatus::WaitingForYou);
        assert!(!awake(&core), "nothing moves until someone answers");
        set_status(&core, SessionStatus::Working);
        assert!(awake(&core));

        core.handle(keep_awake(false)).await.unwrap();
        assert!(!awake(&core), "turning it off lets a working machine sleep");

        // The setting outlives a restart.
        drop(core);
        let core = open();
        assert!(!core.snapshot_and_subscribe().0.settings.keep_awake);
        set_status(&core, SessionStatus::Working);
        assert!(!awake(&core));
        core.handle(keep_awake(true)).await.unwrap();
        assert!(
            awake(&core),
            "turning it on applies to a turn already running"
        );

        set_status(&core, SessionStatus::Done);
        assert!(!awake(&core));
    }

    /// Simulates a daemon that died without shutting down: its database says
    /// agents are working and waiting, but their processes are gone.
    #[tokio::test]
    async fn startup_after_a_crash_clears_what_could_not_have_survived() {
        let dir = tempfile::tempdir().unwrap();
        let session_in = |id: &str, status| Session {
            id: id.into(),
            agent: "a".into(),
            mode: PermissionMode::Auto,
            model: None,
            effort: None,
            status,
            title: None,
            unread: false,
            opened_at_ms: 0,
        };
        let approval = Approval {
            id: "a1".into(),
            session_id: "waiting".into(),
            tool_call: ToolCallPatch::default(),
            options: vec![],
        };
        {
            let store = Store::open(&dir.path().join("tectr.db")).unwrap();
            store
                .insert_workspace(&Workspace {
                    id: "w".into(),
                    name: "r".into(),
                    path: "/r".into(),
                    topics: vec![],
                })
                .unwrap();
            let topic_id = protocol::TopicId::from("t");
            store
                .insert_topic(
                    &"w".into(),
                    &Topic {
                        id: topic_id.clone(),
                        name: "t".into(),
                        worktree_id: "wt".into(),
                        worktrees: vec![Worktree {
                            id: "wt".into(),
                            path: "/wt".into(),
                            branch: "b".into(),
                        }],
                        sessions: vec![],
                        terminals: vec![],
                        mcp: BTreeMap::new(),
                    },
                    false,
                )
                .unwrap();
            for (id, status) in [
                ("waiting", SessionStatus::WaitingForYou),
                ("working", SessionStatus::Working),
                ("done", SessionStatus::Done),
            ] {
                store
                    .insert_session(&topic_id, &session_in(id, status))
                    .unwrap();
            }
            store.insert_pending_approval(&approval).unwrap();
        }

        let core = Core::open(dir.path(), options("m", None)).unwrap();
        let (state, _) = core.snapshot_and_subscribe();

        assert!(state.approvals.is_empty());
        let status = |id: &str| state.session(&id.into()).unwrap().status;
        assert_eq!(status("waiting"), SessionStatus::Stopped);
        assert_eq!(status("working"), SessionStatus::Stopped);
        assert_eq!(
            status("done"),
            SessionStatus::Done,
            "a finished session stays finished"
        );

        let events = core.events_after(&"waiting".into(), 0, 10).unwrap();
        let kinds: Vec<_> = events.into_iter().map(|event| event.kind).collect();
        assert_eq!(
            kinds,
            [
                SessionEventKind::PermissionResolved {
                    approval_id: "a1".into(),
                    outcome: PermissionOutcome::Cancelled,
                },
                SessionEventKind::StatusChanged {
                    status: SessionStatus::Stopped,
                },
            ]
        );

        // A second start finds nothing left to clean up.
        drop(core);
        let core = Core::open(dir.path(), options("m", None)).unwrap();
        assert_eq!(
            core.events_after(&"waiting".into(), 0, 10).unwrap().len(),
            2
        );
    }
}
