//! Tests the engine against a scripted daemon, to check exactly what it sends
//! and how it treats responses a real daemon wouldn't normally produce.

use std::time::Duration;

use engine::{Connection, Engine, EngineError, Liveness};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    ClientMessage, DaemonState, Machine, ServerMessage, SessionEvent, SessionEventKind, SessionRef,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

type Socket = WebSocketStream<TcpStream>;

fn machine() -> Machine {
    Machine {
        id: "m".into(),
        name: "scripted".into(),
    }
}

fn session() -> SessionRef {
    SessionRef {
        machine_id: "m".into(),
        session_id: "s".into(),
    }
}

fn event(seq: u64, text: &str) -> ServerMessage {
    ServerMessage::SessionEvent {
        event: SessionEvent {
            session: session(),
            seq,
            at_ms: 0,
            kind: SessionEventKind::AgentMessage {
                text: text.to_owned(),
            },
        },
    }
}

async fn send(socket: &mut Socket, message: &ServerMessage) {
    let text = serde_json::to_string(message).unwrap();
    socket.send(Message::text(text)).await.unwrap();
}

/// The client's next message. Reading is also what answers its pings.
async fn receive(socket: &mut Socket) -> ClientMessage {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("the client should have sent something")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// Accepts a connection and completes the handshake.
async fn accept(listener: &TcpListener) -> Socket {
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    assert!(matches!(
        receive(&mut socket).await,
        ClientMessage::Hello { .. }
    ));
    send(
        &mut socket,
        &ServerMessage::Welcome {
            version: 1,
            machine: machine(),
            daemon_version: "scripted".to_owned(),
        },
    )
    .await;
    send(
        &mut socket,
        &ServerMessage::Snapshot {
            state: DaemonState {
                machine: machine(),
                workspaces: vec![],
                approvals: vec![],
                providers: vec![],
                drafts: Default::default(),
                draft_images: Default::default(),
                commands: Default::default(),
                settings: Default::default(),
                mcp_servers: vec![],
                paired_clients: vec![],
            },
        },
    )
    .await;
    socket
}

async fn wait_for_events(engine: &Engine, count: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while engine.events_after(&session(), 0).len() < count {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the events should have arrived");
}

async fn listener_and_engine() -> (TcpListener, Engine) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let engine = Engine::new("test");
    engine.add_daemon(&format!("ws://{}", listener.local_addr().unwrap()));
    (listener, engine)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reconnect_resumes_after_the_last_event_seen() {
    let (listener, engine) = listener_and_engine().await;
    let mut socket = accept(&listener).await;
    engine.wait_until_settled().await;

    engine.open_session(&session());
    let ClientMessage::Subscribe { after_seq, .. } = receive(&mut socket).await else {
        panic!("expected a subscribe");
    };
    assert_eq!(after_seq, 0, "a new session starts from the beginning");

    send(&mut socket, &event(1, "one")).await;
    send(&mut socket, &event(2, "two")).await;
    wait_for_events(&engine, 2).await;
    drop(socket);

    let mut socket = accept(&listener).await;
    let ClientMessage::Subscribe { after_seq, .. } = receive(&mut socket).await else {
        panic!("expected a subscribe");
    };
    assert_eq!(
        after_seq, 2,
        "a reconnect asks only for what it hasn't seen"
    );

    send(&mut socket, &event(3, "three")).await;
    wait_for_events(&engine, 3).await;
    let seqs: Vec<u64> = engine
        .events_after(&session(), 0)
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, [1, 2, 3]);
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_and_out_of_order_events_are_ignored() {
    let (listener, engine) = listener_and_engine().await;
    let mut socket = accept(&listener).await;
    engine.wait_until_settled().await;
    engine.open_session(&session());
    receive(&mut socket).await;

    send(&mut socket, &event(50, "stale, from before a reopen")).await;
    send(&mut socket, &event(1, "one")).await;
    send(&mut socket, &event(1, "one again")).await;
    send(&mut socket, &event(2, "two")).await;
    wait_for_events(&engine, 2).await;

    let texts: Vec<_> = engine
        .events_after(&session(), 0)
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        texts,
        [
            SessionEventKind::AgentMessage {
                text: "one".to_owned()
            },
            SessionEventKind::AgentMessage {
                text: "two".to_owned()
            },
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_without_a_common_version_is_reported() {
    let (listener, engine) = listener_and_engine().await;
    let (stream, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
    receive(&mut socket).await;
    send(
        &mut socket,
        &ServerMessage::VersionRejected {
            min_version: 7,
            max_version: 8,
        },
    )
    .await;

    engine.wait_until_settled().await;
    assert_eq!(
        engine.devices()[0].connection,
        Connection::Incompatible {
            daemon_min: 7,
            daemon_max: 8
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_fails_when_the_connection_drops() {
    let (listener, engine) = listener_and_engine().await;
    let mut socket = accept(&listener).await;
    engine.wait_until_settled().await;

    let machine_id = machine().id;
    let request = engine.request(
        &machine_id,
        protocol::Command::CancelTurn {
            session_id: "s".into(),
        },
    );
    let close_after_receiving = async move {
        assert!(matches!(
            receive(&mut socket).await,
            ClientMessage::Request { .. }
        ));
        drop(socket);
    };
    let (result, ()) = tokio::join!(request, close_after_receiving);
    assert!(matches!(result, Err(EngineError::Disconnected)));
}

/// Quick enough to test with, with room between the steps for a busy machine.
fn quick() -> Liveness {
    Liveness {
        ping_every: Duration::from_millis(50),
        stalled_after: Duration::from_millis(250),
        silence_limit: Duration::from_secs(3),
    }
}

async fn listener_and_quick_engine() -> (TcpListener, Engine) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let engine = Engine::new("test").with_liveness(quick());
    engine.add_daemon(&format!("ws://{}", listener.local_addr().unwrap()));
    (listener, engine)
}

async fn wait_for_connection(engine: &Engine, what: &str, wanted: impl Fn(&Connection) -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !wanted(&engine.devices()[0].connection) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

fn cancel_turn() -> protocol::Command {
    protocol::Command::CancelTurn {
        session_id: "s".into(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_keeps_answering_pings_stays_online() {
    let (listener, engine) = listener_and_quick_engine().await;
    let mut socket = accept(&listener).await;
    engine.wait_until_settled().await;

    // Nothing is sent but pongs, for longer than the engine would wait in silence.
    let mut pings = 0;
    let until = tokio::time::Instant::now() + quick().silence_limit + Duration::from_secs(1);
    while let Ok(frame) = tokio::time::timeout_at(until, socket.next()).await {
        if matches!(frame.unwrap().unwrap(), Message::Ping(_)) {
            pings += 1;
        }
    }
    assert!(pings > 10, "only {pings} pings");
    assert_eq!(
        engine.devices()[0].connection,
        Connection::Online { stalled: false }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_goes_quiet_is_shown_as_stalled_until_it_answers_again() {
    let (listener, engine) = listener_and_quick_engine().await;
    let mut socket = accept(&listener).await;
    engine.wait_until_settled().await;

    // Not reading leaves the pings unanswered.
    wait_for_connection(&engine, "the stall to show", |connection| {
        connection == &Connection::Online { stalled: true }
    })
    .await;

    let answering = tokio::spawn(async move { while socket.next().await.is_some() {} });
    wait_for_connection(&engine, "the stall to clear", |connection| {
        connection == &Connection::Online { stalled: false }
    })
    .await;
    answering.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_stays_quiet_is_dropped_and_what_waited_on_it_fails() {
    let (listener, engine) = listener_and_quick_engine().await;
    // Held open and never read, like a connection to a laptop with its lid closed.
    let quiet = accept(&listener).await;
    engine.wait_until_settled().await;

    let machine_id = machine().id;
    let waiting = engine.request(&machine_id, cancel_turn()).await;
    assert!(
        matches!(waiting, Err(EngineError::Disconnected)),
        "{waiting:?}"
    );
    let device = &engine.devices()[0];
    assert_eq!(
        device.connection,
        Connection::Offline {
            reason: "the device stopped answering".to_owned()
        }
    );
    assert!(device.last_seen_ms.is_some());

    // Nothing more is sent into the silence: the next command is refused straight away.
    let refused = engine.request(&machine_id, cancel_turn()).await;
    assert!(
        matches!(&refused, Err(EngineError::Offline(name)) if name == "scripted"),
        "{refused:?}"
    );

    // It comes back on a new connection, and the old one never gets the command again.
    let mut socket = accept(&listener).await;
    wait_for_connection(&engine, "the reconnect", Connection::is_online).await;
    let answer = async {
        let ClientMessage::Request { id, .. } = receive(&mut socket).await else {
            panic!("expected a request");
        };
        let result = Ok(protocol::CommandOutput::Done);
        send(&mut socket, &ServerMessage::Response { id, result }).await;
    };
    let (result, ()) = tokio::join!(engine.request(&machine_id, cancel_turn()), answer);
    assert!(matches!(result, Ok(protocol::CommandOutput::Done)));
    drop(quiet);
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_to_reconnect_starts_a_new_attempt_straight_away() {
    let (listener, engine) = listener_and_engine().await;
    // A device that takes the connection and then says nothing holds an attempt up.
    let (stuck, _) = listener.accept().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(engine.devices()[0].connection, Connection::Connecting);

    engine.reconnect();
    let _socket = tokio::time::timeout(Duration::from_secs(5), accept(&listener))
        .await
        .expect("a new attempt should have started");
    wait_for_connection(&engine, "the connection", Connection::is_online).await;
    drop(stuck);
}
