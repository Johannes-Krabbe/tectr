//! The daemon's state and rules. Everything that changes a session goes through
//! here, so its status, stored events, and live updates can't drift apart.
//!
//! A single lock protects the state. It's held only for short, non-async work,
//! which also gives each session's events a single order. Events are stored
//! before they're broadcast, so a client that reads history can't miss one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use protocol::{
    Approval, ApprovalId, Command, CommandError, CommandOutput, DaemonState, ErrorCode, Machine,
    MachineId, PermissionMode, PermissionOption, PermissionOutcome, Provider, ProviderCandidate,
    Session, SessionEvent, SessionEventKind, SessionId, SessionRef, SessionStatus, StateChange,
    StopReason, ToolCallPatch, Topic, Workspace, WorkspaceId, Worktree,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::agents::AgentLauncher;
use crate::store::Store;
use crate::{acp, folders, git};

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

/// What the rest of the daemon can ask a running agent to do.
pub enum SessionCommand {
    Prompt(String),
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
}

struct Inner {
    store: Store,
    state: DaemonState,
    live: HashMap<SessionId, LiveSession>,
    /// Agents told to stop because their tab closed, which shutdown still has to wait for.
    closing: Vec<JoinHandle<()>>,
    last_seq: HashMap<SessionId, u64>,
    signals: broadcast::Sender<Signal>,
}

pub struct Core {
    inner: Mutex<Inner>,
    signals: broadcast::Sender<Signal>,
    data_dir: PathBuf,
    agents: BTreeMap<String, AgentLauncher>,
    /// Stops two sessions from installing the same adapter at the same time.
    adapter_install: tokio::sync::Mutex<()>,
    shutting_down: AtomicBool,
}

impl Core {
    pub fn open(
        data_dir: &Path,
        machine_name: String,
        agents: BTreeMap<String, AgentLauncher>,
        event_buffer: usize,
    ) -> Result<Arc<Self>> {
        let store = Store::open(&data_dir.join("tectr.db"))?;
        let machine = Machine {
            id: store.machine_id()?,
            name: machine_name,
        };
        let state = store.load_state(machine)?;
        let last_seq = store.last_seqs()?.into_iter().collect();
        let (signals, _) = broadcast::channel(event_buffer);
        let mut inner = Inner {
            store,
            state,
            live: HashMap::new(),
            closing: Vec::new(),
            last_seq,
            signals: signals.clone(),
        };
        inner.recover()?;
        Ok(Arc::new(Self {
            inner: Mutex::new(inner),
            signals,
            data_dir: data_dir.to_owned(),
            agents,
            adapter_install: tokio::sync::Mutex::new(()),
            shutting_down: AtomicBool::new(false),
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
            Command::DeleteTopic { topic_id, force } => self.delete_topic(&topic_id, force),
            Command::StartSession {
                topic_id,
                worktree_id,
                agent,
                mode,
                model,
                effort,
            } => self.start_session(topic_id, worktree_id, agent, mode, model, effort),
            Command::SendPrompt { session_id, text } => self.send_prompt(&session_id, text),
            Command::CancelTurn { session_id } => self.cancel_turn(&session_id),
            Command::AnswerApproval {
                approval_id,
                option_id,
            } => self.answer_approval(&approval_id, &option_id),
            Command::DetectProviders => Ok(self.detect_providers().await),
            Command::AddProvider { agent } => self.add_provider(&agent).await,
            Command::RemoveProvider { agent } => self.remove_provider(&agent),
            Command::SetProviderDefaults {
                agent,
                model,
                effort,
            } => self.set_provider_defaults(&agent, model, effort),
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
            Command::SetDraft { session_id, text } => self.set_draft(session_id, text),
            Command::CloseSession { session_id, force } => self.close_session(&session_id, force),
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
            git::add_worktree(&repo, &path, &branch)
                .await
                .map_err(internal)?;
            // macOS temp directories are symlinks. Agents should see the real path.
            let path = tokio::fs::canonicalize(&path)
                .await
                .map_err(|e| internal(e.into()))?;
            (path, branch)
        };

        let topic = Topic {
            id: topic_id.as_str().into(),
            name,
            worktrees: vec![Worktree {
                id: crate::new_id().as_str().into(),
                path: path.display().to_string(),
                branch,
            }],
            sessions: vec![],
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

    /// Stops the topic's agents and removes it straight away. Its worktrees are removed behind
    /// that, because agents can take seconds to exit and have to be gone first.
    fn delete_topic(
        &self,
        topic_id: &protocol::TopicId,
        force: bool,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        let (repo, topic) = inner
            .state
            .workspaces
            .iter()
            .find_map(|workspace| {
                let topic = workspace.topics.iter().find(|t| &t.id == topic_id)?;
                Some((PathBuf::from(&workspace.path), topic.clone()))
            })
            .ok_or_else(|| not_found("topic", topic_id))?;
        if !force && topic.sessions.iter().any(|s| inner.is_running(s)) {
            return Err(CommandError {
                code: ErrorCode::Busy,
                message: "an agent in this topic is still running, so deleting it needs confirming"
                    .to_owned(),
            });
        }
        inner
            .store
            .delete_topic(topic_id, now_ms())
            .map_err(internal)?;

        let mut agents = vec![];
        for session in &topic.sessions {
            agents.extend(inner.stop_agent(&session.id));
            // The topic is already gone from the database, so this can't turn back now.
            if let Err(error) = inner.set_draft(&session.id, String::new()) {
                tracing::error!(id = %session.id, "failed to clear a draft: {error:#}");
            }
        }
        inner.emit(StateChange::TopicRemoved {
            topic_id: topic_id.clone(),
        });
        let cleanup = tokio::spawn(remove_worktrees(repo, topic.worktrees, agents));
        inner.keep_closing(cleanup);
        Ok(CommandOutput::Done)
    }

    fn start_session(
        self: &Arc<Self>,
        topic_id: protocol::TopicId,
        worktree_id: protocol::WorktreeId,
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
        let topic = inner
            .state
            .topic(&topic_id)
            .ok_or_else(|| not_found("topic", &topic_id))?;
        if !topic.worktrees.iter().any(|w| w.id == worktree_id) {
            return Err(not_found("worktree in that topic", &worktree_id));
        }

        let session = Session {
            id: SessionId(crate::new_id()),
            worktree_id,
            agent,
            mode,
            model,
            effort,
            status: SessionStatus::Starting,
            title: None,
        };
        inner
            .store
            .insert_session(&topic_id, &session)
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
            .state
            .topics()
            .flat_map(|topic| &topic.worktrees)
            .find(|worktree| worktree.id == session.worktree_id)
            .context("the session's worktree no longer exists")?
            .path
            .clone();
        let spawn = acp::AgentSpawn {
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
            },
        );
        Ok(())
    }

    fn send_prompt(
        self: &Arc<Self>,
        session_id: &SessionId,
        text: String,
    ) -> Result<CommandOutput, CommandError> {
        if text.trim().is_empty() {
            return Err(invalid("the prompt is empty"));
        }
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

        inner.append(
            session_id,
            SessionEventKind::UserPrompt { text: text.clone() },
        );
        inner.set_status(session_id, SessionStatus::Working);
        let live = inner.live.get(session_id).expect("made live just above");
        live.commands
            .send(SessionCommand::Prompt(text))
            .map_err(|_| internal(anyhow::anyhow!("the agent has just exited")))?;
        // It's been sent, so it's no longer a draft. A failure above leaves it in place.
        if let Err(error) = inner.set_draft(session_id, String::new()) {
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

    fn set_draft(
        &self,
        session_id: SessionId,
        text: String,
    ) -> Result<CommandOutput, CommandError> {
        let mut inner = self.lock();
        if inner.state.session(&session_id).is_none() {
            return Err(not_found("session", &session_id));
        }
        inner.set_draft(&session_id, text).map_err(internal)?;
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
        inner
            .set_draft(session_id, String::new())
            .map_err(internal)?;
        Ok(CommandOutput::Done)
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
    }

    /// The agent process is gone, either as asked or unexpectedly.
    pub fn agent_exited(&self, id: &SessionId, result: Result<()>) {
        let mut inner = self.lock();
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
            tasks
        };
        for task in tasks {
            // The SDK ends a stubborn agent within about three seconds.
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
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
    fn set_draft(&mut self, session_id: &SessionId, text: String) -> Result<()> {
        let mut drafts = self.state.drafts.clone();
        if text.is_empty() {
            drafts.remove(session_id);
        } else {
            drafts.insert(session_id.clone(), text.clone());
        }
        if drafts != self.state.drafts {
            self.store.set_drafts(&drafts)?;
            self.emit(StateChange::DraftChanged {
                session_id: session_id.clone(),
                text,
            });
        }
        Ok(())
    }

    fn emit(&mut self, change: StateChange) {
        self.state.apply(&change);
        let _ = self.signals.send(Signal::State(Arc::new(change)));
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

/// A topic at the repo root has the repo's own folder as its worktree, which is never removed. A
/// worktree git won't remove stays on disk with its branch, because it holds uncommitted work.
async fn remove_worktrees(repo: PathBuf, worktrees: Vec<Worktree>, agents: Vec<JoinHandle<()>>) {
    for agent in agents {
        // The SDK ends a stubborn agent within about three seconds.
        let _ = tokio::time::timeout(Duration::from_secs(5), agent).await;
    }
    for worktree in worktrees {
        let path = Path::new(&worktree.path);
        if path == repo {
            continue;
        }
        if let Err(error) = git::remove_worktree(&repo, path, &worktree.branch).await {
            tracing::warn!(path = worktree.path, "keeping a worktree: {error:#}");
        }
    }
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

    /// Simulates a daemon that died without shutting down: its database says
    /// agents are working and waiting, but their processes are gone.
    #[tokio::test]
    async fn startup_after_a_crash_clears_what_could_not_have_survived() {
        let dir = tempfile::tempdir().unwrap();
        let session_in = |id: &str, status| Session {
            id: id.into(),
            worktree_id: "wt".into(),
            agent: "a".into(),
            mode: PermissionMode::Auto,
            model: None,
            effort: None,
            status,
            title: None,
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
                        worktrees: vec![Worktree {
                            id: "wt".into(),
                            path: "/wt".into(),
                            branch: "b".into(),
                        }],
                        sessions: vec![],
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

        let core = Core::open(dir.path(), "m".to_owned(), BTreeMap::new(), 16).unwrap();
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
        let core = Core::open(dir.path(), "m".to_owned(), BTreeMap::new(), 16).unwrap();
        assert_eq!(
            core.events_after(&"waiting".into(), 0, 10).unwrap().len(),
            2
        );
    }
}
