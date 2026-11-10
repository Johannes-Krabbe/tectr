//! Tests the engine against a scripted daemon, to check exactly what it sends
//! and how it treats responses a real daemon wouldn't normally produce.

use std::time::Duration;

use engine::{Connection, Engine};
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

async fn receive(socket: &mut Socket) -> ClientMessage {
    let frame = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .expect("the client should have sent something")
        .unwrap()
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
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
    assert!(matches!(result, Err(engine::EngineError::Disconnected)));
}
