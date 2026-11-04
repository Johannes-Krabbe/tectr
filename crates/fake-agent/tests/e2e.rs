//! End-to-end tests: a real daemon and real client engines over real websockets,
//! with the fake agent as a real process. They live in this crate because Cargo
//! only exposes a binary's path to tests in the same package, and it keeps the
//! daemon and engine from depending on each other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use daemon::{AgentLauncher, Config, Daemon};
use engine::{Connection, Engine, EngineError};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    Approval, ClientMessage, Command, CommandOutput, ErrorCode, MachineId, PermissionMode,
    PermissionOutcome, ServerMessage, Session, SessionEventKind, SessionRef, SessionStatus,
    StopReason, Topic, TopicId, Workspace,
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
        name: "My topic!".to_owned(),
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
    let session = start_agent(engine, machine, &topic.id, topic, "fake", mode, model)
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
    topic: &Topic,
    agent: &str,
    mode: PermissionMode,
    model: Option<&str>,
) -> Result<Session, EngineError> {
    let output = engine
        .request(
            machine,
            Command::StartSession {
                topic_id: topic_id.clone(),
                worktree_id: topic.worktrees[0].id.clone(),
                agent: agent.to_owned(),
                mode,
                model: model.map(str::to_owned),
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
    wait_for("the second client's inbox to clear", || {
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

    // The inbox spans machines, and a machine that doesn't own a session says so.
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
        &topic,
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
        &topic,
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
        &topic,
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
