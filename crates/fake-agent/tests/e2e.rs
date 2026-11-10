//! End-to-end tests: a real daemon and real client engines over real websockets,
//! with the fake agent as a real process. They live in this crate because Cargo
//! only exposes a binary's path to tests in the same package, and it keeps the
//! daemon and engine from depending on each other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use daemon::{AgentLauncher, Config, Daemon, Shell};
use engine::{Connection, Engine, EngineError, Remote};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    AgentCommand, Approval, ArchiveRisk, ClientMessage, Command, CommandOutput, DaemonState,
    ErrorCode, MachineId, MoveTarget, PermissionMode, PermissionOutcome, ServerMessage, Session,
    SessionEventKind, SessionRef, SessionStatus, StopReason, Topic, TopicId, Workspace,
    WorktreeRisk,
};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const TIMEOUT: Duration = Duration::from_secs(20);

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        // The default macOS data directory has a space in it.
        let repo = root.join("my repo");
        std::fs::create_dir(&repo).unwrap();
        run_git(&repo, &["init", "-q"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        Self {
            _dir: dir,
            root,
            repo,
        }
    }

    fn config(&self, name: &str, port: u16) -> Config {
        Config {
            agents: BTreeMap::from([(
                "fake".to_owned(),
                AgentLauncher::Command {
                    program: env!("CARGO_BIN_EXE_fake-agent").into(),
                    args: vec![],
                },
            )]),
            // Tests shouldn't hold the machine awake. The daemon's own tests cover it.
            keep_awake: None,
            // The person's own shell would read their setup, which tests can't predict.
            shell: Shell {
                program: "/bin/sh".into(),
                args: vec![],
            },
            ..Config::new(self.root.join("Application Support").join(name), port)
        }
    }

    async fn daemon(&self, name: &str) -> Daemon {
        Daemon::start(self.config(name, 0)).await.unwrap()
    }
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn url(daemon: &Daemon) -> String {
    format!("ws://{}", daemon.addr())
}

async fn connect(daemon: &Daemon) -> Engine {
    let engine = Engine::new("e2e");
    engine.add_daemon(&url(daemon));
    engine.wait_until_settled().await;
    assert_eq!(engine.devices()[0].connection, Connection::Online);
    engine
}

/// Polls until `check` returns a value. Each session test has its own
/// expectation here, so a failure names what never happened.
async fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn add_workspace(engine: &Engine, machine: &MachineId, repo: &Path) -> Workspace {
    let command = Command::AddWorkspace {
        path: repo.display().to_string(),
    };
    match engine.request(machine, command).await.unwrap() {
        CommandOutput::Workspace { workspace } => workspace,
        other => panic!("unexpected output {other:?}"),
    }
}

async fn add_topic(engine: &Engine, machine: &MachineId, repo: &Path) -> Topic {
    let workspace = add_workspace(engine, machine, repo).await;
    let command = Command::CreateTopic {
        workspace_id: workspace.id,
        name: Some("My topic!".to_owned()),
        at_root: false,
    };
    match engine.request(machine, command).await.unwrap() {
        CommandOutput::Topic { topic, .. } => topic,
        other => panic!("unexpected output {other:?}"),
    }
}

async fn start_session(
    engine: &Engine,
    machine: &MachineId,
    topic: &Topic,
    mode: PermissionMode,
    model: Option<&str>,
) -> SessionRef {
    let session = start_agent(engine, machine, &topic.id, "fake", mode, model)
        .await
        .unwrap();
    let session = SessionRef {
        machine_id: machine.clone(),
        session_id: session.id,
    };
    engine.open_session(&session);
    session
}

async fn start_agent(
    engine: &Engine,
    machine: &MachineId,
    topic_id: &TopicId,
    agent: &str,
    mode: PermissionMode,
    model: Option<&str>,
) -> Result<Session, EngineError> {
    let output = engine
        .request(
            machine,
            Command::StartSession {
                topic_id: topic_id.clone(),
                agent: agent.to_owned(),
                mode,
                model: model.map(str::to_owned),
                effort: None,
            },
        )
        .await?;
    match output {
        CommandOutput::Session { session, .. } => Ok(session),
        other => panic!("unexpected output {other:?}"),
    }
}

async fn start_ready_session(
    engine: &Engine,
    machine: &MachineId,
    repo: &Path,
    mode: PermissionMode,
) -> (Topic, SessionRef) {
    let topic = add_topic(engine, machine, repo).await;
    let session = start_session(engine, machine, &topic, mode, None).await;
    wait_for_status(engine, &session, SessionStatus::Ready).await;
    (topic, session)
}

async fn prompt(
    engine: &Engine,
    session: &SessionRef,
    text: &str,
) -> Result<CommandOutput, EngineError> {
    engine
        .request(
            &session.machine_id,
            Command::SendPrompt {
                session_id: session.session_id.clone(),
                text: text.to_owned(),
            },
        )
        .await
}

fn status(engine: &Engine, session: &SessionRef) -> Option<SessionStatus> {
    engine
        .devices()
        .iter()
        .filter_map(|device| device.state.as_ref())
        .filter(|state| state.machine.id == session.machine_id)
        .find_map(|state| state.session(&session.session_id))
        .map(|session| session.status)
}

fn last_status_event(engine: &Engine, session: &SessionRef) -> Option<SessionStatus> {
    engine
        .events_after(session, 0)
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            SessionEventKind::StatusChanged { status } => Some(status),
            _ => None,
        })
}

async fn wait_for_status(engine: &Engine, session: &SessionRef, wanted: SessionStatus) {
    wait_for(&format!("status {wanted:?}"), || {
        (status(engine, session) == Some(wanted)
            && last_status_event(engine, session) == Some(wanted))
        .then_some(())
    })
    .await;
}

async fn wait_for_approval(engine: &Engine, session: &SessionRef) -> Approval {
    wait_for("a pending approval", || {
        engine
            .approvals()
            .into_iter()
            .find(|(machine, approval)| {
                machine == &session.machine_id && approval.session_id == session.session_id
            })
            .map(|(_, approval)| approval)
    })
    .await
}

async fn answer(
    engine: &Engine,
    session: &SessionRef,
    approval: &Approval,
    option: &str,
) -> Result<CommandOutput, EngineError> {
    engine
        .request(
            &session.machine_id,
            Command::AnswerApproval {
                approval_id: approval.id.clone(),
                option_id: option.to_owned(),
            },
        )
        .await
}

fn agent_text(engine: &Engine, session: &SessionRef) -> Vec<String> {
    engine
        .events_after(session, 0)
        .into_iter()
        .filter_map(|event| match event.kind {
            SessionEventKind::AgentMessage { text } => Some(text),
            _ => None,
        })
        .collect()
}

fn assert_gapless(engine: &Engine, session: &SessionRef) {
    let seqs: Vec<u64> = engine
        .events_after(session, 0)
        .iter()
        .map(|event| event.seq)
        .collect();
    let expected: Vec<u64> = (1..=seqs.len() as u64).collect();
    assert_eq!(seqs, expected);
}

fn position(
    engine: &Engine,
    session: &SessionRef,
    matches: impl Fn(&SessionEventKind) -> bool,
) -> usize {
    engine
        .events_after(session, 0)
        .iter()
        .position(|event| matches(&event.kind))
        .expect("the event should exist")
}

#[tokio::test(flavor = "multi_thread")]
async fn core_loop_in_a_fresh_worktree() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();

    // A workspace and a topic with its own branch and worktree.
    let workspace = add_workspace(&engine, &machine, &fixture.repo).await;
    assert_eq!(workspace.name, "my repo");
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let worktree = &topic.worktrees[0];
    assert!(
        worktree.branch.starts_with("tectr/my-topic-"),
        "{}",
        worktree.branch
    );
    let worktree_path = PathBuf::from(&worktree.path);
    assert_eq!(
        run_git(&worktree_path, &["branch", "--show-current"]),
        worktree.branch
    );

    // Adding the same repo again returns the same workspace.
    assert_eq!(
        add_workspace(&engine, &machine, &fixture.repo).await.id,
        workspace.id
    );

    // The session starts in that worktree and streams its answer.
    let session = start_session(&engine, &machine, &topic, PermissionMode::Manual, None).await;
    wait_for_status(&engine, &session, SessionStatus::Ready).await;
    prompt(&engine, &session, "echo hello tectr").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    assert_eq!(agent_text(&engine, &session), ["hello", "tectr"]);

    // The permission request reaches the client, and the tool waits for the answer.
    prompt(&engine, &session, "write").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::WaitingForYou).await;
    let approval = wait_for_approval(&engine, &session).await;
    assert_eq!(
        approval.tool_call.title.as_deref(),
        Some("Write fake-output.txt")
    );
    assert_eq!(approval.options.len(), 2);
    let output = worktree_path.join("fake-output.txt");
    assert!(!output.exists(), "the tool ran before it was allowed");

    answer(&engine, &session, &approval, "allow").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    assert!(output.exists(), "the tool should have run in the worktree");
    assert!(
        !fixture.repo.join("fake-output.txt").exists(),
        "the tool must not touch the main checkout"
    );
    assert!(engine.approvals().is_empty());

    // The transcript has no gaps and is in the order things happened.
    assert_gapless(&engine, &session);
    let requested = position(&engine, &session, |kind| {
        matches!(kind, SessionEventKind::PermissionRequested { .. })
    });
    let resolved = position(&engine, &session, |kind| {
        matches!(
            kind,
            SessionEventKind::PermissionResolved {
                outcome: PermissionOutcome::Selected { .. },
                ..
            }
        )
    });
    let wrote = position(
        &engine,
        &session,
        |kind| matches!(kind, SessionEventKind::AgentMessage { text } if text == "wrote the file"),
    );
    assert!(requested < resolved && resolved < wrote);

    // Every event names the machine it came from.
    assert!(
        engine
            .events_after(&session, 0)
            .iter()
            .all(|event| event.session == session)
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_denied_tool_does_not_run() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;

    prompt(&engine, &session, "write").await.unwrap();
    let approval = wait_for_approval(&engine, &session).await;
    answer(&engine, &session, &approval, "deny").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;

    assert_eq!(agent_text(&engine, &session), ["denied"]);
    assert!(
        !Path::new(&topic.worktrees[0].path)
            .join("fake-output.txt")
            .exists()
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn folders_are_listed_for_completing_a_path() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let list = |path: String| {
        let (engine, machine) = (&engine, &machine);
        async move {
            let command = Command::ListFolders { path };
            match engine.request(machine, command).await {
                Ok(CommandOutput::Folders { folders }) => Ok(folders),
                Ok(other) => panic!("unexpected output {other:?}"),
                Err(error) => Err(error),
            }
        }
    };

    let root = fixture.root.display();
    let folders = list(format!("{root}/")).await.unwrap();
    let repo = folders.iter().find(|f| f.name == "my repo").unwrap();
    assert!(repo.is_repo);
    assert_eq!(repo.path, fixture.repo.display().to_string());

    // The listed path is the one a workspace ends up with, which is how a client knows
    // that a folder has already been added.
    let workspace = add_workspace(&engine, &machine, Path::new(&repo.path)).await;
    assert_eq!(workspace.path, repo.path);

    let inside_a_repo = list(format!("{root}/my repo")).await.unwrap();
    assert!(inside_a_repo.iter().any(|f| f.name == ".git" && !f.is_repo));

    assert_eq!(list(format!("{root}/not there/")).await.unwrap(), []);
    let refused = |result: Result<_, EngineError>| matches!(result, Err(EngineError::Command(error)) if error.code == ErrorCode::InvalidInput);
    assert!(refused(list("relative/".to_owned()).await));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unnamed_topic_numbers_itself_and_can_work_at_the_repo_root() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let workspace = add_workspace(&engine, &machine, &fixture.repo).await;
    let create = async |at_root| {
        let command = Command::CreateTopic {
            workspace_id: workspace.id.clone(),
            name: None,
            at_root,
        };
        match engine.request(&machine, command).await.unwrap() {
            CommandOutput::Topic { topic, .. } => topic,
            other => panic!("unexpected output {other:?}"),
        }
    };

    let at_root = create(true).await;
    assert_eq!(at_root.name, "Topic #1");
    assert_eq!(at_root.worktrees[0].path, workspace.path);
    assert_eq!(
        at_root.worktrees[0].branch,
        run_git(&fixture.repo, &["branch", "--show-current"])
    );
    assert_eq!(
        run_git(&fixture.repo, &["worktree", "list"])
            .lines()
            .count(),
        1
    );

    let in_worktree = create(false).await;
    assert_eq!(in_worktree.name, "Topic #2");
    assert!(
        in_worktree.worktrees[0]
            .branch
            .starts_with("tectr/topic-2-")
    );
    assert_ne!(in_worktree.worktrees[0].path, workspace.path);

    // The new topic screen sends its prompt without waiting for the agent to be ready.
    let session = start_session(&engine, &machine, &at_root, PermissionMode::Yolo, None).await;
    prompt(&engine, &session, "whoami").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let report = agent_text(&engine, &session).concat();
    assert!(
        report.contains(&format!("cwd={}", workspace.path)),
        "{report}"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_provider_reports_what_it_offers_and_is_kept_by_the_daemon() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("providers").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();

    match engine
        .request(&machine, Command::DetectProviders)
        .await
        .unwrap()
    {
        CommandOutput::ProviderCandidates { candidates } => {
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].agent, "fake");
            assert_eq!(candidates[0].missing, None);
        }
        other => panic!("unexpected output {other:?}"),
    }

    let error = engine
        .request(
            &machine,
            Command::AddProvider {
                agent: "nobody".to_owned(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));

    let add = Command::AddProvider {
        agent: "fake".to_owned(),
    };
    let provider = match engine.request(&machine, add).await.unwrap() {
        CommandOutput::Provider { provider } => provider,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(provider.version.as_deref(), Some("0.0.0"));
    assert_eq!(
        provider.modes,
        [
            PermissionMode::Manual,
            PermissionMode::Auto,
            PermissionMode::Yolo
        ]
    );
    // "Default" isn't offered. The model it stands for is marked instead.
    assert!(provider.models.iter().all(|m| m.id != "default"));
    assert_eq!(provider.recommended_model.as_deref(), Some("smart"));
    assert_eq!(provider.default_model.as_deref(), Some("smart"));
    assert_eq!(provider.default_effort.as_deref(), Some("low"));
    let efforts = |id: &str| -> Vec<String> {
        let model = provider.models.iter().find(|m| m.id == id).unwrap();
        model.efforts.iter().map(|e| e.id.clone()).collect()
    };
    assert!(efforts("cheap").is_empty());
    assert_eq!(efforts("smart"), ["low", "high"]);
    let stored = |engine: &Engine| engine.devices()[0].state.clone().unwrap().providers;
    assert_eq!(stored(&engine), std::slice::from_ref(&provider));

    // An effort has to belong to the model it's chosen with.
    let set = |model: &str, effort: Option<&str>| Command::SetProviderDefaults {
        agent: "fake".to_owned(),
        model: Some(model.to_owned()),
        effort: effort.map(str::to_owned),
    };
    let error = engine
        .request(&machine, set("cheap", Some("high")))
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));
    engine
        .request(&machine, set("smart", Some("high")))
        .await
        .unwrap();

    // Asking the agent again doesn't lose what was chosen.
    let refreshed = Command::AddProvider {
        agent: "fake".to_owned(),
    };
    let provider = match engine.request(&machine, refreshed).await.unwrap() {
        CommandOutput::Provider { provider } => provider,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(provider.default_effort.as_deref(), Some("high"));

    // It's a device setting, so it's still there after a restart.
    daemon.shutdown().await;
    let daemon = fixture.daemon("providers").await;
    let engine = connect(&daemon).await;
    assert_eq!(stored(&engine), std::slice::from_ref(&provider));

    let remove = Command::RemoveProvider {
        agent: "fake".to_owned(),
    };
    engine.request(&machine, remove).await.unwrap();
    assert!(stored(&engine).is_empty());
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_gets_its_model_and_effort_without_losing_its_mode() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let start = async |model: &str, effort: Option<&str>| {
        let command = Command::StartSession {
            topic_id: topic.id.clone(),
            agent: "fake".to_owned(),
            mode: PermissionMode::Yolo,
            model: Some(model.to_owned()),
            effort: effort.map(str::to_owned),
        };
        match engine.request(&machine, command).await.unwrap() {
            CommandOutput::Session { session, .. } => SessionRef {
                machine_id: machine.clone(),
                session_id: session.id,
            },
            other => panic!("unexpected output {other:?}"),
        }
    };

    let smart = start("smart", Some("high")).await;
    engine.open_session(&smart);
    prompt(&engine, &smart, "whoami").await.unwrap();
    wait_for_status(&engine, &smart, SessionStatus::Done).await;
    let report = agent_text(&engine, &smart).concat();
    assert!(report.contains("model=smart effort=high"), "{report}");

    // Selecting this model changes the agent's mode, and the chosen one must win.
    let cheap = start("cheap", None).await;
    engine.open_session(&cheap);
    prompt(&engine, &cheap, "whoami").await.unwrap();
    wait_for_status(&engine, &cheap, SessionStatus::Done).await;
    let report = agent_text(&engine, &cheap).concat();
    assert!(report.contains("mode=bypassPermissions"), "{report}");

    // An effort the model doesn't have fails instead of being ignored.
    let unsupported = start("cheap", Some("high")).await;
    engine.open_session(&unsupported);
    wait_for_status(&engine, &unsupported, SessionStatus::Failed).await;
    daemon.shutdown().await;
}

/// Starts a session on a chosen model and effort, and waits until it's ready.
async fn start_configured(
    engine: &Engine,
    machine: &MachineId,
    topic: &Topic,
    mode: PermissionMode,
    model: &str,
    effort: Option<&str>,
) -> SessionRef {
    let command = Command::StartSession {
        topic_id: topic.id.clone(),
        agent: "fake".to_owned(),
        mode,
        model: Some(model.to_owned()),
        effort: effort.map(str::to_owned),
    };
    let CommandOutput::Session { session, .. } = engine.request(machine, command).await.unwrap()
    else {
        panic!("expected a session");
    };
    let session = SessionRef {
        machine_id: machine.clone(),
        session_id: session.id,
    };
    engine.open_session(&session);
    wait_for_status(engine, &session, SessionStatus::Ready).await;
    session
}

async fn set_settings(
    engine: &Engine,
    session: &SessionRef,
    mode: PermissionMode,
    model: Option<&str>,
    effort: Option<&str>,
) -> Result<CommandOutput, EngineError> {
    engine
        .request(
            &session.machine_id,
            Command::SetSessionSettings {
                session_id: session.session_id.clone(),
                mode,
                model: model.map(str::to_owned),
                effort: effort.map(str::to_owned),
            },
        )
        .await
}

/// What the agent says it's running with, from a new turn.
async fn agent_report(engine: &Engine, session: &SessionRef) -> String {
    prompt(engine, session, "whoami").await.unwrap();
    wait_for_status(engine, session, SessionStatus::Done).await;
    agent_text(engine, session).last().unwrap().clone()
}

fn word_after<'a>(report: &'a str, prefix: &str) -> &'a str {
    report
        .split_whitespace()
        .find_map(|word| word.strip_prefix(prefix))
        .unwrap_or_else(|| panic!("no {prefix} in {report}"))
}

fn session_state(engine: &Engine, session: &SessionRef) -> Session {
    state_of(engine)
        .session(&session.session_id)
        .unwrap()
        .clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_agent_changes_settings_without_restarting() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let other_client = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "smart",
        Some("high"),
    )
    .await;
    let before = agent_report(&engine, &session).await;

    set_settings(
        &engine,
        &session,
        PermissionMode::Yolo,
        Some("smart"),
        Some("low"),
    )
    .await
    .unwrap();

    // It's current as soon as the command returns, and every client hears about it.
    let state = session_state(&engine, &session);
    assert_eq!(state.mode, PermissionMode::Yolo);
    assert_eq!(state.effort.as_deref(), Some("low"));
    wait_for("the other client to see the change", || {
        let state = state_of(&other_client);
        let seen = state.session(&session.session_id)?;
        (seen.mode == PermissionMode::Yolo).then_some(())
    })
    .await;

    let report = agent_report(&engine, &session).await;
    assert!(report.contains("mode=bypassPermissions"), "{report}");
    assert!(report.contains("model=smart effort=low"), "{report}");
    assert_eq!(word_after(&report, "pid="), word_after(&before, "pid="));
    assert_eq!(word_after(&report, "resumed="), "false");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_model_keeps_the_chosen_mode_and_can_go_back() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "smart",
        Some("high"),
    )
    .await;

    // Selecting this model moves the agent to another mode, and the chosen one must win.
    set_settings(&engine, &session, PermissionMode::Auto, Some("cheap"), None)
        .await
        .unwrap();
    let report = agent_report(&engine, &session).await;
    assert!(report.contains("mode=auto model=cheap"), "{report}");
    assert_eq!(session_state(&engine, &session).effort, None);

    // Back through a mode the agent was told earlier, which its own report no longer shows.
    set_settings(&engine, &session, PermissionMode::Yolo, Some("cheap"), None)
        .await
        .unwrap();
    set_settings(&engine, &session, PermissionMode::Auto, Some("cheap"), None)
        .await
        .unwrap();
    let report = agent_report(&engine, &session).await;
    assert!(report.contains("mode=auto model=cheap"), "{report}");

    // The effort has to be sent again for the new model, even if it's the same one.
    set_settings(
        &engine,
        &session,
        PermissionMode::Auto,
        Some("smart"),
        Some("high"),
    )
    .await
    .unwrap();
    let report = agent_report(&engine, &session).await;
    assert!(report.contains("model=smart effort=high"), "{report}");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_can_change_during_a_turn_and_cancelling_still_works() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "smart",
        Some("low"),
    )
    .await;

    prompt(&engine, &session, "hang").await.unwrap();
    set_settings(
        &engine,
        &session,
        PermissionMode::Yolo,
        Some("smart"),
        Some("low"),
    )
    .await
    .unwrap();
    assert_eq!(status(&engine, &session), Some(SessionStatus::Working));

    engine
        .request(
            &machine,
            Command::CancelTurn {
                session_id: session.session_id.clone(),
            },
        )
        .await
        .unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let report = agent_report(&engine, &session).await;
    assert!(report.contains("mode=bypassPermissions"), "{report}");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_agent_gets_its_settings_when_it_resumes() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let port = daemon.addr().port();
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "smart",
        Some("low"),
    )
    .await;
    set_settings(
        &engine,
        &session,
        PermissionMode::Manual,
        Some("smart"),
        Some("high"),
    )
    .await
    .unwrap();
    daemon.shutdown().await;

    // What changed while it ran survives a restart.
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let engine = connect(&daemon).await;
    engine.open_session(&session);
    let state = session_state(&engine, &session);
    assert_eq!(state.status, SessionStatus::Stopped);
    assert_eq!(state.mode, PermissionMode::Manual);
    assert_eq!(state.effort.as_deref(), Some("high"));

    // With no agent running, there's nothing to ask, and the next start uses it.
    set_settings(
        &engine,
        &session,
        PermissionMode::Yolo,
        Some("smart"),
        Some("high"),
    )
    .await
    .unwrap();
    assert_eq!(status(&engine, &session), Some(SessionStatus::Stopped));
    let report = agent_report(&engine, &session).await;
    assert!(report.contains("mode=bypassPermissions"), "{report}");
    assert!(report.contains("model=smart effort=high"), "{report}");
    assert_eq!(word_after(&report, "resumed="), "true");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_change_leaves_the_agent_running_as_it_was() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "cheap",
        None,
    )
    .await;
    let before = agent_report(&engine, &session).await;

    // This model has no efforts. Nothing changed in the agent before it failed.
    let error = set_settings(
        &engine,
        &session,
        PermissionMode::Auto,
        Some("cheap"),
        Some("high"),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));
    assert_eq!(session_state(&engine, &session).effort, None);

    let report = agent_report(&engine, &session).await;
    assert_eq!(word_after(&report, "pid="), word_after(&before, "pid="));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_that_fails_partway_stops_the_agent_and_keeps_the_old_settings() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "smart",
        Some("high"),
    )
    .await;
    let before = agent_report(&engine, &session).await;

    // The model changes first, which changes the agent's mode, and then the effort fails.
    set_settings(
        &engine,
        &session,
        PermissionMode::Auto,
        Some("cheap"),
        Some("high"),
    )
    .await
    .unwrap_err();
    wait_for_status(&engine, &session, SessionStatus::Failed).await;
    assert_eq!(
        session_state(&engine, &session).model.as_deref(),
        Some("smart")
    );

    // The next prompt starts it again with what the session says.
    let report = agent_report(&engine, &session).await;
    assert_ne!(word_after(&report, "pid="), word_after(&before, "pid="));
    assert!(
        report.contains("mode=auto model=smart effort=high"),
        "{report}"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_what_the_provider_offers_can_be_chosen() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    engine
        .request(
            &machine,
            Command::AddProvider {
                agent: "fake".to_owned(),
            },
        )
        .await
        .unwrap();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_configured(
        &engine,
        &machine,
        &topic,
        PermissionMode::Auto,
        "smart",
        Some("low"),
    )
    .await;
    let before = agent_report(&engine, &session).await;

    for (model, effort) in [
        ("nope", None),
        ("cheap", Some("low")),
        ("smart", Some("nope")),
    ] {
        let error = set_settings(&engine, &session, PermissionMode::Auto, Some(model), effort)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput),
            "{model} {effort:?}: {error:?}"
        );
    }

    // None of these reached the agent, so it never had to stop.
    let report = agent_report(&engine, &session).await;
    assert_eq!(word_after(&report, "pid="), word_after(&before, "pid="));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_must_be_one_of_the_offered_options() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;

    prompt(&engine, &session, "write").await.unwrap();
    let approval = wait_for_approval(&engine, &session).await;
    let error = answer(&engine, &session, &approval, "made-up")
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));
    assert_eq!(
        engine.approvals().len(),
        1,
        "a bad answer must leave the request open"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mode_and_model_reach_the_agent() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;

    // Yolo never asks: the tool runs straight away.
    let yolo = start_session(
        &engine,
        &machine,
        &topic,
        PermissionMode::Yolo,
        Some("cheap"),
    )
    .await;
    wait_for_status(&engine, &yolo, SessionStatus::Ready).await;
    prompt(&engine, &yolo, "whoami").await.unwrap();
    wait_for_status(&engine, &yolo, SessionStatus::Done).await;
    let report = agent_text(&engine, &yolo).concat();
    assert!(report.contains("mode=bypassPermissions"), "{report}");
    assert!(report.contains("model=cheap"), "{report}");
    assert!(
        report.contains(&format!("cwd={}", topic.worktrees[0].path)),
        "{report}"
    );

    prompt(&engine, &yolo, "write").await.unwrap();
    wait_for_status(&engine, &yolo, SessionStatus::Done).await;
    assert_eq!(agent_text(&engine, &yolo).last().unwrap(), "wrote the file");

    // Two sessions can point at one worktree, and each keeps its own mode.
    let auto = start_session(&engine, &machine, &topic, PermissionMode::Auto, None).await;
    wait_for_status(&engine, &auto, SessionStatus::Ready).await;
    prompt(&engine, &auto, "whoami").await.unwrap();
    wait_for_status(&engine, &auto, SessionStatus::Done).await;
    let report = agent_text(&engine, &auto).concat();
    assert!(
        report.contains("mode=auto") && report.contains("model=none"),
        "{report}"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_outliving_its_turn_keeps_the_session_waiting() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;

    prompt(&engine, &session, "abandon").await.unwrap();
    wait_for("the turn to end", || {
        engine
            .events_after(&session, 0)
            .iter()
            .any(|event| matches!(event.kind, SessionEventKind::TurnEnded { .. }))
            .then_some(())
    })
    .await;
    wait_for_status(&engine, &session, SessionStatus::WaitingForYou).await;

    let approval = wait_for_approval(&engine, &session).await;
    answer(&engine, &session, &approval, "allow").await.unwrap();
    assert_eq!(
        status(&engine, &session),
        Some(SessionStatus::Done),
        "no turn is running, so the session must not claim to be working"
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_first_of_two_clients_answers() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let desk = connect(&daemon).await;
    let pocket = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&desk, &machine, &fixture.repo, PermissionMode::Manual).await;

    prompt(&desk, &session, "write").await.unwrap();
    let approval = wait_for_approval(&desk, &session).await;
    let same = wait_for_approval(&pocket, &session).await;
    assert_eq!(approval, same, "both clients should see the same request");

    let (first, second) = tokio::join!(
        answer(&desk, &session, &approval, "allow"),
        answer(&pocket, &session, &approval, "deny"),
    );
    let answers = [first, second];
    assert_eq!(
        answers.iter().filter(|result| result.is_ok()).count(),
        1,
        "{answers:?}"
    );
    assert!(answers.iter().any(|result| {
        matches!(result, Err(EngineError::Command(error)) if error.code == ErrorCode::NotFound)
    }));

    wait_for_status(&desk, &session, SessionStatus::Done).await;
    wait_for("the second client's approvals to clear", || {
        pocket.approvals().is_empty().then_some(())
    })
    .await;
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn state_is_current_as_soon_as_a_command_returns() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;

    for turn in 1..=100 {
        prompt(&engine, &session, "echo x").await.unwrap();
        let prompts = engine
            .events_after(&session, 0)
            .iter()
            .filter(|event| matches!(event.kind, SessionEventKind::UserPrompt { .. }))
            .count();
        assert_eq!(prompts, turn, "the event must arrive before the response");
        assert_ne!(
            status(&engine, &session),
            Some(SessionStatus::Ready),
            "the state change must arrive before the response"
        );
        wait_for_status(&engine, &session, SessionStatus::Done).await;
    }
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reopening_a_session_quickly_keeps_its_whole_history() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;

    // Reopen repeatedly while events stream, so that some are still on their
    // way each time the history is replayed from the start.
    prompt(&engine, &session, "burst 5000").await.unwrap();
    for _ in 0..50 {
        engine.close_session(&session);
        engine.open_session(&session);
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    wait_for_status(&engine, &session, SessionStatus::Done).await;

    assert_gapless(&engine, &session);
    let expected: Vec<String> = (0..5000).map(|n| n.to_string()).collect();
    assert_eq!(agent_text(&engine, &session), expected);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_connecting_later_gets_the_full_history() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let first = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&first, &machine, &fixture.repo, PermissionMode::Manual).await;
    prompt(&first, &session, "echo a b").await.unwrap();
    wait_for_status(&first, &session, SessionStatus::Done).await;

    let later = connect(&daemon).await;
    assert_eq!(
        status(&later, &session),
        Some(SessionStatus::Done),
        "the tree comes with the snapshot"
    );
    later.open_session(&session);
    wait_for("the replay", || {
        (later.events_after(&session, 0).len() == first.events_after(&session, 0).len())
            .then_some(())
    })
    .await;
    assert_eq!(
        later.events_after(&session, 0),
        first.events_after(&session, 0)
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribing_after_n_sends_only_later_events() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;
    prompt(&engine, &session, "echo a b c").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let total = engine.events_after(&session, 0).len() as u64;

    let mut socket = raw_connect(&daemon).await;
    assert!(matches!(
        say_hello(&mut socket, 1, 1).await,
        ServerMessage::Welcome { version: 1, .. }
    ));
    assert!(matches!(
        raw_next(&mut socket).await,
        ServerMessage::Snapshot { .. }
    ));
    raw_send(
        &mut socket,
        &ClientMessage::Subscribe {
            session_id: session.session_id.clone(),
            after_seq: total - 2,
        },
    )
    .await;
    for expected in [total - 1, total] {
        let ServerMessage::SessionEvent { event } = raw_next(&mut socket).await else {
            panic!("expected an event");
        };
        assert_eq!(event.seq, expected);
    }
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_falling_behind_loses_nothing() {
    let fixture = Fixture::new();
    // A tiny buffer makes every client fall behind the broadcast.
    let config = Config {
        event_buffer: 2,
        ..fixture.config("d", 0)
    };
    let daemon = Daemon::start(config).await.unwrap();
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;

    prompt(&engine, &session, "burst 2000").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;

    assert_gapless(&engine, &session);
    let expected: Vec<String> = (0..2000).map(|n| n.to_string()).collect();
    assert_eq!(agent_text(&engine, &session), expected);
    daemon.shutdown().await;
}

fn state_of(engine: &Engine) -> DaemonState {
    engine.devices()[0].state.clone().unwrap()
}

fn is_process_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// Runs a turn that reports the agent's process ID.
async fn agent_pid(engine: &Engine, session: &SessionRef) -> String {
    prompt(engine, session, "whoami").await.unwrap();
    wait_for_status(engine, session, SessionStatus::Done).await;
    let report = agent_text(engine, session).concat();
    report
        .split_whitespace()
        .find_map(|word| word.strip_prefix("pid="))
        .unwrap_or_else(|| panic!("no pid in {report}"))
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn every_client_sees_the_same_tabs_and_drafts() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let port = daemon.addr().port();
    let machine = daemon.machine_id().clone();
    let first = connect(&daemon).await;
    let second = connect(&daemon).await;

    let (topic, older) =
        start_ready_session(&first, &machine, &fixture.repo, PermissionMode::Auto).await;
    let newer = start_session(&first, &machine, &topic, PermissionMode::Auto, None).await;
    wait_for("the second client to see the new tab", || {
        state_of(&second).session(&newer.session_id).map(|_| ())
    })
    .await;

    first
        .request(
            &machine,
            Command::SetDraft {
                session_id: older.session_id.clone(),
                text: "half a tho".to_owned(),
            },
        )
        .await
        .unwrap();
    let drafts = state_of(&first).drafts;
    assert_eq!(drafts.get(&older.session_id).unwrap(), "half a tho");
    wait_for("the other client to see the draft", || {
        (state_of(&second).drafts == drafts).then_some(())
    })
    .await;

    // A client that wasn't there gets all of it, and so does one after a restart.
    let tabs = |engine: &Engine| {
        let state = state_of(engine);
        let sessions = &state.topic(&topic.id).unwrap().sessions;
        sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>()
    };
    let expected = vec![older.session_id.clone(), newer.session_id.clone()];
    let late = connect(&daemon).await;
    assert_eq!(tabs(&late), expected);
    assert_eq!(state_of(&late).drafts, drafts);
    daemon.shutdown().await;
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let restarted = connect(&daemon).await;
    assert_eq!(tabs(&restarted), expected);
    assert_eq!(state_of(&restarted).drafts, drafts);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sending_a_prompt_clears_its_draft() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;
    engine
        .request(
            &machine,
            Command::SetDraft {
                session_id: session.session_id.clone(),
                text: "echo hi".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(state_of(&engine).drafts.contains_key(&session.session_id));

    prompt(&engine, &session, "echo hi").await.unwrap();
    assert!(state_of(&engine).drafts.is_empty());
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_finished_tab_kills_its_agent_and_drops_its_draft() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let port = daemon.addr().port();
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (topic, first) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;
    let second = start_session(&engine, &machine, &topic, PermissionMode::Auto, None).await;
    wait_for_status(&engine, &second, SessionStatus::Ready).await;
    let third = start_session(&engine, &machine, &topic, PermissionMode::Auto, None).await;
    wait_for_status(&engine, &third, SessionStatus::Ready).await;
    let pid = agent_pid(&engine, &second).await;
    let draft = Command::SetDraft {
        session_id: second.session_id.clone(),
        text: "never sent".to_owned(),
    };
    engine.request(&machine, draft).await.unwrap();

    // Finished, so it closes without being forced.
    let close = |session: &SessionRef, force| Command::CloseSession {
        session_id: session.session_id.clone(),
        force,
    };
    engine
        .request(&machine, close(&second, false))
        .await
        .unwrap();

    let state = state_of(&engine);
    let sessions = &state.topic(&topic.id).unwrap().sessions;
    assert_eq!(sessions.len(), 2, "the tab is gone");
    assert!(state.session(&second.session_id).is_none());
    assert!(state.drafts.is_empty());
    wait_for("the agent's process to die", || {
        (!is_process_alive(&pid)).then_some(())
    })
    .await;

    for session in [&third, &first] {
        engine
            .request(&machine, close(session, false))
            .await
            .unwrap();
    }

    // Closing twice is a clear error, and it stays closed after a restart.
    let error = engine
        .request(&machine, close(&first, false))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no session"), "{error}");
    daemon.shutdown().await;
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let state = state_of(&connect(&daemon).await);
    assert!(state.topic(&topic.id).unwrap().sessions.is_empty());
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_running_agent_has_to_be_forced() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;
    let pid = agent_pid(&engine, &session).await;
    prompt(&engine, &session, "hang").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Working).await;

    let close = |force| Command::CloseSession {
        session_id: session.session_id.clone(),
        force,
    };
    let error = engine.request(&machine, close(false)).await.unwrap_err();
    match error {
        EngineError::Command(error) => assert_eq!(error.code, ErrorCode::Busy),
        other => panic!("unexpected error {other:?}"),
    }
    assert!(state_of(&engine).session(&session.session_id).is_some());
    assert!(
        is_process_alive(&pid),
        "refusing must leave the agent running"
    );

    engine.request(&machine, close(true)).await.unwrap();
    assert!(state_of(&engine).session(&session.session_id).is_none());
    wait_for("the agent's process to die", || {
        (!is_process_alive(&pid)).then_some(())
    })
    .await;
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_answers_a_request_the_agent_is_waiting_on() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;
    prompt(&engine, &session, "write").await.unwrap();
    let approval = wait_for_approval(&engine, &session).await;

    let close = |force| Command::CloseSession {
        session_id: session.session_id.clone(),
        force,
    };
    assert!(engine.request(&machine, close(false)).await.is_err());
    assert_eq!(engine.approvals().len(), 1, "waiting counts as running");

    engine.request(&machine, close(true)).await.unwrap();
    assert!(engine.approvals().is_empty());
    assert!(engine.events_after(&session, 0).iter().any(|event| matches!(
        &event.kind,
        SessionEventKind::PermissionResolved { approval_id, outcome: PermissionOutcome::Cancelled }
            if approval_id == &approval.id
    )));
    daemon.shutdown().await;
}

fn commands_of(engine: &Engine, session: &SessionRef) -> Vec<AgentCommand> {
    state_of(engine)
        .commands
        .get(&session.session_id)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agents_slash_commands_reach_every_client_and_stay_with_its_tab() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let port = daemon.addr().port();
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let other = connect(&daemon).await;
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;
    let wanted = vec![
        AgentCommand {
            name: "usage".to_owned(),
            description: "Runs usage".to_owned(),
            hint: None,
        },
        AgentCommand {
            name: "review".to_owned(),
            description: "Runs review".to_owned(),
            hint: Some("pr".to_owned()),
        },
    ];

    // The ones that change what tectr sets are left out.
    prompt(&engine, &session, "commands usage model review:pr mcp")
        .await
        .unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    for client in [&engine, &other] {
        wait_for("the agent's commands", || {
            (commands_of(client, &session) == wanted).then_some(())
        })
        .await;
    }
    let error = prompt(&engine, &session, " /model opus").await.unwrap_err();
    assert!(
        matches!(&error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput
            && e.message.contains("tectr sets the model")),
        "{error}"
    );
    assert_eq!(status(&engine, &session), Some(SessionStatus::Done));

    // The agent isn't running after a restart, and the list is there before it starts again.
    daemon.shutdown().await;
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let engine = connect(&daemon).await;
    assert_eq!(commands_of(&engine, &session), wanted);

    engine
        .request(&machine, archive_topic(&topic, false))
        .await
        .unwrap();
    assert!(state_of(&engine).commands.is_empty());
    engine
        .request(&machine, restore_topic(&topic))
        .await
        .unwrap();
    assert_eq!(commands_of(&engine, &session), wanted);

    // An agent saying it has none any more clears them.
    engine.open_session(&session);
    prompt(&engine, &session, "commands").await.unwrap();
    wait_for("the commands to go", || {
        commands_of(&engine, &session).is_empty().then_some(())
    })
    .await;
    wait_for_status(&engine, &session, SessionStatus::Done).await;

    prompt(&engine, &session, "commands usage").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    wait_for("the new commands", || {
        (commands_of(&engine, &session).len() == 1).then_some(())
    })
    .await;
    let close = Command::CloseSession {
        session_id: session.session_id.clone(),
        force: false,
    };
    engine.request(&machine, close).await.unwrap();
    assert!(state_of(&engine).commands.is_empty());
    daemon.shutdown().await;
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    assert!(state_of(&connect(&daemon).await).commands.is_empty());
    daemon.shutdown().await;
}

/// What `Command::CreateTopic` makes when nobody gives it a name, which is what the app does.
async fn add_unnamed_topic(engine: &Engine, machine: &MachineId, repo: &Path) -> Topic {
    let workspace = add_workspace(engine, machine, repo).await;
    let command = Command::CreateTopic {
        workspace_id: workspace.id,
        name: None,
        at_root: false,
    };
    match engine.request(machine, command).await.unwrap() {
        CommandOutput::Topic { topic, .. } => topic,
        other => panic!("unexpected output {other:?}"),
    }
}

/// Runs a turn that reports `title`, and returns once it's over and the session shows it.
async fn set_title(engine: &Engine, session: &SessionRef, title: &str) {
    prompt(engine, session, &format!("title {title}"))
        .await
        .unwrap();
    wait_for_status(engine, session, SessionStatus::Done).await;
    wait_for("the session's title", || {
        let state = state_of(engine);
        let shown = state.session(&session.session_id)?.title.as_deref()?;
        (shown == title).then_some(())
    })
    .await;
}

fn topic_name(engine: &Engine, topic: &Topic) -> String {
    state_of(engine).topic(&topic.id).unwrap().name.clone()
}

async fn wait_for_topic_name(engine: &Engine, topic: &Topic, wanted: &str) {
    wait_for("the topic's name", || {
        (topic_name(engine, topic) == wanted).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_topic_nobody_named_goes_by_its_first_tab_until_someone_names_it() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let port = daemon.addr().port();
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;

    let topic = add_unnamed_topic(&engine, &machine, &fixture.repo).await;
    assert_eq!(topic.name, "Topic #1");
    let first = start_session(&engine, &machine, &topic, PermissionMode::Auto, None).await;
    let second = start_session(&engine, &machine, &topic, PermissionMode::Auto, None).await;

    set_title(&engine, &second, "The second tab").await;
    assert_eq!(topic_name(&engine, &topic), "Topic #1");

    set_title(&engine, &first, "The first tab").await;
    wait_for_topic_name(&engine, &topic, "The first tab").await;

    // It's the same name as the tab's, so a later title carries over too.
    prompt(&engine, &first, "title A better title")
        .await
        .unwrap();
    wait_for_topic_name(&engine, &topic, "A better title").await;
    wait_for_status(&engine, &first, SessionStatus::Done).await;

    let rename = |name: &str| Command::RenameTopic {
        topic_id: topic.id.clone(),
        name: name.to_owned(),
    };
    let error = engine.request(&machine, rename("   ")).await.unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));
    assert_eq!(topic_name(&engine, &topic), "A better title");

    engine.request(&machine, rename("  Mine ")).await.unwrap();
    assert_eq!(topic_name(&engine, &topic), "Mine");
    set_title(&engine, &first, "Ignored").await;
    assert_eq!(topic_name(&engine, &topic), "Mine");

    // The branch keeps the name it was made with, and it's the worktree's own.
    assert_eq!(
        state_of(&engine).topic(&topic.id).unwrap().worktrees,
        topic.worktrees
    );

    daemon.shutdown().await;
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let engine = connect(&daemon).await;
    assert_eq!(topic_name(&engine, &topic), "Mine");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_topic_given_a_name_keeps_it() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;

    set_title(&engine, &session, "From the agent").await;

    assert_eq!(topic_name(&engine, &topic), "My topic!");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_topic_never_reuses_a_number_still_in_the_list() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let first = add_unnamed_topic(&engine, &machine, &fixture.repo).await;
    add_unnamed_topic(&engine, &machine, &fixture.repo).await;

    engine
        .request(&machine, archive_topic(&first, false))
        .await
        .unwrap();

    let next = add_unnamed_topic(&engine, &machine, &fixture.repo).await;
    assert_eq!(next.name, "Topic #3");
    daemon.shutdown().await;
}

fn branches(repo: &Path) -> String {
    run_git(repo, &["branch", "--format=%(refname:short)"])
}

fn archive_topic(topic: &Topic, force: bool) -> Command {
    Command::ArchiveTopic {
        topic_id: topic.id.clone(),
        force,
    }
}

fn restore_topic(topic: &Topic) -> Command {
    Command::RestoreTopic {
        topic_id: topic.id.clone(),
    }
}

fn move_topic(topic: &Topic, to: MoveTarget, force: bool) -> Command {
    Command::MoveTopic {
        topic_id: topic.id.clone(),
        to,
        force,
    }
}

fn commit(dir: &Path, message: &str) {
    run_git(
        dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            message,
        ],
    );
}

async fn archived(engine: &Engine, machine: &MachineId) -> Vec<TopicId> {
    match engine
        .request(machine, Command::ListArchivedTopics)
        .await
        .unwrap()
    {
        CommandOutput::ArchivedTopics { topics } => topics.into_iter().map(|t| t.id).collect(),
        other => panic!("unexpected output {other:?}"),
    }
}

async fn add_root_topic(engine: &Engine, machine: &MachineId, repo: &Path) -> Topic {
    let workspace = add_workspace(engine, machine, repo).await;
    let command = Command::CreateTopic {
        workspace_id: workspace.id,
        name: None,
        at_root: true,
    };
    match engine.request(machine, command).await.unwrap() {
        CommandOutput::Topic { topic, .. } => topic,
        other => panic!("unexpected output {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_a_clean_topic_removes_its_folder_and_restoring_brings_it_all_back() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Auto).await;
    let pid = agent_pid(&engine, &session).await;
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let worktree = PathBuf::from(&topic.worktrees[0].path);
    let branch = topic.worktrees[0].branch.clone();
    assert!(worktree.exists());

    let output = engine
        .request(&machine, archive_topic(&topic, false))
        .await
        .unwrap();
    assert_eq!(output, CommandOutput::Done);
    assert!(state_of(&engine).topic(&topic.id).is_none());
    assert!(state_of(&engine).session(&session.session_id).is_none());
    wait_for("the agent's process to die", || {
        (!is_process_alive(&pid)).then_some(())
    })
    .await;
    wait_for("the worktree to go", || (!worktree.exists()).then_some(())).await;
    wait_for("the branch to go", || {
        (!branches(&fixture.repo).contains(&branch)).then_some(())
    })
    .await;
    assert_eq!(
        archived(&engine, &machine).await,
        std::slice::from_ref(&topic.id)
    );

    let error = engine
        .request(&machine, archive_topic(&topic, false))
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::NotFound));

    // Its folder comes back at the same path, so the agent picks up where it was.
    engine
        .request(&machine, restore_topic(&topic))
        .await
        .unwrap();
    let restored = state_of(&engine).topic(&topic.id).cloned().unwrap();
    assert_eq!(restored.worktrees, topic.worktrees);
    assert_eq!(restored.sessions.len(), 1);
    assert_eq!(run_git(&worktree, &["branch", "--show-current"]), branch);
    assert!(archived(&engine, &machine).await.is_empty());
    prompt(&engine, &session, "whoami").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let report = agent_text(&engine, &session).concat();
    assert!(report.contains("resumed=true"), "{report}");
    assert!(
        report.contains(&format!("cwd={}", worktree.display())),
        "{report}"
    );
    assert!(!report.contains("[tectr]"), "nothing moved: {report}");
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_says_what_it_would_lose_and_only_loses_it_when_forced() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;
    let pid = agent_pid(&engine, &session).await;
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let worktree = PathBuf::from(&topic.worktrees[0].path);
    let branch = topic.worktrees[0].branch.clone();
    commit(&worktree, "work");
    std::fs::write(worktree.join("unsaved.txt"), "work").unwrap();
    prompt(&engine, &session, "write").await.unwrap();
    let approval = wait_for_approval(&engine, &session).await;

    let output = engine
        .request(&machine, archive_topic(&topic, false))
        .await
        .unwrap();
    let expected = ArchiveRisk {
        running: 1,
        worktrees: vec![WorktreeRisk {
            path: worktree.display().to_string(),
            branch: branch.clone(),
            uncommitted: 1,
            unpushed: 1,
        }],
    };
    assert_eq!(output, CommandOutput::ArchiveRisk { risk: expected });
    assert!(state_of(&engine).topic(&topic.id).is_some());
    assert!(
        is_process_alive(&pid),
        "asking must leave the agent running"
    );
    assert!(worktree.join("unsaved.txt").exists());

    engine
        .request(&machine, archive_topic(&topic, true))
        .await
        .unwrap();
    assert!(state_of(&engine).topic(&topic.id).is_none());
    assert!(engine.approvals().is_empty());
    assert!(engine.events_after(&session, 0).iter().any(|event| matches!(
        &event.kind,
        SessionEventKind::PermissionResolved { approval_id, outcome: PermissionOutcome::Cancelled }
            if approval_id == &approval.id
    )));
    wait_for("the agent's process to die", || {
        (!is_process_alive(&pid)).then_some(())
    })
    .await;
    wait_for("the worktree to go", || (!worktree.exists()).then_some(())).await;
    daemon.shutdown().await;
    assert!(!branches(&fixture.repo).contains(&branch));
}

#[tokio::test(flavor = "multi_thread")]
async fn pushed_commits_are_not_at_risk() {
    let fixture = Fixture::new();
    let remote = fixture.root.join("remote.git");
    run_git(&fixture.root, &["init", "-q", "--bare", "remote.git"]);
    run_git(
        &fixture.repo,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let worktree = PathBuf::from(&topic.worktrees[0].path);
    commit(&worktree, "pushed work");
    run_git(&worktree, &["push", "-q", "origin", "HEAD"]);

    let output = engine
        .request(&machine, archive_topic(&topic, false))
        .await
        .unwrap();
    assert_eq!(output, CommandOutput::Done);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_a_topic_at_the_repo_root_leaves_the_repo_alone() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let topic = add_root_topic(&engine, &machine, &fixture.repo).await;
    std::fs::write(fixture.repo.join("unsaved.txt"), "work").unwrap();
    let before = branches(&fixture.repo);

    let output = engine
        .request(&machine, archive_topic(&topic, false))
        .await
        .unwrap();
    assert_eq!(
        output,
        CommandOutput::Done,
        "the repo isn't the topic's to lose"
    );
    daemon.shutdown().await;

    assert!(fixture.repo.join(".git").exists());
    assert!(fixture.repo.join("unsaved.txt").exists());
    assert_eq!(branches(&fixture.repo), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_a_topic_takes_its_tabs_along_and_tells_each_agent_once() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let topic = add_root_topic(&engine, &machine, &fixture.repo).await;
    let root = topic.worktrees[0].clone();
    let session = start_session(&engine, &machine, &topic, PermissionMode::Yolo, None).await;
    wait_for_status(&engine, &session, SessionStatus::Ready).await;
    commit(&fixture.repo, "discussed");

    // A new worktree starts from the commit the topic was on.
    engine
        .request(&machine, move_topic(&topic, MoveTarget::NewWorktree, false))
        .await
        .unwrap();
    let moved = state_of(&engine).topic(&topic.id).cloned().unwrap();
    assert_eq!(moved.worktrees.len(), 2);
    let folder = moved.worktree().unwrap().clone();
    assert_ne!(folder.id, root.id);
    assert!(
        folder.branch.starts_with("tectr/topic-1-"),
        "{}",
        folder.branch
    );
    assert_eq!(
        run_git(Path::new(&folder.path), &["rev-parse", "HEAD"]),
        run_git(&fixture.repo, &["rev-parse", "HEAD"])
    );

    prompt(&engine, &session, "whoami").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let said = agent_text(&engine, &session);
    assert!(
        said[0].starts_with("[tectr] This conversation moved"),
        "{said:?}"
    );
    assert!(said[0].contains(&folder.path), "{said:?}");
    assert!(said[1].contains("resumed=true"), "{said:?}");
    assert!(
        said[1].contains(&format!("cwd={}", folder.path)),
        "{said:?}"
    );
    prompt(&engine, &session, "echo again").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    assert_eq!(agent_text(&engine, &session)[2..], ["again"]);

    // The branch shown follows what the agent checks out.
    run_git(Path::new(&folder.path), &["switch", "-q", "-c", "part-2"]);
    prompt(&engine, &session, "echo switched").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    wait_for("the new branch to show", || {
        let topic = state_of(&engine).topic(&topic.id).cloned()?;
        (topic.worktree()?.branch == "part-2").then_some(())
    })
    .await;

    // A running agent has to be confirmed, and going back reuses the repo's folder.
    prompt(&engine, &session, "hang").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Working).await;
    let error = engine
        .request(&machine, move_topic(&topic, MoveTarget::Root, false))
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::Busy));
    engine
        .request(&machine, move_topic(&topic, MoveTarget::Root, true))
        .await
        .unwrap();
    wait_for_status(&engine, &session, SessionStatus::Stopped).await;
    let back = state_of(&engine).topic(&topic.id).cloned().unwrap();
    assert_eq!(back.worktree_id, root.id);
    assert_eq!(back.worktrees.len(), 2);

    // Archiving removes the worktree it left, but not the branch the agent made there.
    engine
        .request(&machine, archive_topic(&topic, true))
        .await
        .unwrap();
    wait_for("the left worktree to go", || {
        (!Path::new(&folder.path).exists()).then_some(())
    })
    .await;
    daemon.shutdown().await;
    let left = branches(&fixture.repo);
    assert!(!left.contains(&folder.branch), "{left}");
    assert!(left.contains("part-2"), "{left}");
}

#[tokio::test(flavor = "multi_thread")]
async fn state_and_events_survive_a_daemon_restart() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let port = daemon.addr().port();
    let machine = daemon.machine_id().clone();
    let engine = connect(&daemon).await;
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;
    prompt(&engine, &session, "echo before").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;

    // A request left open when the daemon stops is cancelled, not lost.
    prompt(&engine, &session, "write").await.unwrap();
    let approval = wait_for_approval(&engine, &session).await;
    daemon.shutdown().await;

    // While the daemon is away, the client keeps the last state and says so.
    wait_for("the client to notice", || {
        matches!(engine.devices()[0].connection, Connection::Offline { .. }).then_some(())
    })
    .await;
    let device = &engine.devices()[0];
    assert!(device.last_seen_ms.is_some());
    assert!(device.state.as_ref().unwrap().topic(&topic.id).is_some());
    let error = prompt(&engine, &session, "echo nope").await.unwrap_err();
    assert!(matches!(error, EngineError::Offline(_)), "{error:?}");

    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    assert_eq!(
        daemon.machine_id(),
        &machine,
        "the machine ID must never change"
    );

    // The client reconnects by itself and carries on from where it stopped.
    wait_for("the client to reconnect", || {
        (engine.devices()[0].connection == Connection::Online).then_some(())
    })
    .await;
    wait_for_status(&engine, &session, SessionStatus::Stopped).await;
    assert_gapless(&engine, &session);
    assert!(engine.approvals().is_empty());
    assert!(engine.events_after(&session, 0).iter().any(|event| matches!(
        &event.kind,
        SessionEventKind::PermissionResolved { approval_id, outcome: PermissionOutcome::Cancelled }
            if approval_id == &approval.id
    )));
    let state = engine.devices()[0].state.clone().unwrap();
    assert_eq!(state.topic(&topic.id).unwrap().worktrees, topic.worktrees);

    // A new prompt brings the agent back through the agent's own resume.
    prompt(&engine, &session, "whoami").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let report = agent_text(&engine, &session).concat();
    assert!(report.contains("resumed=true"), "{report}");
    assert!(
        report.contains("mode=default"),
        "the mode must survive: {report}"
    );
    assert_gapless(&engine, &session);
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_crash_fails_one_session_and_can_be_resumed() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (topic, crashing) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;
    let healthy = start_session(&engine, &machine, &topic, PermissionMode::Manual, None).await;
    wait_for_status(&engine, &healthy, SessionStatus::Ready).await;

    prompt(&engine, &crashing, "crash").await.unwrap();
    wait_for_status(&engine, &crashing, SessionStatus::Failed).await;
    assert!(
        engine
            .events_after(&crashing, 0)
            .iter()
            .any(|event| matches!(event.kind, SessionEventKind::Error { .. })),
        "the failure should be in the transcript"
    );

    // The other session doesn't notice.
    assert_eq!(status(&engine, &healthy), Some(SessionStatus::Ready));
    prompt(&engine, &healthy, "echo fine").await.unwrap();
    wait_for_status(&engine, &healthy, SessionStatus::Done).await;

    // Prompting again restarts and resumes the crashed one.
    prompt(&engine, &crashing, "whoami").await.unwrap();
    wait_for_status(&engine, &crashing, SessionStatus::Done).await;
    assert!(
        agent_text(&engine, &crashing)
            .concat()
            .contains("resumed=true")
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_prompt_during_a_turn_is_refused_and_cancel_stops_it() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;

    let cancel = Command::CancelTurn {
        session_id: session.session_id.clone(),
    };
    assert!(
        engine.request(&machine, cancel.clone()).await.is_err(),
        "there's nothing to cancel yet"
    );

    prompt(&engine, &session, "hang").await.unwrap();
    let error = prompt(&engine, &session, "echo too soon")
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::Busy));

    // Give the agent a moment to start waiting, then interrupt it.
    tokio::time::sleep(Duration::from_millis(200)).await;
    engine.request(&machine, cancel).await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    assert!(
        engine
            .events_after(&session, 0)
            .iter()
            .any(|event| matches!(
                event.kind,
                SessionEventKind::TurnEnded {
                    stop_reason: StopReason::Cancelled
                }
            ))
    );

    // The session is usable again.
    prompt(&engine, &session, "echo again").await.unwrap();
    wait_for("the next turn's answer", || {
        agent_text(&engine, &session)
            .contains(&"again".to_owned())
            .then_some(())
    })
    .await;
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelling_answers_an_open_permission_request() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (topic, session) =
        start_ready_session(&engine, &machine, &fixture.repo, PermissionMode::Manual).await;

    prompt(&engine, &session, "write").await.unwrap();
    let approval = wait_for_approval(&engine, &session).await;
    engine
        .request(
            &machine,
            Command::CancelTurn {
                session_id: session.session_id.clone(),
            },
        )
        .await
        .unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;

    assert!(engine.approvals().is_empty());
    assert!(
        !Path::new(&topic.worktrees[0].path)
            .join("fake-output.txt")
            .exists()
    );
    let error = answer(&engine, &session, &approval, "allow")
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::NotFound));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_client_merges_many_daemons() {
    let fixture = Fixture::new();
    let laptop = fixture.daemon("laptop").await;
    let desktop = fixture.daemon("desktop").await;
    assert_ne!(laptop.machine_id(), desktop.machine_id());

    let engine = Engine::new("e2e");
    engine.add_daemon(&url(&laptop));
    engine.add_daemon(&url(&desktop));
    engine.add_daemon("ws://127.0.0.1:1"); // Offline, and must not get in the way.
    engine.wait_until_settled().await;
    assert_eq!(
        engine
            .devices()
            .iter()
            .filter(|device| device.connection == Connection::Online)
            .count(),
        2
    );

    let (_, on_laptop) = start_ready_session(
        &engine,
        laptop.machine_id(),
        &fixture.repo,
        PermissionMode::Manual,
    )
    .await;
    let (_, on_desktop) = start_ready_session(
        &engine,
        desktop.machine_id(),
        &fixture.repo,
        PermissionMode::Manual,
    )
    .await;

    // Commands go to the right machine, and events keep their own machine's ID.
    prompt(&engine, &on_laptop, "echo from-laptop")
        .await
        .unwrap();
    prompt(&engine, &on_desktop, "write").await.unwrap();
    wait_for_status(&engine, &on_laptop, SessionStatus::Done).await;
    let approval = wait_for_approval(&engine, &on_desktop).await;
    assert_eq!(agent_text(&engine, &on_laptop), ["from-laptop"]);
    assert!(
        engine
            .events_after(&on_desktop, 0)
            .iter()
            .all(|e| e.session == on_desktop)
    );

    // Approvals span machines, and a machine that doesn't own a session says so.
    assert_eq!(engine.approvals().len(), 1);
    let wrong_machine = SessionRef {
        machine_id: laptop.machine_id().clone(),
        session_id: on_desktop.session_id.clone(),
    };
    let error = prompt(&engine, &wrong_machine, "echo lost")
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Command(e) if e.code == ErrorCode::NotFound));
    answer(&engine, &on_desktop, &approval, "allow")
        .await
        .unwrap();
    wait_for_status(&engine, &on_desktop, SessionStatus::Done).await;

    laptop.shutdown().await;
    desktop.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_requests_get_clear_errors() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();

    let invalid = |result: Result<CommandOutput, EngineError>| matches!(result, Err(EngineError::Command(e)) if e.code == ErrorCode::InvalidInput);
    let add = |path: &str| Command::AddWorkspace {
        path: path.to_owned(),
    };
    assert!(invalid(
        engine.request(&machine, add("relative/path")).await
    ));
    assert!(
        invalid(
            engine
                .request(&machine, add(fixture.root.to_str().unwrap()))
                .await
        ),
        "not a repo"
    );

    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let unknown_agent = start_agent(
        &engine,
        &machine,
        &topic.id,
        "nope",
        PermissionMode::Auto,
        None,
    )
    .await;
    assert!(invalid(unknown_agent.map(|_| CommandOutput::Done)));
    let unknown_mode = start_agent(
        &engine,
        &machine,
        &topic.id,
        "fake",
        PermissionMode::Unknown,
        None,
    )
    .await;
    assert!(invalid(unknown_mode.map(|_| CommandOutput::Done)));

    let session = start_session(&engine, &machine, &topic, PermissionMode::Auto, None).await;
    assert!(invalid(prompt(&engine, &session, "   ").await));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_cant_start_fails_its_session() {
    let fixture = Fixture::new();
    let mut config = fixture.config("d", 0);
    config.agents.insert(
        "missing".to_owned(),
        AgentLauncher::Command {
            program: "/does/not/exist".into(),
            args: vec![],
        },
    );
    let daemon = Daemon::start(config).await.unwrap();
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let session = start_agent(
        &engine,
        &machine,
        &topic.id,
        "missing",
        PermissionMode::Auto,
        None,
    )
    .await
    .unwrap();
    let session = SessionRef {
        machine_id: machine,
        session_id: session.id,
    };
    engine.open_session(&session);

    wait_for_status(&engine, &session, SessionStatus::Failed).await;
    assert!(
        engine
            .events_after(&session, 0)
            .iter()
            .any(|event| matches!(event.kind, SessionEventKind::Error { .. }))
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_version_mismatch_is_rejected_and_reported() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let mut socket = raw_connect(&daemon).await;
    let reply = say_hello(&mut socket, 900, 901).await;
    assert_eq!(
        reply,
        ServerMessage::VersionRejected {
            min_version: 1,
            max_version: 1
        }
    );
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn browsers_cannot_connect() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let mut request = url(&daemon).into_client_request().unwrap();
    request
        .headers_mut()
        .insert("Origin", "https://evil.example".parse().unwrap());
    let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(error.to_string().contains("403"), "{error}");
    daemon.shutdown().await;
}

type RawSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn raw_connect(daemon: &Daemon) -> RawSocket {
    tokio_tungstenite::connect_async(url(daemon))
        .await
        .unwrap()
        .0
}

async fn raw_send(socket: &mut RawSocket, message: &ClientMessage) {
    socket
        .send(Message::text(serde_json::to_string(message).unwrap()))
        .await
        .unwrap();
}

async fn raw_next(socket: &mut RawSocket) -> ServerMessage {
    let frame = tokio::time::timeout(TIMEOUT, socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

async fn say_hello(socket: &mut RawSocket, min_version: u32, max_version: u32) -> ServerMessage {
    raw_send(
        socket,
        &ClientMessage::Hello {
            min_version,
            max_version,
            client: "raw".to_owned(),
        },
    )
    .await;
    raw_next(socket).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_finished_turn_is_unread_until_any_client_reads_it() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let desk = connect(&daemon).await;
    let pocket = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let (_, session) =
        start_ready_session(&desk, &machine, &fixture.repo, PermissionMode::Manual).await;
    let unread = |engine: &Engine| session_state(engine, &session).unread;
    let set_unread = |unread: bool| {
        desk.request(
            &machine,
            Command::SetUnread {
                session_id: session.session_id.clone(),
                unread,
            },
        )
    };
    assert!(!unread(&desk), "a fresh session has nothing to read");
    assert!(
        matches!(set_unread(true).await, Err(EngineError::Command(error)) if error.code == ErrorCode::InvalidInput),
        "only a finished session can be unread"
    );

    prompt(&desk, &session, "echo a b").await.unwrap();
    wait_for_status(&desk, &session, SessionStatus::Done).await;
    assert!(unread(&desk));
    wait_for("the other client to see it unread", || {
        unread(&pocket).then_some(())
    })
    .await;

    set_unread(false).await.unwrap();
    wait_for("the other client to see it read", || {
        (!unread(&pocket)).then_some(())
    })
    .await;
    set_unread(true).await.unwrap();
    assert!(unread(&desk), "it can be marked unread again");

    prompt(&desk, &session, "echo c").await.unwrap();
    assert!(!unread(&desk), "working on something new clears it");
    wait_for_status(&desk, &session, SessionStatus::Done).await;
    assert!(unread(&desk));
    daemon.shutdown().await;
}

/// What a client has drawn of a terminal: everything since its last reset, and how much to
/// acknowledge.
#[derive(Default)]
struct Screen {
    text: String,
    resets: usize,
    unacked: u32,
}

fn attach(
    engine: &Engine,
    machine: &MachineId,
    terminal: &protocol::TerminalId,
) -> std::sync::Arc<std::sync::Mutex<Screen>> {
    let screen = std::sync::Arc::new(std::sync::Mutex::new(Screen::default()));
    let drawn = screen.clone();
    engine.attach_terminal(
        machine,
        terminal,
        (80, 24),
        None,
        std::sync::Arc::new(move |kind, payload| {
            let mut screen = drawn.lock().unwrap();
            screen.unacked += payload.len() as u32;
            match kind {
                protocol::frame::Kind::Reset => {
                    let (_, _, text) = protocol::frame::read_reset(&payload).unwrap();
                    screen.text = String::from_utf8_lossy(text).into_owned();
                    screen.resets += 1;
                }
                _ => screen.text += &String::from_utf8_lossy(&payload),
            }
        }),
    );
    screen
}

async fn wait_for_text(
    engine: &Engine,
    machine: &MachineId,
    terminal: &protocol::TerminalId,
    screen: &std::sync::Mutex<Screen>,
    text: &str,
) {
    wait_for(text, || {
        let mut screen = screen.lock().unwrap();
        let unacked = std::mem::take(&mut screen.unacked);
        engine.terminal_ack(machine, terminal, unacked);
        screen.text.contains(text).then_some(())
    })
    .await;
}

fn terminal_state(engine: &Engine, id: &protocol::TerminalId) -> Option<protocol::Terminal> {
    engine.devices()[0].state.as_ref()?.terminal(id).cloned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_terminal_runs_a_shell_and_comes_back_where_it_was_after_a_restart() {
    let fixture = Fixture::new();
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let opened = engine
        .request(
            &machine,
            Command::OpenTerminal {
                topic_id: topic.id.clone(),
                cols: 80,
                rows: 24,
            },
        )
        .await
        .unwrap();
    let CommandOutput::Terminal { terminal, .. } = opened else {
        panic!("unexpected output {opened:?}");
    };
    assert_eq!(
        terminal_state(&engine, &terminal.id).map(|t| t.status),
        Some(protocol::TerminalStatus::Running)
    );

    let screen = attach(&engine, &machine, &terminal.id);
    let folder = topic.worktree().unwrap().path.clone();
    let typed = "mkdir inner && cd inner && echo here-$((40+2)) && pwd\n";
    engine
        .terminal_input(&machine, &terminal.id, typed.as_bytes())
        .unwrap();
    wait_for_text(&engine, &machine, &terminal.id, &screen, "here-42").await;
    wait_for_text(
        &engine,
        &machine,
        &terminal.id,
        &screen,
        &format!("{folder}/inner"),
    )
    .await;
    daemon.shutdown().await;

    // The tab stays, and showing it starts a shell where the last one was, below what it left.
    let daemon = Daemon::start(fixture.config("d", port)).await.unwrap();
    let engine = connect(&daemon).await;
    assert_eq!(
        terminal_state(&engine, &terminal.id).map(|t| t.status),
        Some(protocol::TerminalStatus::Stopped)
    );
    let screen = attach(&engine, &machine, &terminal.id);
    wait_for_text(&engine, &machine, &terminal.id, &screen, "here-42").await;
    engine
        .terminal_input(
            &machine,
            &terminal.id,
            b"echo now-in-$(basename \"$PWD\")-$((1+1))\n",
        )
        .unwrap();
    wait_for_text(&engine, &machine, &terminal.id, &screen, "now-in-inner-2").await;
    wait_for("the terminal to be running again", || {
        (terminal_state(&engine, &terminal.id)?.status == protocol::TerminalStatus::Running)
            .then_some(())
    })
    .await;

    // Exiting the shell closes its tab.
    engine
        .terminal_input(&machine, &terminal.id, b"exit\n")
        .unwrap();
    wait_for("the terminal's tab to close", || {
        terminal_state(&engine, &terminal.id)
            .is_none()
            .then_some(())
    })
    .await;
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_terminal_removes_its_tab_and_what_it_showed() {
    let fixture = Fixture::new();
    let daemon = fixture.daemon("d").await;
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let topic = add_topic(&engine, &machine, &fixture.repo).await;
    let CommandOutput::Terminal { terminal, .. } = engine
        .request(
            &machine,
            Command::OpenTerminal {
                topic_id: topic.id.clone(),
                cols: 80,
                rows: 24,
            },
        )
        .await
        .unwrap()
    else {
        panic!("expected a terminal");
    };
    let history = fixture
        .root
        .join("Application Support")
        .join("d")
        .join("terminals")
        .join(format!("{}.vt", terminal.id));
    engine
        .request(
            &machine,
            Command::CloseTerminal {
                terminal_id: terminal.id.clone(),
            },
        )
        .await
        .unwrap();
    assert!(terminal_state(&engine, &terminal.id).is_none());
    // The terminal's thread finishes deleting it on its way out.
    wait_for("its history to be gone", || {
        (!history.exists()).then_some(())
    })
    .await;
    daemon.shutdown().await;
}

// Remote clients, over iroh on loopback: no relay, and addresses come from a lookup the test fills in.

async fn remote_daemon(fixture: &Fixture, name: &str) -> Daemon {
    Daemon::start(Config {
        network: daemon::Network::Local,
        ..fixture.config(name, 0)
    })
    .await
    .unwrap()
}

async fn pairing_code(local: &Engine, machine: &MachineId) -> String {
    match local.request(machine, Command::StartPairing).await.unwrap() {
        CommandOutput::Pairing { code, .. } => code,
        other => panic!("unexpected output {other:?}"),
    }
}

/// A client on "another machine", which finds the daemon only through the lookup.
fn remote_engine(daemon: &Daemon) -> Engine {
    let lookup = iroh::address_lookup::MemoryLookup::new();
    lookup.add_endpoint_info(daemon.remote_addr().expect("the daemon listens remotely"));
    Engine::with_remote(
        "e2e-remote",
        Remote {
            key: iroh::SecretKey::generate(),
            network: engine::Network::Local(lookup),
        },
    )
}

fn connection(engine: &Engine, url: &str) -> Option<Connection> {
    engine
        .devices()
        .into_iter()
        .find(|device| device.url == url)
        .map(|device| device.connection)
}

#[tokio::test]
async fn a_paired_client_works_over_iroh_until_it_is_removed() {
    let fixture = Fixture::new();
    let daemon = remote_daemon(&fixture, "d").await;
    let machine = daemon.machine_id().clone();
    let local = connect(&daemon).await;
    assert!(
        daemon.remote_addr().is_none(),
        "nothing listens before a pairing"
    );
    let code = pairing_code(&local, &machine).await;

    let remote = remote_engine(&daemon);
    let url = remote.pair(&code, "Phone").await.unwrap();
    remote.wait_until_settled().await;
    assert_eq!(connection(&remote, &url), Some(Connection::Online));
    let client = wait_for("the paired client", || {
        local.devices()[0]
            .state
            .as_ref()
            .and_then(|state| state.paired_clients.first().cloned())
    })
    .await;
    assert_eq!(client.name, "Phone");

    // The same protocol as loopback, agents included.
    let (_, session) =
        start_ready_session(&remote, &machine, &fixture.repo, PermissionMode::Auto).await;
    prompt(&remote, &session, "echo over iroh").await.unwrap();
    wait_for("the echo", || {
        (agent_text(&remote, &session) == ["over", "iroh"]).then_some(())
    })
    .await;

    let refused = remote
        .request(&machine, Command::StartPairing)
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("only starts on the device itself"),
        "{refused}"
    );

    local
        .request(&machine, Command::RemovePairedClient { key: client.key })
        .await
        .unwrap();
    let reason = wait_for("the removed client to go offline", || {
        match connection(&remote, &url) {
            Some(Connection::Offline { reason }) if reason.contains("not paired") => Some(reason),
            _ => None,
        }
    })
    .await;
    assert!(reason.contains("not paired"), "{reason}");
    daemon.shutdown().await;
}

#[tokio::test]
async fn strangers_and_wrong_codes_are_refused() {
    let fixture = Fixture::new();
    let daemon = remote_daemon(&fixture, "d").await;
    let machine = daemon.machine_id().clone();
    let local = connect(&daemon).await;
    let code = pairing_code(&local, &machine).await;
    let parsed = protocol::PairingCode::parse(&code).unwrap();

    // Knowing the key isn't enough to connect.
    let stranger = remote_engine(&daemon);
    let url = protocol::remote_daemon_url(&parsed.daemon_key);
    stranger.add_daemon(&url);
    stranger.wait_until_settled().await;
    match connection(&stranger, &url) {
        Some(Connection::Offline { reason }) => assert!(reason.contains("not paired"), "{reason}"),
        other => panic!("a stranger got {other:?}"),
    }

    let wrong = protocol::PairingCode {
        secret: "0".repeat(32),
        ..parsed.clone()
    };
    let error = stranger.pair(&wrong.to_string(), "x").await.unwrap_err();
    assert!(error.to_string().contains("wrong"), "{error}");
    assert!(stranger.pair("not a code", "x").await.is_err());

    local
        .request(&machine, Command::CancelPairing)
        .await
        .unwrap();
    let error = stranger.pair(&code, "x").await.unwrap_err();
    assert!(error.to_string().contains("isn't pairing"), "{error}");
    let state = local.devices()[0].state.clone().unwrap();
    assert!(state.paired_clients.is_empty());
    daemon.shutdown().await;
}

#[tokio::test]
async fn a_paired_daemon_listens_again_after_a_restart() {
    let fixture = Fixture::new();
    let daemon = remote_daemon(&fixture, "d").await;
    let machine = daemon.machine_id().clone();
    let local = connect(&daemon).await;
    let code = pairing_code(&local, &machine).await;
    let key = iroh::SecretKey::generate();
    let lookup = iroh::address_lookup::MemoryLookup::new();
    lookup.add_endpoint_info(daemon.remote_addr().unwrap());
    let remote = Engine::with_remote(
        "e2e-remote",
        Remote {
            key: key.clone(),
            network: engine::Network::Local(lookup.clone()),
        },
    );
    let url = remote.pair(&code, "Laptop").await.unwrap();
    drop(remote);
    drop(local);
    daemon.shutdown().await;

    let daemon = remote_daemon(&fixture, "d").await;
    let addr = daemon
        .remote_addr()
        .expect("a daemon with paired clients listens straight away");
    lookup.set_endpoint_info(addr);
    let remote = Engine::with_remote(
        "e2e-remote",
        Remote {
            key,
            network: engine::Network::Local(lookup),
        },
    );
    remote.add_daemon(&url);
    remote.wait_until_settled().await;
    assert_eq!(connection(&remote, &url), Some(Connection::Online));
    daemon.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_imported_conversation_shows_its_history_and_carries_on_where_it_was() {
    let fixture = Fixture::new();
    let home = fixture.root.join("home");
    let id = "7f0c2a52-9a8e-4c1e-9d47-3c1f1c0d2b6e";
    let conversation = [
        r#"{"parentUuid":null,"isSidechain":false,"type":"user","message":{"role":"user","content":"Fix the login bug"},"uuid":"u1","timestamp":"2026-11-10T11:30:00.000Z","cwd":"/elsewhere"}"#,
        r#"{"parentUuid":"u1","isSidechain":false,"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Fixed it."}]},"uuid":"a1","timestamp":"2026-11-10T11:30:05.000Z"}"#,
        r#"{"type":"ai-title","aiTitle":"Login bug"}"#,
    ]
    .join("\n");
    let folder = home.join(".claude/projects/-elsewhere");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(format!("{id}.jsonl")), conversation).unwrap();

    // The fake agent stands in for Claude, whose conversations these are.
    let mut config = fixture.config("d", 0);
    config.agents = BTreeMap::from([(
        "claude".to_owned(),
        AgentLauncher::Command {
            program: env!("CARGO_BIN_EXE_fake-agent").into(),
            args: vec![],
        },
    )]);
    config.mcp_home = daemon::ConfigHome {
        home: Some(home),
        codex_home: None,
        xdg_config: None,
        opencode_config: None,
        opencode_content: None,
    };
    let daemon = Daemon::start(config).await.unwrap();
    let engine = connect(&daemon).await;
    let machine = daemon.machine_id().clone();
    let list = async || match engine
        .request(&machine, Command::ListConversations)
        .await
        .unwrap()
    {
        CommandOutput::Conversations { conversations } => conversations,
        other => panic!("unexpected output {other:?}"),
    };
    let import = |topic: &Topic| Command::ImportConversation {
        topic_id: topic.id.clone(),
        agent: "claude".to_owned(),
        id: id.to_owned(),
    };

    // Only an agent added to the device has conversations to import.
    assert_eq!(list().await, []);
    let topic = add_unnamed_topic(&engine, &machine, &fixture.repo).await;
    let refused = engine.request(&machine, import(&topic)).await.unwrap_err();
    assert!(matches!(refused, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));
    let add = Command::AddProvider {
        agent: "claude".to_owned(),
    };
    engine.request(&machine, add).await.unwrap();
    let found = list().await;
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].id.as_str(), found[0].title.as_str()),
        (id, "Login bug")
    );
    assert_eq!(found[0].cwd, "/elsewhere");

    let session = match engine.request(&machine, import(&topic)).await.unwrap() {
        CommandOutput::Session { session, .. } => session,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(session.status, SessionStatus::Stopped);
    assert_eq!(session.title.as_deref(), Some("Login bug"));
    assert_eq!(session.mode, PermissionMode::Auto);
    let session = SessionRef {
        machine_id: machine.clone(),
        session_id: session.id,
    };
    engine.open_session(&session);
    wait_for("the imported history", || {
        (agent_text(&engine, &session) == ["Fixed it."]).then_some(())
    })
    .await;
    let history = engine.events_after(&session, 0);
    assert!(
        matches!(&history[0].kind, SessionEventKind::UserPrompt { text } if text == "Fix the login bug")
    );
    assert_eq!(history[0].at_ms, 1_794_310_200_000);
    assert_gapless(&engine, &session);
    wait_for_topic_name(&engine, &topic, "Login bug").await;

    // Once open, it can't be listed or imported again.
    assert_eq!(list().await, []);
    let twice = engine.request(&machine, import(&topic)).await.unwrap_err();
    assert!(matches!(twice, EngineError::Command(e) if e.code == ErrorCode::InvalidInput));

    // Its first prompt resumes the agent's own session, told it's in a new folder now.
    prompt(&engine, &session, "whoami").await.unwrap();
    wait_for_status(&engine, &session, SessionStatus::Done).await;
    let said = agent_text(&engine, &session);
    assert!(
        said[1].starts_with("[tectr] This conversation moved"),
        "{said:?}"
    );
    assert!(said[1].contains("no longer in /elsewhere"), "{said:?}");
    assert!(
        said[2].contains(&format!("resumed=true session={id}")),
        "{said:?}"
    );
    daemon.shutdown().await;
}
