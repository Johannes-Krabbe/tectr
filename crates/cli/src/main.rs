//! A minimal client that tests the engine before any UI exists. It holds no
//! logic of its own beyond parsing arguments and printing.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use engine::{Connection, Device, Engine};
use protocol::{
    Approval, Command, CommandOutput, DEFAULT_PORT, MachineId, PermissionMode, SessionEvent,
    SessionEventKind, SessionRef, SessionStatus, ToolCallPatch,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast::error::RecvError;

/// Run, watch, and steer coding agents across your machines.
#[derive(Parser)]
#[command(name = "tectr", version)]
struct Cli {
    /// Daemon URLs. Repeat the flag or separate them with commas.
    #[arg(
        long = "daemon",
        env = "TECTR_DAEMONS",
        value_delimiter = ',',
        global = true
    )]
    daemons: Vec<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show every device, workspace, topic, session, and pending approval.
    Status,
    /// Add an existing git repository as a workspace.
    AddWorkspace {
        path: String,
        /// The machine to add it on, by name or ID prefix. Needed only with
        /// more than one daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Create a topic with a fresh branch and worktree.
    NewTopic { workspace: String, name: String },
    /// Start an agent in a topic's first worktree.
    NewSession {
        topic: String,
        #[arg(long, default_value = "claude")]
        agent: String,
        #[arg(long, value_enum, default_value_t = ModeArg::Auto)]
        mode: ModeArg,
        /// The agent's own model name, such as `haiku`.
        #[arg(long)]
        model: Option<String>,
    },
    /// Send a prompt and return without waiting for the agent.
    Prompt { session: String, text: String },
    /// Print a session's history and follow it live.
    Watch {
        session: String,
        /// Exit when the agent has nothing left to do.
        #[arg(long)]
        until_done: bool,
    },
    /// Follow a session and talk to it. Type a prompt, or the number of an
    /// option when the agent asks for permission.
    Chat { session: String },
    /// Answer a pending approval with an option's number or ID.
    Answer { approval: String, option: String },
    /// Interrupt the current turn.
    Cancel { session: String },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Manual,
    Auto,
    Yolo,
}

impl From<ModeArg> for PermissionMode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Manual => Self::Manual,
            ModeArg::Auto => Self::Auto,
            ModeArg::Yolo => Self::Yolo,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let engine = Engine::new("tectr-cli");
    if cli.daemons.is_empty() {
        engine.add_daemon(&format!("ws://127.0.0.1:{DEFAULT_PORT}"));
    }
    for url in &cli.daemons {
        engine.add_daemon(url);
    }
    engine.wait_until_settled().await;

    match cli.command {
        Cmd::Status => print_status(&engine),
        Cmd::AddWorkspace { path, on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            let output = engine
                .request(
                    &machine,
                    Command::AddWorkspace {
                        path: absolute(&path)?,
                    },
                )
                .await?;
            if let CommandOutput::Workspace { workspace } = output {
                println!("workspace {} {}", short(&workspace.id.0), workspace.path);
            }
        }
        Cmd::NewTopic { workspace, name } => {
            let (machine, workspace_id) = resolve(
                &engine,
                &workspace,
                |state| state.workspaces.iter().map(|w| w.id.clone()).collect(),
                |w| &w.0,
            )?;
            let output = engine
                .request(&machine, Command::CreateTopic { workspace_id, name })
                .await?;
            if let CommandOutput::Topic { topic, .. } = output {
                println!("topic {} {}", short(&topic.id.0), topic.name);
                for worktree in &topic.worktrees {
                    println!("  worktree {} {}", worktree.branch, worktree.path);
                }
            }
        }
        Cmd::NewSession {
            topic,
            agent,
            mode,
            model,
        } => {
            let (machine, topic_id) = resolve(
                &engine,
                &topic,
                |state| state.topics().map(|t| t.id.clone()).collect(),
                |t| &t.0,
            )?;
            let worktree_id = engine
                .devices()
                .iter()
                .filter_map(|device| device.state.as_ref())
                .find_map(|state| state.topic(&topic_id))
                .and_then(|topic| topic.worktrees.first())
                .map(|worktree| worktree.id.clone())
                .context("this topic has no worktree")?;
            let output = engine
                .request(
                    &machine,
                    Command::StartSession {
                        topic_id,
                        worktree_id,
                        agent,
                        mode: mode.into(),
                        model,
                    },
                )
                .await?;
            if let CommandOutput::Session { session, .. } = output {
                println!("session {} {}", short(&session.id.0), session.agent);
            }
        }
        Cmd::Prompt { session, text } => {
            let session = resolve_session(&engine, &session)?;
            engine
                .request(
                    &session.machine_id,
                    Command::SendPrompt {
                        session_id: session.session_id,
                        text,
                    },
                )
                .await?;
        }
        Cmd::Watch {
            session,
            until_done,
        } => {
            let session = resolve_session(&engine, &session)?;
            watch(&engine, &session, until_done).await?;
        }
        Cmd::Chat { session } => {
            let session = resolve_session(&engine, &session)?;
            chat(&engine, &session).await?;
        }
        Cmd::Answer { approval, option } => {
            let (machine, approval) = find_approval(&engine, &approval)?;
            let option_id = option_id(&approval, &option)?;
            engine
                .request(
                    &machine,
                    Command::AnswerApproval {
                        approval_id: approval.id,
                        option_id,
                    },
                )
                .await?;
        }
        Cmd::Cancel { session } => {
            let session = resolve_session(&engine, &session)?;
            engine
                .request(
                    &session.machine_id,
                    Command::CancelTurn {
                        session_id: session.session_id,
                    },
                )
                .await?;
        }
    }
    Ok(())
}

/// Relative paths mean relative to here, which is right while daemons are local.
fn absolute(path: &str) -> Result<String> {
    if Path::new(path).is_absolute() {
        return Ok(path.to_owned());
    }
    let full = std::env::current_dir()?.join(path);
    Ok(std::fs::canonicalize(&full)
        .unwrap_or(full)
        .display()
        .to_string())
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn online_devices(engine: &Engine) -> Vec<Device> {
    engine
        .devices()
        .into_iter()
        .filter(|device| device.connection == Connection::Online)
        .collect()
}

fn choose_machine(engine: &Engine, wanted: Option<&str>) -> Result<MachineId> {
    let machines: Vec<_> = online_devices(engine)
        .into_iter()
        .filter_map(|device| device.state.map(|state| state.machine))
        .collect();
    if machines.is_empty() {
        bail!("no daemon is reachable\n{}", describe_failures(engine));
    }
    match wanted {
        Some(wanted) => machines
            .into_iter()
            .find(|machine| machine.name == wanted || machine.id.0.starts_with(wanted))
            .map(|machine| machine.id)
            .with_context(|| format!("no online machine matches {wanted}")),
        None if machines.len() == 1 => Ok(machines[0].id.clone()),
        None => bail!("more than one machine is online. Choose one with --on"),
    }
}

fn describe_failures(engine: &Engine) -> String {
    engine
        .devices()
        .iter()
        .map(|device| match &device.connection {
            Connection::Offline { reason } => format!("  {}: {reason}", device.url),
            other => format!("  {}: {other:?}", device.url),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Finds the one ID starting with `prefix`, on any daemon. This works for
/// offline machines too, whose commands then fail with a clear error.
fn resolve<T: Clone>(
    engine: &Engine,
    prefix: &str,
    ids: impl Fn(&protocol::DaemonState) -> Vec<T>,
    text: impl Fn(&T) -> &str,
) -> Result<(MachineId, T)> {
    let mut matches = vec![];
    for device in engine.devices() {
        if let Some(state) = &device.state {
            for id in ids(state) {
                if text(&id).starts_with(prefix) {
                    matches.push((state.machine.id.clone(), id));
                }
            }
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 if engine.devices().iter().all(|d| d.state.is_none()) => {
            bail!("no daemon is reachable\n{}", describe_failures(engine))
        }
        0 => bail!("nothing matches {prefix}"),
        count => bail!("{prefix} is ambiguous: {count} matches"),
    }
}

fn resolve_session(engine: &Engine, prefix: &str) -> Result<SessionRef> {
    let (machine_id, session_id) = resolve(
        engine,
        prefix,
        |state| state.sessions().map(|session| session.id.clone()).collect(),
        |id| &id.0,
    )?;
    Ok(SessionRef {
        machine_id,
        session_id,
    })
}

fn find_approval(engine: &Engine, prefix: &str) -> Result<(MachineId, Approval)> {
    let mut matches: Vec<_> = engine
        .approvals()
        .into_iter()
        .filter(|(_, approval)| approval.id.0.starts_with(prefix))
        .collect();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => bail!("no pending approval matches {prefix}"),
        count => bail!("{prefix} is ambiguous: {count} matches"),
    }
}

/// Accepts a 1-based option number or an option's ID.
fn option_id(approval: &Approval, choice: &str) -> Result<String> {
    if let Ok(number) = choice.parse::<usize>()
        && let Some(option) = number.checked_sub(1).and_then(|i| approval.options.get(i))
    {
        return Ok(option.id.clone());
    }
    approval
        .options
        .iter()
        .find(|option| option.id == choice)
        .map(|option| option.id.clone())
        .with_context(|| format!("this approval has no option {choice}"))
}

fn print_status(engine: &Engine) {
    for device in engine.devices() {
        let connection = match &device.connection {
            Connection::Connecting => "connecting".to_owned(),
            Connection::Online => "online".to_owned(),
            Connection::Offline { .. } => "offline".to_owned(),
            Connection::Incompatible {
                daemon_min,
                daemon_max,
            } => {
                format!("incompatible: the daemon speaks protocol {daemon_min}-{daemon_max}")
            }
        };
        let Some(state) = &device.state else {
            println!("{} ({connection})", device.url);
            continue;
        };
        println!("{} ({connection}) {}", state.machine.name, device.url);
        for workspace in &state.workspaces {
            println!(
                "  workspace {} {} {}",
                short(&workspace.id.0),
                workspace.name,
                workspace.path
            );
            for topic in &workspace.topics {
                println!("    topic {} {}", short(&topic.id.0), topic.name);
                for worktree in &topic.worktrees {
                    println!("      worktree {} {}", worktree.branch, worktree.path);
                }
                for session in &topic.sessions {
                    println!(
                        "      session {} {} {:?} {:?} {}",
                        short(&session.id.0),
                        session.agent,
                        session.mode,
                        session.status,
                        session.title.as_deref().unwrap_or(""),
                    );
                }
            }
        }
    }
    let approvals = engine.approvals();
    if !approvals.is_empty() {
        println!("approvals");
        for (_, approval) in approvals {
            print_approval(&approval);
        }
    }
}

fn print_approval(approval: &Approval) {
    println!(
        "[permission {}] session {}: {}",
        short(&approval.id.0),
        short(&approval.session_id.0),
        approval
            .tool_call
            .title
            .as_deref()
            .unwrap_or("(untitled tool call)"),
    );
    for (index, option) in approval.options.iter().enumerate() {
        println!("  {}) {}", index + 1, option.name);
    }
}

/// Prints a session's events as they arrive.
#[derive(Default)]
struct Printer {
    printed: u64,
    in_message: bool,
    /// Tool updates often carry only a status, so remember each tool's title.
    tool_titles: HashMap<String, String>,
}

impl Printer {
    fn print_new(&mut self, engine: &Engine, session: &SessionRef) {
        for event in engine.events_after(session, self.printed) {
            self.print(&event);
            self.printed = event.seq;
        }
    }

    fn finish(&mut self) {
        if self.in_message {
            println!();
            self.in_message = false;
        }
    }

    fn print_tool(&mut self, id: &str, tool: &ToolCallPatch) {
        if let Some(title) = &tool.title {
            self.tool_titles.insert(id.to_owned(), title.clone());
        }
        let title = self.tool_titles.get(id).map_or("", String::as_str);
        match tool.status {
            Some(status) => println!("[tool] {title} ({status:?})"),
            None if tool.title.is_some() => println!("[tool] {title}"),
            None => {}
        }
    }

    fn print(&mut self, event: &SessionEvent) {
        let streams = matches!(
            event.kind,
            SessionEventKind::AgentMessage { .. } | SessionEventKind::AgentThought { .. }
        );
        if !streams {
            self.finish();
        }
        match &event.kind {
            SessionEventKind::AgentMessage { text } | SessionEventKind::AgentThought { text } => {
                print!("{text}");
                let _ = std::io::stdout().flush();
                self.in_message = true;
            }
            SessionEventKind::StatusChanged { status } => println!("[status] {status:?}"),
            SessionEventKind::AgentReady {
                name,
                version,
                current_mode,
                ..
            } => println!(
                "[agent] {name} {} in mode {}",
                version.as_deref().unwrap_or(""),
                current_mode.as_deref().unwrap_or("unknown"),
            ),
            SessionEventKind::UserPrompt { text } => println!("> {text}"),
            SessionEventKind::ToolCall { id, tool }
            | SessionEventKind::ToolCallUpdate { id, tool } => self.print_tool(id, tool),
            SessionEventKind::PermissionRequested { approval } => print_approval(approval),
            SessionEventKind::PermissionResolved { outcome, .. } => {
                println!("[permission] {outcome:?}");
            }
            SessionEventKind::TurnEnded { stop_reason } => println!("[turn ended] {stop_reason:?}"),
            SessionEventKind::Error { message } => println!("[error] {message}"),
            SessionEventKind::Unknown => {}
        }
    }
}

fn session_status(engine: &Engine, session: &SessionRef) -> Option<SessionStatus> {
    engine
        .devices()
        .iter()
        .filter_map(|device| device.state.as_ref())
        .filter(|state| state.machine.id == session.machine_id)
        .find_map(|state| state.session(&session.session_id))
        .map(|session| session.status)
}

/// Whether the session has nothing left to do, going by both its status and
/// its history. The status comes with the snapshot and can be ahead of the
/// events, so this waits for the event that matches it.
fn is_done(engine: &Engine, session: &SessionRef) -> bool {
    let status = session_status(engine, session);
    let finished = matches!(
        status,
        Some(
            SessionStatus::Ready
                | SessionStatus::Done
                | SessionStatus::Failed
                | SessionStatus::Stopped
        )
    );
    let last_event = engine
        .events_after(session, 0)
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            SessionEventKind::StatusChanged { status } => Some(status),
            _ => None,
        });
    finished && last_event == status
}

async fn watch(engine: &Engine, session: &SessionRef, until_done: bool) -> Result<()> {
    let mut updates = engine.updates();
    engine.open_session(session);
    let mut printer = Printer::default();
    loop {
        // Check before printing, so nothing that arrives in between is left
        // unprinted on exit.
        let done = until_done && is_done(engine, session);
        printer.print_new(engine, session);
        if done {
            printer.finish();
            return Ok(());
        }
        match updates.recv().await {
            Ok(_) | Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => return Ok(()),
        }
    }
}

async fn chat(engine: &Engine, session: &SessionRef) -> Result<()> {
    let mut updates = engine.updates();
    engine.open_session(session);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut printer = Printer::default();
    printer.print_new(engine, session);

    loop {
        tokio::select! {
            update = updates.recv() => match update {
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    printer.print_new(engine, session);
                }
                Err(RecvError::Closed) => return Ok(()),
            },
            line = lines.next_line() => {
                let Some(line) = line? else { return Ok(()) };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let pending = engine.approvals().into_iter().find(|(machine, approval)| {
                    machine == &session.machine_id && approval.session_id == session.session_id
                });
                let command = match pending {
                    Some((_, approval)) => match option_id(&approval, line) {
                        Ok(option_id) => Command::AnswerApproval { approval_id: approval.id, option_id },
                        Err(error) => {
                            println!("[error] {error}");
                            continue;
                        }
                    },
                    None => Command::SendPrompt {
                        session_id: session.session_id.clone(),
                        text: line.to_owned(),
                    },
                };
                if let Err(error) = engine.request(&session.machine_id, command).await {
                    println!("[error] {error}");
                }
            }
        }
    }
}
