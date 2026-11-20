//! A minimal client that tests the engine before any UI exists. It holds no
//! logic of its own beyond parsing arguments and printing.

mod mcp;
mod terminal;

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use engine::{ClientStore, Connection, Device, Engine, Remote};
use protocol::{
    Approval, ArchiveRisk, ArchivedTopic, ClosedWorkspace, Command, CommandOutput, Conversation,
    MachineId, MoveTarget, PermissionMode, SessionEvent, SessionEventKind, SessionRef,
    SessionStatus, ToolCallPatch, TopicId, WorkspaceId,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::broadcast::error::RecvError;

/// Run, watch, and steer coding agents across your machines.
#[derive(Parser)]
#[command(name = "tectr", version)]
struct Cli {
    /// Daemon URLs. Repeat the flag or separate them with commas. Without it, the CLI
    /// uses this machine's daemon and every daemon it paired with.
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
        /// Make a new folder there instead, with a fresh git repository in it.
        #[arg(long)]
        new: bool,
        /// The machine to add it on, by name or ID prefix. Needed only with
        /// more than one daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Clone a repository into a new folder and add it as a workspace.
    CloneWorkspace {
        /// The repository's address, as git clone takes it.
        url: String,
        /// Where it goes. Defaults to a folder named after the repository, here.
        path: Option<String>,
        /// The machine to clone it on, by name or ID prefix. Needed only with
        /// more than one daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Choose a device's name. An empty name goes back to its hostname.
    RenameDevice {
        name: String,
        /// The machine to rename, by name or ID prefix. Needed only with
        /// more than one daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Choose whether a device stays awake while an agent is starting or working, so a
    /// locked screen doesn't pause it. Closing a laptop's lid still sleeps.
    KeepAwake {
        #[arg(value_enum)]
        state: Switch,
        /// The machine to change, by name or ID prefix. Needed only with more than one
        /// daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Stop a workspace's agents and take it out of the app, keeping its topics, tabs and
    /// worktrees for when it's opened again. Without --force, nothing happens while an agent
    /// in it is running.
    CloseWorkspace {
        /// The workspace, by name or ID prefix.
        workspace: String,
        /// Close it even if that stops an agent.
        #[arg(long)]
        force: bool,
    },
    /// List closed workspaces, newest first.
    Closed,
    /// Bring a closed workspace back with its topics and tabs.
    OpenWorkspace {
        /// The closed workspace, by name or ID prefix.
        workspace: String,
    },
    /// Forget a closed workspace: its topics, tabs and transcripts, and the worktrees and
    /// branches made for it. The repo itself stays. Without --force, nothing happens if that
    /// would lose uncommitted work or unpushed commits, and what it would is printed instead.
    RemoveWorkspace {
        /// The closed workspace, by name or ID prefix.
        workspace: String,
        /// Remove it whatever that loses.
        #[arg(long)]
        force: bool,
    },
    /// Create a topic with a fresh branch and worktree.
    NewTopic { workspace: String, name: String },
    /// Choose a topic's name.
    RenameTopic { topic: String, name: String },
    /// Stop a topic's agents, take it out of the app and remove its worktrees and the branches
    /// made for them. Without --force, nothing happens if that would stop an agent or lose
    /// uncommitted work or unpushed commits, and what it would is printed instead.
    ArchiveTopic {
        topic: String,
        /// Archive it whatever that loses.
        #[arg(long)]
        force: bool,
    },
    /// List archived topics, newest first.
    Archived,
    /// Bring an archived topic back with its tabs, making its folder again.
    RestoreTopic { topic: String },
    /// Move every tab of a topic to another folder: `new` for a fresh worktree from where the
    /// topic is now, `root` for the repo itself, or a worktree the topic was in, by branch or ID
    /// prefix.
    MoveTopic {
        topic: String,
        to: String,
        /// Move it even if an agent is still running, which stops it.
        #[arg(long)]
        force: bool,
    },
    /// Start an agent in a topic's folder.
    NewSession {
        /// The topic, by name or ID prefix. Inside a tectr agent, its own topic.
        #[arg(env = "TECTR_TOPIC")]
        topic: String,
        #[arg(long, default_value = "claude")]
        agent: String,
        #[arg(long, value_enum, default_value_t = ModeArg::Auto)]
        mode: ModeArg,
        /// The agent's own model name, such as `haiku`.
        #[arg(long)]
        model: Option<String>,
    },
    /// List the Claude Code and Codex conversations that can be imported, newest first.
    Conversations {
        /// The machine to list, by name or ID prefix. Needed only with more than one daemon.
        #[arg(long)]
        on: Option<String>,
    },
    /// Open a Claude Code or Codex conversation as a new tab in a topic, with its history.
    /// Its first prompt carries it on where it left off.
    Import {
        topic: String,
        /// The conversation's ID or a prefix of it, as `tectr conversations` lists it.
        conversation: String,
    },
    /// Open a new tab in the session's topic that carries on from its latest finished answer,
    /// with the agent remembering everything up to there. Only agents that can copy their
    /// sessions can do this.
    Fork {
        /// The session's ID or a prefix of it. Inside a tectr agent, itself.
        #[arg(env = "TECTR_SESSION")]
        session: String,
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
    /// Stop the agent and remove its tab.
    Close {
        session: String,
        /// Stop it even if it's still running.
        #[arg(long)]
        force: bool,
    },
    /// Open terminals in a topic's folder, read what they show and type into them.
    Terminal {
        #[command(subcommand)]
        command: terminal::TerminalCmd,
    },
    /// Manage the MCP servers agents get, device-wide and per topic.
    Mcp {
        #[command(subcommand)]
        command: mcp::McpCmd,
    },
    /// Show a code that lets one more machine in. Run it on the machine to reach, then
    /// `tectr add-device <code>` on the one that wants in, and type the digits it shows here.
    Pair,
    /// Pair with a device on another machine using the code `tectr pair` showed there. It
    /// shows digits to type there.
    AddDevice { code: String },
    /// Stop reaching a paired device, and take this machine off its allowed list if it's online.
    RemoveDevice {
        /// The device's name, machine ID prefix or URL.
        device: String,
    },
    /// Take a machine off this device's allowed list, disconnecting it.
    Unpair {
        /// The client's name or key prefix, as `tectr status` lists it.
        client: String,
        /// The device to change, by name or ID prefix. Needed only with more than one daemon.
        #[arg(long)]
        on: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Switch {
    On,
    Off,
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
    let store = ClientStore::default_dir().map(ClientStore::new);
    let engine = match &store {
        Some(store) => Engine::with_remote(
            "tectr-cli",
            Remote {
                key: store.key()?,
                network: engine::Network::Public,
            },
        ),
        None => Engine::new("tectr-cli"),
    };
    if cli.daemons.is_empty() {
        engine.add_daemon(&protocol::local_daemon_url());
        for url in store.iter().flat_map(ClientStore::daemons) {
            engine.add_daemon(&url);
        }
    }
    for url in &cli.daemons {
        engine.add_daemon(url);
    }
    engine.wait_until_settled().await;

    match cli.command {
        Cmd::Status => print_status(&engine),
        Cmd::AddWorkspace { path, new, on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            let path = absolute(&path)?;
            let command = if new {
                Command::CreateWorkspace { path }
            } else {
                Command::AddWorkspace { path }
            };
            let output = engine.request(&machine, command).await?;
            if let CommandOutput::Workspace { workspace } = output {
                println!("workspace {} {}", short(&workspace.id.0), workspace.path);
            }
        }
        Cmd::CloneWorkspace { url, path, on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            let path = match path {
                Some(path) => absolute(&path)?,
                None => absolute(repo_name(&url).context("name the folder to clone into")?)?,
            };
            eprintln!("cloning {url} into {path}");
            let output = engine
                .request(&machine, Command::CloneWorkspace { url, path })
                .await?;
            if let CommandOutput::Workspace { workspace } = output {
                println!("workspace {} {}", short(&workspace.id.0), workspace.path);
            }
        }
        Cmd::RenameDevice { name, on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            engine
                .request(&machine, Command::RenameMachine { name })
                .await?;
        }
        Cmd::KeepAwake { state, on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            let mut settings = online_devices(&engine)
                .into_iter()
                .filter_map(|device| device.state)
                .find(|state| state.machine.id == machine)
                .map(|state| state.settings)
                .unwrap_or_default();
            settings.keep_awake = matches!(state, Switch::On);
            engine
                .request(&machine, Command::SetDeviceSettings { settings })
                .await?;
        }
        Cmd::CloseWorkspace { workspace, force } => {
            let (machine, workspace_id) = resolve_workspace(&engine, &workspace)?;
            engine
                .request(
                    &machine,
                    Command::CloseWorkspace {
                        workspace_id,
                        force,
                    },
                )
                .await?;
        }
        Cmd::Closed => {
            for (_, machine, workspace) in closed_workspaces(&engine).await? {
                println!(
                    "{machine} workspace {} {} {}",
                    short(&workspace.id.0),
                    workspace.name,
                    workspace.path
                );
            }
        }
        Cmd::OpenWorkspace { workspace } => {
            let (machine, closed) = resolve_closed(&engine, &workspace).await?;
            engine
                .request(
                    &machine,
                    Command::OpenWorkspace {
                        workspace_id: closed.id,
                    },
                )
                .await?;
        }
        Cmd::RemoveWorkspace { workspace, force } => {
            let (machine, closed) = resolve_closed(&engine, &workspace).await?;
            let output = engine
                .request(
                    &machine,
                    Command::RemoveWorkspace {
                        workspace_id: closed.id,
                        force,
                    },
                )
                .await?;
            if let CommandOutput::ArchiveRisk { risk } = output {
                print_risk(&risk);
                bail!("removing would lose this. Pass --force to remove anyway");
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
                .request(
                    &machine,
                    Command::CreateTopic {
                        workspace_id,
                        name: Some(name),
                        at_root: false,
                    },
                )
                .await?;
            if let CommandOutput::Topic { topic, .. } = output {
                println!("topic {} {}", short(&topic.id.0), topic.name);
                for worktree in &topic.worktrees {
                    println!("  worktree {} {}", worktree.branch, worktree.path);
                }
            }
        }
        Cmd::RenameTopic { topic, name } => {
            let (machine, topic_id) = resolve_topic(&engine, &topic)?;
            engine
                .request(&machine, Command::RenameTopic { topic_id, name })
                .await?;
        }
        Cmd::ArchiveTopic { topic, force } => {
            let (machine, topic_id) = resolve_topic(&engine, &topic)?;
            let output = engine
                .request(&machine, Command::ArchiveTopic { topic_id, force })
                .await?;
            if let CommandOutput::ArchiveRisk { risk } = output {
                print_risk(&risk);
                bail!("archiving would lose this. Pass --force to archive anyway");
            }
        }
        Cmd::Archived => {
            for (_, machine, topic) in archived_topics(&engine).await? {
                println!("{machine} topic {} {}", short(&topic.id.0), topic.name);
            }
        }
        Cmd::RestoreTopic { topic } => {
            let mut matches: Vec<_> = archived_topics(&engine)
                .await?
                .into_iter()
                .filter(|(_, _, archived)| archived.id.0.starts_with(&topic))
                .collect();
            let (machine, _, archived) = match matches.len() {
                1 => matches.remove(0),
                0 => bail!("no archived topic matches {topic}"),
                count => bail!("{topic} is ambiguous: {count} matches"),
            };
            engine
                .request(
                    &machine,
                    Command::RestoreTopic {
                        topic_id: archived.id,
                    },
                )
                .await?;
        }
        Cmd::MoveTopic { topic, to, force } => {
            let (machine, topic_id) = resolve_topic(&engine, &topic)?;
            let to = match to.as_str() {
                "new" => MoveTarget::NewWorktree,
                "root" => MoveTarget::Root,
                wanted => {
                    let worktrees: Vec<_> = engine
                        .devices()
                        .iter()
                        .filter_map(|device| device.state.as_ref())
                        .find_map(|state| state.topic(&topic_id))
                        .map(|topic| topic.worktrees.clone())
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|w| w.branch == wanted || w.id.0.starts_with(wanted))
                        .collect();
                    match worktrees.as_slice() {
                        [worktree] => MoveTarget::Worktree {
                            worktree_id: worktree.id.clone(),
                        },
                        [] => bail!("this topic has no worktree matching {wanted}"),
                        _ => bail!("{wanted} is ambiguous"),
                    }
                }
            };
            engine
                .request(
                    &machine,
                    Command::MoveTopic {
                        topic_id,
                        to,
                        force,
                    },
                )
                .await?;
        }
        Cmd::NewSession {
            topic,
            agent,
            mode,
            model,
        } => {
            let (machine, topic_id) = resolve_topic(&engine, &topic)?;
            let output = engine
                .request(
                    &machine,
                    Command::StartSession {
                        topic_id,
                        agent,
                        mode: mode.into(),
                        model,
                        effort: None,
                    },
                )
                .await?;
            if let CommandOutput::Session { session, .. } = output {
                println!("session {} {}", short(&session.id.0), session.agent);
            }
        }
        Cmd::Conversations { on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            for c in conversations(&engine, &machine).await? {
                println!("{} {} {}  {}", c.agent, short(&c.id), c.title, c.cwd);
            }
        }
        Cmd::Import {
            topic,
            conversation,
        } => {
            let (machine, topic_id) = resolve_topic(&engine, &topic)?;
            let mut matches: Vec<_> = conversations(&engine, &machine)
                .await?
                .into_iter()
                .filter(|c| c.id.starts_with(&conversation))
                .collect();
            let found = match matches.len() {
                1 => matches.remove(0),
                0 => bail!("no conversation on the topic's device matches {conversation}"),
                count => bail!("{conversation} is ambiguous: {count} matches"),
            };
            let output = engine
                .request(
                    &machine,
                    Command::ImportConversation {
                        topic_id,
                        agent: found.agent,
                        id: found.id,
                    },
                )
                .await?;
            if let CommandOutput::Session { session, .. } = output {
                println!("session {} {}", short(&session.id.0), session.agent);
            }
        }
        Cmd::Fork { session } => {
            let session = resolve_session(&engine, &session)?;
            let output = engine
                .request(
                    &session.machine_id,
                    Command::ForkSession {
                        session_id: session.session_id,
                        after_seq: None,
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
                        images: vec![],
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
        Cmd::Close { session, force } => {
            let session = resolve_session(&engine, &session)?;
            engine
                .request(
                    &session.machine_id,
                    Command::CloseSession {
                        session_id: session.session_id,
                        force,
                    },
                )
                .await?;
        }
        Cmd::Terminal { command } => terminal::run(&engine, command).await?,
        Cmd::Mcp { command } => mcp::run(&engine, command).await?,
        Cmd::Pair => {
            let machine = choose_local_machine(&engine)?;
            if let CommandOutput::Pairing { code, .. } =
                engine.request(&machine, Command::StartPairing).await?
            {
                println!("{code}");
                eprintln!(
                    "On the other machine, run: tectr add-device {code}\nThen type the digits it shows here. The code works for 5 minutes."
                );
                confirm_pairing(&engine, &machine).await?;
            }
        }
        Cmd::AddDevice { code } => {
            let store = store.context("couldn't find a data directory to keep pairings in")?;
            let pending = engine.pair(&code, &engine::this_machine_name()).await?;
            eprintln!(
                "On {}, type {}",
                pending.machine.name,
                spaced_digits(&pending.digits)
            );
            let url = engine.finish_pairing(pending).await?;
            store.add_daemon(&url)?;
            println!("paired with {url}");
        }
        Cmd::RemoveDevice { device } => {
            let store = store.context("couldn't find a data directory to keep pairings in")?;
            let found = engine.devices().into_iter().find(|candidate| {
                candidate.url == device
                    || candidate.state.as_ref().is_some_and(|state| {
                        state.machine.name == device || state.machine.id.0.starts_with(&device)
                    })
            });
            let Some(found) = found.filter(|found| store.daemons().contains(&found.url)) else {
                bail!("no paired device matches {device}");
            };
            if let Some(state) = &found.state
                && found.connection.is_online()
            {
                let key = store.key()?.public().to_string();
                // It may already have removed this machine itself.
                let _ = engine
                    .request(&state.machine.id, Command::RemovePairedClient { key })
                    .await;
            }
            store.remove_daemon(&found.url)?;
            engine.remove_daemon(&found.url);
        }
        Cmd::Unpair { client, on } => {
            let machine = choose_machine(&engine, on.as_deref())?;
            let clients: Vec<_> = online_devices(&engine)
                .into_iter()
                .filter_map(|device| device.state)
                .filter(|state| state.machine.id == machine)
                .flat_map(|state| state.paired_clients)
                .filter(|paired| paired.name == client || paired.key.starts_with(&client))
                .collect();
            let key = match clients.as_slice() {
                [paired] => paired.key.clone(),
                [] => bail!("no paired client matches {client}"),
                _ => bail!("{client} is ambiguous"),
            };
            engine
                .request(&machine, Command::RemovePairedClient { key })
                .await?;
        }
    }
    Ok(())
}

/// Asks for the digits the other machine shows until they pair it. Running out of input
/// stops the pairing, so the code doesn't stay open with nobody to confirm it.
async fn confirm_pairing(engine: &Engine, machine: &MachineId) -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        eprint!("Digits: ");
        let Some(line) = lines.next_line().await? else {
            engine.request(machine, Command::CancelPairing).await?;
            bail!("stopped pairing");
        };
        if line.trim().is_empty() {
            continue;
        }
        let confirm = Command::ConfirmPairing { digits: line };
        match engine.request(machine, confirm).await {
            Ok(CommandOutput::Paired { client }) => {
                println!("paired {}", client.name);
                return Ok(());
            }
            Ok(_) => return Ok(()),
            Err(error) => eprintln!("{error}"),
        }
    }
}

/// `482913` as `482 913`, which is easier to read off one screen and type on another.
fn spaced_digits(digits: &str) -> String {
    let digits: Vec<char> = digits.chars().collect();
    let (first, second) = digits.split_at(digits.len() / 2);
    format!(
        "{} {}",
        first.iter().collect::<String>(),
        second.iter().collect::<String>()
    )
}

/// Pairing starts on the device itself, so it's the daemon reached over loopback.
fn choose_local_machine(engine: &Engine) -> Result<MachineId> {
    online_devices(engine)
        .into_iter()
        .find(|device| protocol::remote_daemon_key(&device.url).is_none())
        .and_then(|device| device.state)
        .map(|state| state.machine.id)
        .with_context(|| {
            format!(
                "this machine's daemon isn't reachable\n{}",
                describe_failures(engine)
            )
        })
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

/// The folder name `git clone` would choose for `url`.
fn repo_name(url: &str) -> Option<&str> {
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last);
    (!name.is_empty()).then_some(name)
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn online_devices(engine: &Engine) -> Vec<Device> {
    engine
        .devices()
        .into_iter()
        .filter(|device| device.connection.is_online())
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

/// Every online machine's archived topics, with the machine and its name.
async fn archived_topics(engine: &Engine) -> Result<Vec<(MachineId, String, ArchivedTopic)>> {
    let mut all = vec![];
    for device in online_devices(engine) {
        let Some(state) = device.state else { continue };
        let output = engine
            .request(&state.machine.id, Command::ListArchivedTopics)
            .await?;
        if let CommandOutput::ArchivedTopics { topics } = output {
            let machine = &state.machine;
            all.extend(
                topics
                    .into_iter()
                    .map(|t| (machine.id.clone(), machine.name.clone(), t)),
            );
        }
    }
    Ok(all)
}

fn print_risk(risk: &ArchiveRisk) {
    if risk.running > 0 {
        println!("{} agent(s) still running", risk.running);
    }
    for worktree in &risk.worktrees {
        println!(
            "{}: {} uncommitted file(s), {} unpushed commit(s) on {}",
            worktree.path, worktree.uncommitted, worktree.unpushed, worktree.branch
        );
    }
}

/// Every online machine's closed workspaces, with the machine and its name.
async fn closed_workspaces(engine: &Engine) -> Result<Vec<(MachineId, String, ClosedWorkspace)>> {
    let mut all = vec![];
    for device in online_devices(engine) {
        let Some(state) = device.state else { continue };
        let output = engine
            .request(&state.machine.id, Command::ListClosedWorkspaces)
            .await?;
        if let CommandOutput::ClosedWorkspaces { workspaces } = output {
            let machine = &state.machine;
            all.extend(
                workspaces
                    .into_iter()
                    .map(|w| (machine.id.clone(), machine.name.clone(), w)),
            );
        }
    }
    Ok(all)
}

/// A closed workspace by its exact name or an ID prefix, a name winning.
async fn resolve_closed(engine: &Engine, wanted: &str) -> Result<(MachineId, ClosedWorkspace)> {
    let all = closed_workspaces(engine).await?;
    let named: Vec<_> = all.iter().filter(|(_, _, w)| w.name == wanted).collect();
    let mut matches = if named.is_empty() {
        all.iter()
            .filter(|(_, _, w)| w.id.0.starts_with(wanted))
            .collect()
    } else {
        named
    };
    match matches.len() {
        1 => {
            let (machine, _, workspace) = matches.remove(0);
            Ok((machine.clone(), workspace.clone()))
        }
        0 => bail!("no closed workspace matches {wanted}"),
        count => bail!("{wanted} is ambiguous: {count} matches. Use an ID prefix"),
    }
}

/// Accepts an open workspace's exact name or an ID prefix, a name winning.
fn resolve_workspace(engine: &Engine, wanted: &str) -> Result<(MachineId, WorkspaceId)> {
    let mut named: Vec<_> = engine
        .devices()
        .into_iter()
        .filter_map(|device| device.state)
        .flat_map(|state| {
            state
                .workspaces
                .iter()
                .filter(|workspace| workspace.name == wanted)
                .map(|workspace| (state.machine.id.clone(), workspace.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    match named.len() {
        0 => {}
        1 => return Ok(named.remove(0)),
        count => bail!("{count} workspaces are named {wanted}. Use an ID prefix"),
    }
    resolve(
        engine,
        wanted,
        |state| state.workspaces.iter().map(|w| w.id.clone()).collect(),
        |w| &w.0,
    )
}

async fn conversations(engine: &Engine, machine: &MachineId) -> Result<Vec<Conversation>> {
    match engine.request(machine, Command::ListConversations).await? {
        CommandOutput::Conversations { conversations } => Ok(conversations),
        _ => bail!("the daemon didn't list conversations"),
    }
}

/// Accepts a topic's exact name or an ID prefix. A name wins, so a name that looks like an ID
/// still works.
fn resolve_topic(engine: &Engine, wanted: &str) -> Result<(MachineId, TopicId)> {
    let mut named: Vec<_> = engine
        .devices()
        .into_iter()
        .filter_map(|device| device.state)
        .flat_map(|state| {
            state
                .topics()
                .filter(|topic| topic.name == wanted)
                .map(|topic| (state.machine.id.clone(), topic.id.clone()))
                .collect::<Vec<_>>()
        })
        .collect();
    match named.len() {
        0 => {}
        1 => return Ok(named.remove(0)),
        count => bail!("{count} topics are named {wanted}. Use an ID prefix"),
    }
    resolve(
        engine,
        wanted,
        |state| state.topics().map(|t| t.id.clone()).collect(),
        |t| &t.0,
    )
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
            Connection::Online { stalled: false } => "online".to_owned(),
            Connection::Online { stalled: true } => "not responding".to_owned(),
            Connection::Offline { .. } => "offline".to_owned(),
            Connection::Asleep => "asleep".to_owned(),
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
        if let Some(line) = mcp::status_line(state) {
            println!("  {line}");
        }
        for client in &state.paired_clients {
            println!("  paired client {} {}", short(&client.key), client.name);
        }
        for workspace in &state.workspaces {
            println!(
                "  workspace {} {} {}",
                short(&workspace.id.0),
                workspace.name,
                workspace.path
            );
            if let Some(line) = mcp::workspace_status_line(state, workspace) {
                println!("    {line}");
            }
            for topic in &workspace.topics {
                println!("    topic {} {}", short(&topic.id.0), topic.name);
                for worktree in &topic.worktrees {
                    let here = if worktree.id == topic.worktree_id {
                        " (here)"
                    } else {
                        ""
                    };
                    println!(
                        "      worktree {} {} {}{here}",
                        short(&worktree.id.0),
                        worktree.branch,
                        worktree.path
                    );
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
                for terminal in &topic.terminals {
                    println!(
                        "      terminal {} {:?} {}",
                        short(&terminal.id.0),
                        terminal.status,
                        terminal.title.as_deref().unwrap_or(""),
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
            SessionEventKind::UserPrompt { text, .. } => println!("> {text}"),
            SessionEventKind::ToolCall { id, tool }
            | SessionEventKind::ToolCallUpdate { id, tool } => self.print_tool(id, tool),
            SessionEventKind::PermissionRequested { approval } => print_approval(approval),
            SessionEventKind::PermissionResolved { outcome, .. } => {
                println!("[permission] {outcome:?}");
            }
            SessionEventKind::TurnEnded { stop_reason, .. } => {
                println!("[turn ended] {stop_reason:?}");
            }
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
                        images: vec![],
                    },
                };
                if let Err(error) = engine.request(&session.machine_id, command).await {
                    println!("[error] {error}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_named_like_git_names_it() {
        assert_eq!(repo_name("https://github.com/me/tectr.git"), Some("tectr"));
        assert_eq!(repo_name("git@github.com:me/tectr.git"), Some("tectr"));
        assert_eq!(repo_name("https://github.com/me/tectr/"), Some("tectr"));
        assert_eq!(repo_name("/srv/repos/tectr"), Some("tectr"));
        assert_eq!(repo_name(".git"), None);
    }
}
